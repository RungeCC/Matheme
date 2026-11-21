//! Planar non-zero to even-odd winding resolution for filled vector contours.
//!
//! Wolfram Language's `FilledCurve` renders multiple contours under the even-odd
//! rule, whereas Typst shapes and variable fonts often assume the non-zero
//! winding rule. For ordinary contours (no self-intersections, no crossings, and
//! no same-sign nested boundaries), non-zero and even-odd produce identical
//! fills, so a fast structural check leaves exact cubic Béziers untouched with
//! zero cloning. When self-intersections, crossings, or same-sign nestings exist,
//! we extract the exact exterior boundary of the non-zero filled region
//! (`winding != 0`) using sweep-line pruned intersection splitting within strict
//! complexity budgets.

use std::collections::{HashMap, HashSet};

use kurbo::{BezPath, PathEl, Point, Rect, Vec2};

use crate::extract::{Segment, Subpath};

type VKey = (i64, i64);

/// Maximum flattened vertices permitted during the fast equivalence check.
/// Above this threshold we skip planar boolean resolution and report a
/// degradation warning rather than risking quadratic blowup on dense paths.
pub(crate) const MAX_WINDING_CHECK_VERTICES: usize = 4096;

/// Maximum raw segments permitted in the slow-path planar boundary extractor.
pub(crate) const MAX_WINDING_RESOLVE_EDGES: usize = 1024;

/// Maximum split sub-segments permitted during slow-path boundary extraction
/// (guards against dense all-pairs crossing stars/grids with large `K`).
const MAX_WINDING_SPLIT_EDGES: usize = 8192;

/// Outcome of attempting to resolve non-zero contours into even-odd-equivalent
/// contours.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WindingResolution {
    /// Original contours are already equivalent under even-odd; reuse them
    /// directly without cloning.
    Unchanged,
    /// Rewritten simple contours whose even-odd fill matches the original
    /// non-zero fill.
    Resolved(Vec<Subpath>),
    /// Input exceeded planar complexity budgets; caller should keep original
    /// contours and emit a degradation warning.
    BudgetExceeded,
    /// Boundary extraction failed to stitch closed loops; caller should keep
    /// original contours and emit a degradation warning.
    ExtractionFailed,
}

#[derive(Debug, Clone, Copy)]
struct ScaleTol {
    fast_tol: f64,
    fine_tol: f64,
    quant_scale: f64,
    min_seg_len2: f64,
    eps: f64,
    collinear_eps: f64,
    area_eps: f64,
    probe_min: f64,
    probe_max: f64,
}

impl ScaleTol {
    fn from_subpaths(subpaths: &[Subpath]) -> Self {
        let extent = subpaths_extent(subpaths);
        let min_seg = (extent * 1e-6).clamp(1e-9, 1e-5);
        Self {
            fast_tol: (extent * 2e-3).clamp(1e-5, 0.25),
            fine_tol: (extent * 5e-4).clamp(1e-6, 0.05),
            quant_scale: (1e6 / extent).clamp(1e4, 1e8),
            min_seg_len2: min_seg * min_seg,
            eps: (extent * 1e-6).clamp(1e-9, 1e-5),
            collinear_eps: (extent * 1e-5).clamp(1e-8, 1e-4),
            area_eps: (extent * extent * 1e-9).clamp(1e-12, 1e-8),
            probe_min: (extent * 1e-6).clamp(1e-9, 1e-5),
            probe_max: (extent * 5e-4).clamp(1e-6, 5e-4),
        }
    }
}

#[derive(Debug, Clone)]
struct Ring {
    points: Vec<Point>,
    bbox: Rect,
    signed_area: f64,
    /// True if this ring came from a purely linear subpath (so fine flattening
    /// produces the exact same vertices and can be reused without re-flattening).
    is_linear: bool,
}

#[derive(Debug, Clone, Copy)]
struct RawSeg {
    a: Point,
    b: Point,
    bbox: Rect,
}

#[derive(Debug, Clone, Copy)]
struct DirectedEdge {
    u: VKey,
    v: VKey,
    a: Point,
    b: Point,
}

enum ExtractError {
    BudgetExceeded,
    Failed,
}

/// Resolve a slice of contours under the non-zero fill rule into simple contours
/// whose even-odd fill matches the original non-zero fill.
pub(crate) fn resolve_nonzero_subpaths(subpaths: &[Subpath]) -> WindingResolution {
    let mut closed_count = 0usize;
    let mut single_closed_idx = 0usize;
    for (idx, sp) in subpaths.iter().enumerate() {
        if sp.closed && !sp.segments.is_empty() {
            closed_count += 1;
            single_closed_idx = idx;
        }
    }
    if closed_count == 0 {
        return WindingResolution::Unchanged;
    }

    // O(1) zero-allocation fast path for a single closed linear triangle or quad
    // (covers rectangles, table rules, bars, diamonds, and simple SVG polygons).
    if closed_count == 1 {
        let sp = &subpaths[single_closed_idx];
        if sp.segments.len() <= 4
            && sp.segments.iter().all(|s| matches!(s, Segment::Line(_)))
        {
            if let Some(simple) = check_small_linear_polygon_simple(sp) {
                if simple {
                    return WindingResolution::Unchanged;
                }
            }
        }
    }

    let tol = ScaleTol::from_subpaths(subpaths);
    let Some((check_rings, has_cubics)) =
        flatten_subpaths(subpaths, tol.fast_tol, tol, MAX_WINDING_CHECK_VERTICES)
    else {
        return WindingResolution::BudgetExceeded;
    };

    if is_evenodd_equivalent(&check_rings, tol) {
        return WindingResolution::Unchanged;
    }

    let open_tail: Vec<Subpath> = subpaths
        .iter()
        .filter(|sp| !sp.closed && !sp.segments.is_empty())
        .cloned()
        .collect();

    // Reuse `check_rings` directly when no cubic segments exist; otherwise only
    // re-flatten subpaths containing cubics.
    let fine_rings = if !has_cubics {
        check_rings
    } else {
        let Some(rings) = refine_rings(subpaths, check_rings, tol, MAX_WINDING_CHECK_VERTICES)
        else {
            return WindingResolution::BudgetExceeded;
        };
        rings
    };

    match extract_nonzero_boundary(&fine_rings, tol) {
        Ok(mut resolved) => {
            if resolved.is_empty() {
                return WindingResolution::ExtractionFailed;
            }
            resolved.extend(open_tail);
            WindingResolution::Resolved(resolved)
        }
        Err(ExtractError::BudgetExceeded) => WindingResolution::BudgetExceeded,
        Err(ExtractError::Failed) => WindingResolution::ExtractionFailed,
    }
}

/// Fast O(1) check for a single closed linear subpath with <= 4 segments.
/// Returns `Some(true)` if guaranteed simple (no self-intersection),
/// `Some(false)` if it self-intersects (bowtie quad), or `None` to fall back.
fn check_small_linear_polygon_simple(sp: &Subpath) -> Option<bool> {
    let mut pts = [Point::ZERO; 5];
    let mut len = 0usize;
    let start = Point::new(sp.start[0], sp.start[1]);
    if !start.x.is_finite() || !start.y.is_finite() {
        return None;
    }
    pts[len] = start;
    len += 1;

    for seg in &sp.segments {
        let Segment::Line(p) = *seg else {
            return None;
        };
        let pt = Point::new(p[0], p[1]);
        if !pt.x.is_finite() || !pt.y.is_finite() {
            return None;
        }
        if pts[len - 1].distance_squared(pt) > 1e-18 {
            if len >= pts.len() {
                return None;
            }
            pts[len] = pt;
            len += 1;
        }
    }
    if len >= 2 && pts[0].distance_squared(pts[len - 1]) <= 1e-18 {
        len -= 1;
    }
    if len <= 3 {
        // Any degenerate or triangular contour cannot self-intersect.
        return Some(true);
    }
    if len == 4 {
        let tol = ScaleTol::from_subpaths(std::slice::from_ref(sp));
        // A quad can only self-intersect on its two opposite edge pairs.
        let p0 = pts[0];
        let p1 = pts[1];
        let p2 = pts[2];
        let p3 = pts[3];
        let opp1 = bboxes_overlap(seg_bbox(p0, p1), seg_bbox(p2, p3), tol.eps)
            && segments_intersect(p0, p1, p2, p3, tol);
        if opp1 {
            return Some(false);
        }
        let opp2 = bboxes_overlap(seg_bbox(p1, p2), seg_bbox(p3, p0), tol.eps)
            && segments_intersect(p1, p2, p3, p0, tol);
        return Some(!opp2);
    }
    None
}

fn subpaths_extent(subpaths: &[Subpath]) -> f64 {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    let mut extend = |p: [f64; 2]| {
        if p[0].is_finite() && p[1].is_finite() {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
    };
    for sp in subpaths {
        extend(sp.start);
        for seg in &sp.segments {
            match *seg {
                Segment::Line(p) => extend(p),
                Segment::Cubic(c1, c2, p) => {
                    extend(c1);
                    extend(c2);
                    extend(p);
                }
            }
        }
    }
    if x0.is_finite() && y0.is_finite() {
        (x1 - x0).max(y1 - y0).max(1e-6)
    } else {
        1.0
    }
}

fn subpath_has_cubics(sp: &Subpath) -> bool {
    sp.segments.iter().any(|s| matches!(s, Segment::Cubic(..)))
}

fn subpath_to_bezpath(sp: &Subpath) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(Point::new(sp.start[0], sp.start[1]));
    for seg in &sp.segments {
        match *seg {
            Segment::Line(p) => path.line_to(Point::new(p[0], p[1])),
            Segment::Cubic(c1, c2, p) => path.curve_to(
                Point::new(c1[0], c1[1]),
                Point::new(c2[0], c2[1]),
                Point::new(p[0], p[1]),
            ),
        }
    }
    if sp.closed {
        path.close_path();
    }
    path
}

fn flatten_linear_subpath(
    sp: &Subpath,
    rings: &mut Vec<Ring>,
    tol: ScaleTol,
    vertex_budget: usize,
    total_vertices: &mut usize,
) -> bool {
    let mut pts = Vec::with_capacity(sp.segments.len() + 1);
    let start = Point::new(sp.start[0], sp.start[1]);
    pts.push(start);
    for seg in &sp.segments {
        if let Segment::Line(p) = *seg {
            let pt = Point::new(p[0], p[1]);
            if pts
                .last()
                .is_none_or(|prev| prev.distance_squared(pt) > tol.min_seg_len2)
            {
                pts.push(pt);
                if *total_vertices + pts.len() > vertex_budget {
                    return false;
                }
            }
        }
    }
    let before = rings.len();
    flush_ring(&mut pts, rings, tol, true);
    if rings.len() > before {
        *total_vertices += rings[before..].iter().map(|r| r.points.len()).sum::<usize>();
    }
    *total_vertices <= vertex_budget
}

fn flatten_cubic_subpath(
    sp: &Subpath,
    tolerance: f64,
    rings: &mut Vec<Ring>,
    tol: ScaleTol,
    vertex_budget: usize,
    total_vertices: &mut usize,
) -> bool {
    let bez = subpath_to_bezpath(sp);
    let mut pts: Vec<Point> = Vec::new();
    let mut start = Point::ZERO;
    let mut exceeded = false;
    let before = rings.len();

    kurbo::flatten(&bez, tolerance, |el| {
        if exceeded {
            return;
        }
        match el {
            PathEl::MoveTo(p) => {
                flush_ring(&mut pts, rings, tol, false);
                start = p;
                pts.push(p);
            }
            PathEl::LineTo(p) => {
                if pts
                    .last()
                    .is_none_or(|prev| prev.distance_squared(p) > tol.min_seg_len2)
                {
                    pts.push(p);
                    if *total_vertices + pts.len() > vertex_budget {
                        exceeded = true;
                    }
                }
            }
            PathEl::ClosePath => {
                if pts
                    .first()
                    .is_some_and(|first| first.distance_squared(start) <= tol.min_seg_len2)
                {
                    flush_ring(&mut pts, rings, tol, false);
                }
            }
            PathEl::QuadTo(_, _) | PathEl::CurveTo(_, _, _) => {}
        }
    });
    if exceeded {
        return false;
    }
    flush_ring(&mut pts, rings, tol, false);
    if rings.len() > before {
        *total_vertices += rings[before..].iter().map(|r| r.points.len()).sum::<usize>();
    }
    *total_vertices <= vertex_budget
}

/// Flatten closed subpaths into rings, returning `None` if `vertex_budget` is
/// exceeded. Also returns whether any closed subpath contained cubic segments.
fn flatten_subpaths(
    subpaths: &[Subpath],
    tolerance: f64,
    tol: ScaleTol,
    vertex_budget: usize,
) -> Option<(Vec<Ring>, bool)> {
    let mut rings = Vec::with_capacity(subpaths.len());
    let mut total_vertices = 0usize;
    let mut has_cubics = false;

    for sp in subpaths {
        if !sp.closed || sp.segments.is_empty() {
            continue;
        }
        if subpath_has_cubics(sp) {
            has_cubics = true;
            if !flatten_cubic_subpath(
                sp,
                tolerance,
                &mut rings,
                tol,
                vertex_budget,
                &mut total_vertices,
            ) {
                return None;
            }
        } else if !flatten_linear_subpath(sp, &mut rings, tol, vertex_budget, &mut total_vertices) {
            return None;
        }
    }
    Some((rings, has_cubics))
}

/// Produce fine-tolerance rings while reusing already-exact linear rings from
/// the fast check.
fn refine_rings(
    subpaths: &[Subpath],
    fast_rings: Vec<Ring>,
    tol: ScaleTol,
    vertex_budget: usize,
) -> Option<Vec<Ring>> {
    let mut linear_iter = fast_rings.into_iter().filter(|r| r.is_linear);
    let mut rings = Vec::with_capacity(subpaths.len());
    let mut total_vertices = 0usize;

    for sp in subpaths {
        if !sp.closed || sp.segments.is_empty() {
            continue;
        }
        if subpath_has_cubics(sp) {
            if !flatten_cubic_subpath(
                sp,
                tol.fine_tol,
                &mut rings,
                tol,
                vertex_budget,
                &mut total_vertices,
            ) {
                return None;
            }
        } else if let Some(ring) = linear_iter.next() {
            total_vertices += ring.points.len();
            if total_vertices > vertex_budget {
                return None;
            }
            rings.push(ring);
        } else if !flatten_linear_subpath(sp, &mut rings, tol, vertex_budget, &mut total_vertices) {
            return None;
        }
    }
    Some(rings)
}

fn flush_ring(pts: &mut Vec<Point>, rings: &mut Vec<Ring>, tol: ScaleTol, is_linear: bool) {
    if pts.len() >= 2
        && pts.first().unwrap().distance_squared(*pts.last().unwrap()) <= tol.min_seg_len2
    {
        pts.pop();
    }
    if pts.len() < 3 {
        pts.clear();
        return;
    }

    let n = pts.len();
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    let mut area2 = 0.0;

    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        x0 = x0.min(a.x);
        y0 = y0.min(a.y);
        x1 = x1.max(a.x);
        y1 = y1.max(a.y);
        area2 += a.x * b.y - b.x * a.y;
    }

    let bbox = Rect::new(x0, y0, x1, y1);
    // Only discard truly degenerate rings (zero-width/zero-height slivers);
    // keep non-degenerate self-intersecting rings (e.g. symmetric figure-8 /
    // bowtie) whose positive and negative lobes cancel in net signed area.
    if (0.5 * area2).abs() <= tol.area_eps
        && (bbox.width() <= tol.eps || bbox.height() <= tol.eps)
    {
        pts.clear();
        return;
    }

    rings.push(Ring {
        points: std::mem::take(pts),
        bbox,
        signed_area: 0.5 * area2,
        is_linear,
    });
}

#[inline]
fn seg_bbox(a: Point, b: Point) -> Rect {
    Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

#[inline]
fn bboxes_overlap(a: Rect, b: Rect, pad: f64) -> bool {
    a.x0 <= b.x1 + pad && a.x1 + pad >= b.x0 && a.y0 <= b.y1 + pad && a.y1 + pad >= b.y0
}

fn ring_self_intersects(ring: &Ring, tol: ScaleTol) -> bool {
    let n = ring.points.len();
    if n < 4 {
        return false;
    }
    if n <= 64 {
        for i in 0..n {
            let a = ring.points[i];
            let b = ring.points[(i + 1) % n];
            let ab = seg_bbox(a, b);
            for j in (i + 2)..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                let c = ring.points[j];
                let d = ring.points[(j + 1) % n];
                if bboxes_overlap(ab, seg_bbox(c, d), tol.eps)
                    && segments_intersect(a, b, c, d, tol)
                {
                    return true;
                }
            }
        }
        return false;
    }

    // Sweep-line X-sorted pruning: O(n log n + k) instead of O(n^2).
    let mut segs: Vec<(Rect, usize)> = (0..n)
        .map(|i| {
            let a = ring.points[i];
            let b = ring.points[(i + 1) % n];
            (seg_bbox(a, b), i)
        })
        .collect();
    segs.sort_unstable_by(|x, y| x.0.x0.total_cmp(&y.0.x0));

    for p in 0..n {
        let (ab, i) = segs[p];
        let a = ring.points[i];
        let b = ring.points[(i + 1) % n];
        let x_limit = ab.x1 + tol.eps;
        for q in (p + 1)..n {
            let (cd, j) = segs[q];
            if cd.x0 > x_limit {
                break;
            }
            // Skip adjacent edges sharing a vertex.
            let diff = i.abs_diff(j);
            if diff <= 1 || diff == n - 1 {
                continue;
            }
            if ab.y0 <= cd.y1 + tol.eps && ab.y1 + tol.eps >= cd.y0 {
                let c = ring.points[j];
                let d = ring.points[(j + 1) % n];
                if segments_intersect(a, b, c, d, tol) {
                    return true;
                }
            }
        }
    }
    false
}

/// Fast structural check: true when no ring self-intersects, no two rings
/// intersect, and no ring is nested strictly inside a same-orientation ring.
fn is_evenodd_equivalent(rings: &[Ring], tol: ScaleTol) -> bool {
    for ring in rings {
        if ring_self_intersects(ring, tol) {
            return false;
        }
    }

    if rings.len() < 2 {
        return true;
    }

    // Sort ring indices by bbox.x0 for sweep-line pairwise pruning.
    let mut order: Vec<usize> = (0..rings.len()).collect();
    order.sort_unstable_by(|&a, &b| rings[a].bbox.x0.total_cmp(&rings[b].bbox.x0));

    for p in 0..order.len() {
        let ri = &rings[order[p]];
        let x_limit = ri.bbox.x1 + tol.eps;
        for &idx_j in &order[(p + 1)..] {
            let rj = &rings[idx_j];
            if rj.bbox.x0 > x_limit {
                break;
            }
            if ri.bbox.y0 > rj.bbox.y1 + tol.eps || ri.bbox.y1 + tol.eps < rj.bbox.y0 {
                continue;
            }
            if rings_intersect(ri, rj, tol) {
                return false;
            }
            if ri.signed_area * rj.signed_area > 0.0 {
                let i_in_j =
                    rj.bbox.contains(ri.points[0]) && ring_winding_number(rj, ri.points[0]) != 0;
                let j_in_i =
                    ri.bbox.contains(rj.points[0]) && ring_winding_number(ri, rj.points[0]) != 0;
                if i_in_j || j_in_i {
                    return false;
                }
            }
        }
    }

    true
}

fn rings_intersect(ri: &Ring, rj: &Ring, tol: ScaleTol) -> bool {
    let ni = ri.points.len();
    let nj = rj.points.len();

    // Common overlap box of both rings; only segments touching this box can intersect.
    let overlap = Rect::new(
        ri.bbox.x0.max(rj.bbox.x0) - tol.eps,
        ri.bbox.y0.max(rj.bbox.y0) - tol.eps,
        ri.bbox.x1.min(rj.bbox.x1) + tol.eps,
        ri.bbox.y1.min(rj.bbox.y1) + tol.eps,
    );

    let mut segs_i: Vec<(Point, Point, Rect)> = Vec::new();
    for i in 0..ni {
        let a = ri.points[i];
        let b = ri.points[(i + 1) % ni];
        let ab = seg_bbox(a, b);
        if bboxes_overlap(ab, overlap, 0.0) {
            segs_i.push((a, b, ab));
        }
    }
    if segs_i.is_empty() {
        return false;
    }

    let mut segs_j: Vec<(Point, Point, Rect)> = Vec::new();
    for j in 0..nj {
        let c = rj.points[j];
        let d = rj.points[(j + 1) % nj];
        let cd = seg_bbox(c, d);
        if bboxes_overlap(cd, overlap, 0.0) {
            segs_j.push((c, d, cd));
        }
    }
    if segs_j.is_empty() {
        return false;
    }

    if segs_i.len() * segs_j.len() <= 144 {
        for &(a, b, ab) in &segs_i {
            for &(c, d, cd) in &segs_j {
                if bboxes_overlap(ab, cd, tol.eps) && segments_intersect(a, b, c, d, tol) {
                    return true;
                }
            }
        }
        return false;
    }

    // Sweep-line on x0 across the two filtered segment sets.
    segs_i.sort_unstable_by(|x, y| x.2.x0.total_cmp(&y.2.x0));
    segs_j.sort_unstable_by(|x, y| x.2.x0.total_cmp(&y.2.x0));

    let mut j_start = 0usize;
    for &(a, b, ab) in &segs_i {
        while j_start < segs_j.len() && segs_j[j_start].2.x1 + tol.eps < ab.x0 {
            j_start += 1;
        }
        let x_limit = ab.x1 + tol.eps;
        for &(c, d, cd) in &segs_j[j_start..] {
            if cd.x0 > x_limit {
                break;
            }
            if ab.y0 <= cd.y1 + tol.eps
                && ab.y1 + tol.eps >= cd.y0
                && segments_intersect(a, b, c, d, tol)
            {
                return true;
            }
        }
    }
    false
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point, tol: ScaleTol) -> bool {
    let ab = b - a;
    let cd = d - c;
    let ac = c - a;
    let denom = ab.cross(cd);
    let scale = ab.hypot() * cd.hypot();

    if denom.abs() > 1e-9 * scale.max(tol.min_seg_len2) {
        let t = ac.cross(cd) / denom;
        let u = ac.cross(ab) / denom;
        (-1e-6..=1.0 + 1e-6).contains(&t) && (-1e-6..=1.0 + 1e-6).contains(&u)
    } else {
        let ab_len = ab.hypot().max(tol.eps);
        let dist = ac.cross(ab).abs() / ab_len;
        if dist > tol.collinear_eps {
            return false;
        }
        let t0 = ac.dot(ab) / (ab_len * ab_len);
        let t1 = (d - a).dot(ab) / (ab_len * ab_len);
        let lo = t0.min(t1);
        let hi = t0.max(t1);
        hi >= -1e-6 && lo <= 1.0 + 1e-6
    }
}

/// Compute mutual interior split parameters for segment `ab` and segment `cd`
/// in one geometric pass, appending valid interior parameters to `out_t` and
/// `out_u`.
fn segment_split_pair(
    a: Point,
    b: Point,
    c: Point,
    d: Point,
    tol: ScaleTol,
    out_t: &mut Vec<f64>,
    out_u: &mut Vec<f64>,
) {
    let ab = b - a;
    let cd = d - c;
    let ac = c - a;
    let ab_len2 = ab.hypot2();
    let cd_len2 = cd.hypot2();
    if ab_len2 <= tol.min_seg_len2 || cd_len2 <= tol.min_seg_len2 {
        return;
    }
    let scale = (ab_len2 * cd_len2).sqrt();
    let denom = ab.cross(cd);

    if denom.abs() > 1e-9 * scale {
        let t = ac.cross(cd) / denom;
        let u = ac.cross(ab) / denom;
        if (-1e-6..=1.0 + 1e-6).contains(&u) && t > 1e-6 && t < 1.0 - 1e-6 {
            out_t.push(t);
        }
        if (-1e-6..=1.0 + 1e-6).contains(&t) && u > 1e-6 && u < 1.0 - 1e-6 {
            out_u.push(u);
        }
    } else {
        let ab_len = ab_len2.sqrt();
        if ac.cross(ab).abs() / ab_len <= tol.collinear_eps {
            for p in [c, d] {
                let t = (p - a).dot(ab) / ab_len2;
                if t > 1e-6 && t < 1.0 - 1e-6 {
                    out_t.push(t);
                }
            }
            for p in [a, b] {
                let u = (p - c).dot(cd) / cd_len2;
                if u > 1e-6 && u < 1.0 - 1e-6 {
                    out_u.push(u);
                }
            }
        }
    }
}

fn ring_winding_number(ring: &Ring, p: Point) -> i32 {
    let pts = &ring.points;
    let n = pts.len();
    let mut wn = 0_i32;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        if a.y <= p.y {
            if b.y > p.y && (b - a).cross(p - a) > 0.0 {
                wn += 1;
            }
        } else if b.y <= p.y && (b - a).cross(p - a) < 0.0 {
            wn -= 1;
        }
    }
    wn
}

/// Spatial index over rings sorted by `bbox.x0`, allowing early termination
/// once `ring.bbox.x0 > p.x`.
struct RingIndex<'a> {
    rings: Vec<&'a Ring>,
}

impl<'a> RingIndex<'a> {
    fn new(rings: &'a [Ring]) -> Self {
        let mut sorted: Vec<&Ring> = rings.iter().collect();
        sorted.sort_unstable_by(|a, b| a.bbox.x0.total_cmp(&b.bbox.x0));
        Self { rings: sorted }
    }

    fn total_winding_number(&self, p: Point) -> i32 {
        let mut wn = 0_i32;
        for &ring in &self.rings {
            if ring.bbox.x0 > p.x {
                break;
            }
            if ring.bbox.x1 >= p.x && ring.bbox.y0 <= p.y && ring.bbox.y1 >= p.y {
                wn += ring_winding_number(ring, p);
            }
        }
        wn
    }
}

fn vkey(p: Point, quant_scale: f64) -> VKey {
    (
        (p.x * quant_scale).round() as i64,
        (p.y * quant_scale).round() as i64,
    )
}

fn undirected_edge_key(u: VKey, v: VKey) -> (VKey, VKey) {
    if u <= v {
        (u, v)
    } else {
        (v, u)
    }
}

fn extract_nonzero_boundary(rings: &[Ring], tol: ScaleTol) -> Result<Vec<Subpath>, ExtractError> {
    if rings.is_empty() {
        return Err(ExtractError::Failed);
    }

    let mut raw_segs: Vec<RawSeg> = Vec::new();
    for ring in rings {
        let n = ring.points.len();
        for i in 0..n {
            let a = ring.points[i];
            let b = ring.points[(i + 1) % n];
            if a.distance_squared(b) > tol.min_seg_len2 {
                raw_segs.push(RawSeg {
                    a,
                    b,
                    bbox: seg_bbox(a, b),
                });
                if raw_segs.len() > MAX_WINDING_RESOLVE_EDGES {
                    return Err(ExtractError::BudgetExceeded);
                }
            }
        }
    }

    if raw_segs.is_empty() {
        return Err(ExtractError::Failed);
    }

    // Sort raw segments by x0 and sweep-prune pair checks in a single pass
    // (splitting both segment i and segment j together).
    raw_segs.sort_unstable_by(|x, y| x.bbox.x0.total_cmp(&y.bbox.x0));

    let seg_count = raw_segs.len();
    let mut all_splits: Vec<Vec<f64>> = vec![vec![0.0, 1.0]; seg_count];
    let mut estimated_subsegs = seg_count;

    for i in 0..seg_count {
        let si = raw_segs[i];
        let x_limit = si.bbox.x1 + tol.eps;
        let (left, right) = all_splits.split_at_mut(i + 1);
        let splits_i = &mut left[i];

        for offset in 0..right.len() {
            let j = i + 1 + offset;
            let sj = raw_segs[j];
            if sj.bbox.x0 > x_limit {
                break;
            }
            if si.bbox.y0 > sj.bbox.y1 + tol.eps || si.bbox.y1 + tol.eps < sj.bbox.y0 {
                continue;
            }
            let before = splits_i.len() + right[offset].len();
            segment_split_pair(
                si.a,
                si.b,
                sj.a,
                sj.b,
                tol,
                splits_i,
                &mut right[offset],
            );
            let added = (splits_i.len() + right[offset].len()) - before;
            estimated_subsegs += added;
            if estimated_subsegs > MAX_WINDING_SPLIT_EDGES {
                return Err(ExtractError::BudgetExceeded);
            }
        }
    }

    let ring_index = RingIndex::new(rings);
    let mut canonical_pts: HashMap<VKey, Point> = HashMap::new();
    let mut intern = |p: Point| -> (VKey, Point) {
        let k = vkey(p, tol.quant_scale);
        let cp = *canonical_pts.entry(k).or_insert(p);
        (k, cp)
    };

    let mut seen_edges: HashSet<(VKey, VKey)> = HashSet::new();
    let mut kept_edges: Vec<DirectedEdge> = Vec::new();

    for (seg, mut split_params) in raw_segs.into_iter().zip(all_splits.into_iter()) {
        split_params.sort_unstable_by(f64::total_cmp);
        split_params.dedup_by(|x, y| (*x - *y).abs() < 1e-6);

        for win in split_params.windows(2) {
            let p = seg.a.lerp(seg.b, win[0]);
            let q = seg.a.lerp(seg.b, win[1]);
            let (ku, pu) = intern(p);
            let (kv, pv) = intern(q);
            if ku == kv {
                continue;
            }
            if !seen_edges.insert(undirected_edge_key(ku, kv)) {
                continue;
            }

            let dir = pv - pu;
            let len = dir.hypot();
            if len * len <= tol.min_seg_len2 {
                continue;
            }
            let mid = pu.midpoint(pv);
            let normal = Vec2::new(-dir.y / len, dir.x / len);
            let probe = (len * 0.05).clamp(tol.probe_min, tol.probe_max);

            let left_in = ring_index.total_winding_number(mid + normal * probe) != 0;
            let right_in = ring_index.total_winding_number(mid - normal * probe) != 0;

            match (left_in, right_in) {
                (true, false) => kept_edges.push(DirectedEdge {
                    u: ku,
                    v: kv,
                    a: pu,
                    b: pv,
                }),
                (false, true) => kept_edges.push(DirectedEdge {
                    u: kv,
                    v: ku,
                    a: pv,
                    b: pu,
                }),
                _ => {}
            }
        }
    }

    let (loops, had_open_chain) = stitch_boundary_loops(&kept_edges);
    if loops.is_empty() || had_open_chain {
        Err(ExtractError::Failed)
    } else {
        Ok(loops)
    }
}

/// Stitch directed boundary edges into closed [`Subpath`] loops.
///
/// Only genuinely closed walks (`cur_key == loop_start_key`) are emitted; open
/// chains are never forcefully closed with synthetic edges, and `had_open_chain`
/// reports whether any unclosed chain was encountered.
fn stitch_boundary_loops(edges: &[DirectedEdge]) -> (Vec<Subpath>, bool) {
    if edges.is_empty() {
        return (Vec::new(), false);
    }

    let mut outgoing: HashMap<VKey, Vec<usize>> = HashMap::new();
    for (idx, edge) in edges.iter().enumerate() {
        outgoing.entry(edge.u).or_default().push(idx);
    }

    let mut used = vec![false; edges.len()];
    let mut subpaths = Vec::new();
    let mut had_open_chain = false;

    for start_idx in 0..edges.len() {
        if used[start_idx] {
            continue;
        }
        used[start_idx] = true;
        let first = edges[start_idx];
        let loop_start_key = first.u;
        let mut cur_key = first.v;
        let mut prev_dir = first.b - first.a;
        let mut vertices = vec![first.a, first.b];

        for _ in 0..edges.len() {
            if cur_key == loop_start_key {
                break;
            }
            let Some(candidates) = outgoing.get(&cur_key) else {
                break;
            };
            let next_idx = pick_next_edge(edges, candidates, &used, prev_dir, loop_start_key);
            let Some(idx) = next_idx else {
                break;
            };
            used[idx] = true;
            let e = edges[idx];
            prev_dir = e.b - e.a;
            cur_key = e.v;
            if cur_key != loop_start_key {
                vertices.push(e.b);
            }
        }

        if cur_key != loop_start_key {
            had_open_chain = true;
            continue;
        }

        if vertices.len() >= 3 {
            let start = [vertices[0].x, vertices[0].y];
            let mut segments = Vec::with_capacity(vertices.len());
            for pt in &vertices[1..] {
                segments.push(Segment::Line([pt.x, pt.y]));
            }
            segments.push(Segment::Line(start));
            subpaths.push(Subpath {
                start,
                segments,
                closed: true,
            });
        }
    }

    (subpaths, had_open_chain)
}

fn pick_next_edge(
    edges: &[DirectedEdge],
    candidates: &[usize],
    used: &[bool],
    prev_dir: Vec2,
    loop_start_key: VKey,
) -> Option<usize> {
    let mut best: Option<(f64, bool, usize)> = None;
    let back = Vec2::new(-prev_dir.x, -prev_dir.y);
    let back_angle = back.y.atan2(back.x);

    for &idx in candidates {
        if used[idx] {
            continue;
        }
        let e = edges[idx];
        let dir = e.b - e.a;
        let mut delta = dir.y.atan2(dir.x) - back_angle;
        if delta <= 0.0 {
            delta += std::f64::consts::TAU;
        }
        let closes = e.v == loop_start_key;
        let replace = match best {
            None => true,
            Some((best_delta, best_closes, _)) => {
                delta + 1e-5 < best_delta
                    || ((delta - best_delta).abs() <= 1e-5 && closes && !best_closes)
            }
        };
        if replace {
            best = Some((delta, closes, idx));
        }
    }

    best.map(|(_, _, idx)| idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evenodd_winding(subpaths: &[Subpath], pt: [f64; 2]) -> i32 {
        let tol = ScaleTol::from_subpaths(subpaths);
        let (rings, _) = flatten_subpaths(subpaths, tol.fine_tol, tol, usize::MAX).unwrap();
        let idx = RingIndex::new(&rings);
        idx.total_winding_number(Point::new(pt[0], pt[1])).abs() % 2
    }

    fn poly_subpath(pts: &[[f64; 2]]) -> Subpath {
        let mut segments = Vec::with_capacity(pts.len());
        for &p in &pts[1..] {
            segments.push(Segment::Line(p));
        }
        segments.push(Segment::Line(pts[0]));
        Subpath {
            start: pts[0],
            segments,
            closed: true,
        }
    }

    #[test]
    fn star_five_pointed_nonzero_fills_center() {
        let star = poly_subpath(&[
            [15.0, 0.0],
            [25.0, 30.0],
            [0.0, 12.0],
            [30.0, 12.0],
            [5.0, 30.0],
        ]);
        let WindingResolution::Resolved(resolved) = resolve_nonzero_subpaths(&[star]) else {
            panic!("expected star to resolve");
        };
        assert!(!resolved.is_empty());
        assert_eq!(evenodd_winding(&resolved, [15.0, 17.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [1.0, 1.0]), 0);
    }

    #[test]
    fn overlapping_same_sign_squares_merge_without_hole() {
        let sq1 = poly_subpath(&[[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]);
        let sq2 = poly_subpath(&[[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]]);
        let WindingResolution::Resolved(resolved) = resolve_nonzero_subpaths(&[sq1, sq2]) else {
            panic!("expected overlapping squares to resolve");
        };
        assert_eq!(evenodd_winding(&resolved, [15.0, 15.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [5.0, 5.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [25.0, 25.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [25.0, 5.0]), 0);
    }

    #[test]
    fn nested_same_sign_squares_drop_inner_hole() {
        let outer = poly_subpath(&[[0.0, 0.0], [30.0, 0.0], [30.0, 30.0], [0.0, 30.0]]);
        let inner = poly_subpath(&[[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]]);
        let WindingResolution::Resolved(resolved) = resolve_nonzero_subpaths(&[outer, inner])
        else {
            panic!("expected nested same-sign squares to resolve");
        };
        assert_eq!(evenodd_winding(&resolved, [15.0, 15.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [5.0, 5.0]), 1);
    }

    #[test]
    fn outer_with_opposite_sign_hole_hits_zero_copy_fast_path() {
        let outer = Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Cubic([10.0, -5.0], [20.0, -5.0], [30.0, 0.0]),
                Segment::Line([30.0, 30.0]),
                Segment::Line([0.0, 30.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        };
        let hole = poly_subpath(&[[10.0, 10.0], [10.0, 20.0], [20.0, 20.0], [20.0, 10.0]]);
        assert_eq!(
            resolve_nonzero_subpaths(&[outer, hole]),
            WindingResolution::Unchanged
        );
    }

    #[test]
    fn unit_em_curved_glyph_hits_fast_path_without_cloning() {
        let outer = Subpath {
            start: [0.4, 0.0],
            segments: vec![
                Segment::Cubic([0.7, 0.0], [0.8, 0.25], [0.8, 0.5]),
                Segment::Cubic([0.8, 0.75], [0.7, 1.0], [0.4, 1.0]),
                Segment::Cubic([0.1, 1.0], [0.0, 0.75], [0.0, 0.5]),
                Segment::Cubic([0.0, 0.25], [0.1, 0.0], [0.4, 0.0]),
            ],
            closed: true,
        };
        let hole = Subpath {
            start: [0.4, 0.2],
            segments: vec![
                Segment::Cubic([0.22, 0.2], [0.18, 0.35], [0.18, 0.5]),
                Segment::Cubic([0.18, 0.65], [0.22, 0.8], [0.4, 0.8]),
                Segment::Cubic([0.58, 0.8], [0.62, 0.65], [0.62, 0.5]),
                Segment::Cubic([0.62, 0.35], [0.58, 0.2], [0.4, 0.2]),
            ],
            closed: true,
        };
        assert_eq!(
            resolve_nonzero_subpaths(&[outer, hole]),
            WindingResolution::Unchanged
        );
    }

    #[test]
    fn simple_and_bowtie_quads_handled_accurately() {
        let rect = poly_subpath(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        assert_eq!(
            resolve_nonzero_subpaths(&[rect]),
            WindingResolution::Unchanged
        );

        let bowtie = poly_subpath(&[[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]]);
        assert!(matches!(
            resolve_nonzero_subpaths(&[bowtie]),
            WindingResolution::Resolved(_)
        ));
    }

    #[test]
    fn open_edge_chain_is_not_force_closed() {
        // Three edges forming an open polyline (missing the closing edge back to (0, 0)).
        let edges = vec![
            DirectedEdge {
                u: (0, 0),
                v: (10, 0),
                a: Point::new(0.0, 0.0),
                b: Point::new(10.0, 0.0),
            },
            DirectedEdge {
                u: (10, 0),
                v: (10, 10),
                a: Point::new(10.0, 0.0),
                b: Point::new(10.0, 10.0),
            },
            DirectedEdge {
                u: (10, 10),
                v: (0, 10),
                a: Point::new(10.0, 10.0),
                b: Point::new(0.0, 10.0),
            },
        ];
        let (loops, had_open) = stitch_boundary_loops(&edges);
        assert!(loops.is_empty(), "open chain must not be force-closed");
        assert!(had_open);
    }

    #[test]
    fn exceeds_vertex_budget_returns_budget_exceeded() {
        // Build a 5000-vertex polygon (> MAX_WINDING_CHECK_VERTICES).
        let n = MAX_WINDING_CHECK_VERTICES + 500;
        let pts: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let angle = std::f64::consts::TAU * (i as f64) / (n as f64);
                [100.0 + 50.0 * angle.cos(), 100.0 + 50.0 * angle.sin()]
            })
            .collect();
        let huge = poly_subpath(&pts);
        assert_eq!(
            resolve_nonzero_subpaths(&[huge]),
            WindingResolution::BudgetExceeded
        );
    }

    #[test]
    fn exceeds_edge_budget_in_slow_path_returns_budget_exceeded() {
        // Build a 1200-vertex self-intersecting star (> MAX_WINDING_RESOLVE_EDGES
        // but < MAX_WINDING_CHECK_VERTICES).
        let n = MAX_WINDING_RESOLVE_EDGES + 101;
        let step = 3; // gcd(3, 1125)? ensure coprime
        let n = if n % 3 == 0 { n + 1 } else { n };
        let pts: Vec<[f64; 2]> = (0..n)
            .map(|k| {
                let v = (k * step) % n;
                let angle = std::f64::consts::TAU * (v as f64) / (n as f64);
                [100.0 + 50.0 * angle.cos(), 100.0 + 50.0 * angle.sin()]
            })
            .collect();
        let dense = poly_subpath(&pts);
        assert_eq!(
            resolve_nonzero_subpaths(&[dense]),
            WindingResolution::BudgetExceeded
        );
    }
}
