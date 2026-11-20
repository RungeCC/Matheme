//! Planar non-zero to even-odd winding resolution for filled vector contours.
//!
//! Wolfram Language's `FilledCurve` renders multiple contours under the even-odd
//! rule, whereas Typst shapes and variable fonts often assume the non-zero
//! winding rule. For ordinary contours (no self-intersections, no crossings, and
//! no same-sign nested boundaries), non-zero and even-odd produce identical
//! fills, so a fast structural check leaves exact cubic Béziers untouched. When
//! self-intersections, crossings, or same-sign nestings exist, we extract the
//! exact exterior boundary of the non-zero filled region (`winding != 0`) so
//! Wolfram's even-odd `FilledCurve` renders the intended shape without holes.

use std::collections::{HashMap, HashSet};

use kurbo::{BezPath, PathEl, Point, Rect, Vec2};

use crate::graphics::{Segment, Subpath};

type VKey = (i64, i64);

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
}

#[derive(Debug, Clone, Copy)]
struct DirectedEdge {
    u: VKey,
    v: VKey,
    a: Point,
    b: Point,
}

/// Resolve a slice of contours under the non-zero fill rule into simple contours
/// whose even-odd fill matches the original non-zero fill.
pub(crate) fn resolve_nonzero_subpaths(subpaths: &[Subpath]) -> Vec<Subpath> {
    let closed_count = subpaths
        .iter()
        .filter(|sp| sp.closed && !sp.segments.is_empty())
        .count();
    if closed_count == 0 {
        return subpaths.to_vec();
    }

    let tol = ScaleTol::from_subpaths(subpaths);
    let check_rings = flatten_subpaths(subpaths, tol.fast_tol, tol);
    if is_evenodd_equivalent(&check_rings, tol) {
        return subpaths.to_vec();
    }

    let open_tail: Vec<Subpath> = subpaths
        .iter()
        .filter(|sp| !sp.closed && !sp.segments.is_empty())
        .cloned()
        .collect();

    let fine_rings = flatten_subpaths(subpaths, tol.fine_tol, tol);
    let mut resolved = extract_nonzero_boundary(&fine_rings, tol);
    if resolved.is_empty() && open_tail.is_empty() {
        return subpaths.to_vec();
    }
    resolved.extend(open_tail);
    resolved
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

fn flatten_subpaths(subpaths: &[Subpath], tolerance: f64, tol: ScaleTol) -> Vec<Ring> {
    let mut rings = Vec::new();
    for sp in subpaths {
        if !sp.closed || sp.segments.is_empty() {
            continue;
        }
        let bez = subpath_to_bezpath(sp);
        let mut pts: Vec<Point> = Vec::new();
        let mut start = Point::ZERO;
        kurbo::flatten(&bez, tolerance, |el| match el {
            PathEl::MoveTo(p) => {
                flush_ring(&mut pts, &mut rings, tol);
                start = p;
                pts.push(p);
            }
            PathEl::LineTo(p) => {
                if pts
                    .last()
                    .is_none_or(|prev| prev.distance_squared(p) > tol.min_seg_len2)
                {
                    pts.push(p);
                }
            }
            PathEl::ClosePath => {
                if pts
                    .first()
                    .is_some_and(|first| first.distance_squared(start) <= tol.min_seg_len2)
                {
                    flush_ring(&mut pts, &mut rings, tol);
                }
            }
            PathEl::QuadTo(_, _) | PathEl::CurveTo(_, _, _) => {}
        });
        flush_ring(&mut pts, &mut rings, tol);
    }
    rings
}

fn flush_ring(pts: &mut Vec<Point>, rings: &mut Vec<Ring>, tol: ScaleTol) {
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

    if (0.5 * area2).abs() <= tol.area_eps {
        pts.clear();
        return;
    }

    rings.push(Ring {
        points: std::mem::take(pts),
        bbox: Rect::new(x0, y0, x1, y1),
        signed_area: 0.5 * area2,
    });
}

fn seg_bbox(a: Point, b: Point) -> Rect {
    Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

fn bboxes_overlap(a: Rect, b: Rect, pad: f64) -> bool {
    a.x0 <= b.x1 + pad && a.x1 + pad >= b.x0 && a.y0 <= b.y1 + pad && a.y1 + pad >= b.y0
}

/// Fast structural check: true when no ring self-intersects, no two rings
/// intersect, and no ring is nested strictly inside a same-orientation ring.
fn is_evenodd_equivalent(rings: &[Ring], tol: ScaleTol) -> bool {
    for ring in rings {
        let n = ring.points.len();
        if n < 4 {
            continue;
        }
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
                    return false;
                }
            }
        }
    }

    for i in 0..rings.len() {
        for j in (i + 1)..rings.len() {
            let ri = &rings[i];
            let rj = &rings[j];
            if !bboxes_overlap(ri.bbox, rj.bbox, tol.eps) {
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
    for i in 0..ni {
        let a = ri.points[i];
        let b = ri.points[(i + 1) % ni];
        let ab = seg_bbox(a, b);
        if !bboxes_overlap(ab, rj.bbox, tol.eps) {
            continue;
        }
        for j in 0..nj {
            let c = rj.points[j];
            let d = rj.points[(j + 1) % nj];
            if bboxes_overlap(ab, seg_bbox(c, d), tol.eps) && segments_intersect(a, b, c, d, tol)
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

fn segment_split_params(
    a: Point,
    b: Point,
    c: Point,
    d: Point,
    tol: ScaleTol,
    out_t: &mut Vec<f64>,
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
    } else {
        let ab_len = ab_len2.sqrt();
        if ac.cross(ab).abs() / ab_len <= tol.collinear_eps {
            for p in [c, d] {
                let t = (p - a).dot(ab) / ab_len2;
                if t > 1e-6 && t < 1.0 - 1e-6 {
                    out_t.push(t);
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

fn total_winding_number(rings: &[Ring], p: Point) -> i32 {
    let mut wn = 0_i32;
    for ring in rings {
        if ring.bbox.contains(p) {
            wn += ring_winding_number(ring, p);
        }
    }
    wn
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

fn extract_nonzero_boundary(rings: &[Ring], tol: ScaleTol) -> Vec<Subpath> {
    if rings.is_empty() {
        return Vec::new();
    }

    let mut raw_segs: Vec<(Point, Point, Rect)> = Vec::new();
    for ring in rings {
        let n = ring.points.len();
        for i in 0..n {
            let a = ring.points[i];
            let b = ring.points[(i + 1) % n];
            if a.distance_squared(b) > tol.min_seg_len2 {
                raw_segs.push((a, b, seg_bbox(a, b)));
            }
        }
    }

    let mut canonical_pts: HashMap<VKey, Point> = HashMap::new();
    let mut intern = |p: Point| -> (VKey, Point) {
        let k = vkey(p, tol.quant_scale);
        let cp = *canonical_pts.entry(k).or_insert(p);
        (k, cp)
    };

    let mut seen_edges: HashSet<(VKey, VKey)> = HashSet::new();
    let mut kept_edges: Vec<DirectedEdge> = Vec::new();
    let mut split_params: Vec<f64> = Vec::new();

    for i in 0..raw_segs.len() {
        let (a, b, ab_box) = raw_segs[i];
        split_params.clear();
        split_params.push(0.0);
        split_params.push(1.0);

        for (j, &(c, d, cd_box)) in raw_segs.iter().enumerate() {
            if i == j || !bboxes_overlap(ab_box, cd_box, tol.eps) {
                continue;
            }
            segment_split_params(a, b, c, d, tol, &mut split_params);
        }

        split_params.sort_by(f64::total_cmp);
        split_params.dedup_by(|x, y| (*x - *y).abs() < 1e-6);

        for win in split_params.windows(2) {
            let p = a.lerp(b, win[0]);
            let q = a.lerp(b, win[1]);
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

            let left_in = total_winding_number(rings, mid + normal * probe) != 0;
            let right_in = total_winding_number(rings, mid - normal * probe) != 0;

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

    stitch_boundary_loops(&kept_edges)
}

fn stitch_boundary_loops(edges: &[DirectedEdge]) -> Vec<Subpath> {
    if edges.is_empty() {
        return Vec::new();
    }

    let mut outgoing: HashMap<VKey, Vec<usize>> = HashMap::new();
    for (idx, edge) in edges.iter().enumerate() {
        outgoing.entry(edge.u).or_default().push(idx);
    }

    let mut used = vec![false; edges.len()];
    let mut subpaths = Vec::new();

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

    subpaths
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
        let rings = flatten_subpaths(subpaths, tol.fine_tol, tol);
        let total = total_winding_number(&rings, Point::new(pt[0], pt[1]));
        total.abs() % 2
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
        let resolved = resolve_nonzero_subpaths(&[star]);
        assert!(!resolved.is_empty());
        assert_eq!(evenodd_winding(&resolved, [15.0, 17.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [1.0, 1.0]), 0);
    }

    #[test]
    fn overlapping_same_sign_squares_merge_without_hole() {
        let sq1 = poly_subpath(&[[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]);
        let sq2 = poly_subpath(&[[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]]);
        let resolved = resolve_nonzero_subpaths(&[sq1, sq2]);
        assert_eq!(evenodd_winding(&resolved, [15.0, 15.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [5.0, 5.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [25.0, 25.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [25.0, 5.0]), 0);
    }

    #[test]
    fn nested_same_sign_squares_drop_inner_hole() {
        let outer = poly_subpath(&[[0.0, 0.0], [30.0, 0.0], [30.0, 30.0], [0.0, 30.0]]);
        let inner = poly_subpath(&[[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]]);
        let resolved = resolve_nonzero_subpaths(&[outer, inner]);
        assert_eq!(evenodd_winding(&resolved, [15.0, 15.0]), 1);
        assert_eq!(evenodd_winding(&resolved, [5.0, 5.0]), 1);
    }

    #[test]
    fn outer_with_opposite_sign_hole_preserves_cubics() {
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
        let resolved = resolve_nonzero_subpaths(&[outer, hole]);
        assert_eq!(resolved.len(), 2);
        assert!(
            matches!(resolved[0].segments[0], Segment::Cubic(..)),
            "standard outer+hole should hit fast path and keep cubics"
        );
    }

    #[test]
    fn unit_em_curved_glyph_hits_fast_path_preserving_cubics() {
        // Normalized 1-em rounded "O"-like outer contour + opposite-sign inner hole.
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
        let resolved = resolve_nonzero_subpaths(&[outer, hole]);
        assert_eq!(resolved.len(), 2);
        assert!(
            resolved.iter().all(|sp| sp
                .segments
                .iter()
                .all(|seg| matches!(seg, Segment::Cubic(..)))),
            "normalized unit-em curved glyphs must preserve exact cubic Béziers"
        );
    }
}
