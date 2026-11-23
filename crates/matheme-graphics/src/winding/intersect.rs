//! Sweep-line intersection tests, segment splitting, and winding-number queries.

use kurbo::{Point, Rect};

use super::{Ring, ScaleTol};

#[inline]
pub(super) fn seg_bbox(a: Point, b: Point) -> Rect {
    Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

#[inline]
pub(super) fn bboxes_overlap(a: Rect, b: Rect, pad: f64) -> bool {
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
        for &(cd, j) in &segs[(p + 1)..n] {
            if cd.x0 > x_limit {
                break;
            }
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
pub(super) fn is_evenodd_equivalent(rings: &[Ring], tol: ScaleTol) -> bool {
    for ring in rings {
        if ring_self_intersects(ring, tol) {
            return false;
        }
    }

    if rings.len() < 2 {
        return true;
    }

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

pub(super) fn segments_intersect(a: Point, b: Point, c: Point, d: Point, tol: ScaleTol) -> bool {
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
pub(super) fn segment_split_pair(
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

pub(super) fn ring_winding_number(ring: &Ring, p: Point) -> i32 {
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
pub(super) struct RingIndex<'a> {
    rings: Vec<&'a Ring>,
}

impl<'a> RingIndex<'a> {
    pub(super) fn new(rings: &'a [Ring]) -> Self {
        let mut sorted: Vec<&Ring> = rings.iter().collect();
        sorted.sort_unstable_by(|a, b| a.bbox.x0.total_cmp(&b.bbox.x0));
        Self { rings: sorted }
    }

    pub(super) fn total_winding_number(&self, p: Point) -> i32 {
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
