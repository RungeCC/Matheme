//! Budgeted flattening of closed [`Subpath`] contours into polygonal [`Ring`]s.

use kurbo::{BezPath, PathEl, Point, Rect};

use super::{Ring, ScaleTol};
use crate::ir::{Segment, Subpath};

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
        *total_vertices += rings[before..]
            .iter()
            .map(|r| r.points.len())
            .sum::<usize>();
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
        *total_vertices += rings[before..]
            .iter()
            .map(|r| r.points.len())
            .sum::<usize>();
    }
    *total_vertices <= vertex_budget
}

/// Flatten closed subpaths into rings, returning `None` if `vertex_budget` is
/// exceeded. Also returns whether any closed subpath contained cubic segments.
pub(super) fn flatten_subpaths(
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
pub(super) fn refine_rings(
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
    if (0.5 * area2).abs() <= tol.area_eps && (bbox.width() <= tol.eps || bbox.height() <= tol.eps)
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
