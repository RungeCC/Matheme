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

mod boundary;
mod flatten;
mod intersect;

use kurbo::{Point, Rect};

use crate::ir::{Segment, Subpath};
use boundary::{extract_nonzero_boundary, ExtractError};
use flatten::{flatten_subpaths, refine_rings};
use intersect::{bboxes_overlap, is_evenodd_equivalent, seg_bbox, segments_intersect};

/// Maximum flattened vertices permitted during the fast equivalence check.
pub(crate) const MAX_WINDING_CHECK_VERTICES: usize = 4096;

/// Maximum raw segments permitted in the slow-path planar boundary extractor.
pub(crate) const MAX_WINDING_RESOLVE_EDGES: usize = 1024;

/// Maximum split sub-segments permitted during slow-path boundary extraction
/// (guards against dense all-pairs crossing stars/grids with large `K`).
pub(crate) const MAX_WINDING_SPLIT_EDGES: usize = 8192;

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
pub(super) struct ScaleTol {
    pub(super) fast_tol: f64,
    pub(super) fine_tol: f64,
    pub(super) quant_scale: f64,
    pub(super) min_seg_len2: f64,
    pub(super) eps: f64,
    pub(super) collinear_eps: f64,
    pub(super) area_eps: f64,
    pub(super) probe_min: f64,
    pub(super) probe_max: f64,
}

impl ScaleTol {
    pub(super) fn from_subpaths(subpaths: &[Subpath]) -> Self {
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
pub(super) struct Ring {
    pub(super) points: Vec<Point>,
    pub(super) bbox: Rect,
    pub(super) signed_area: f64,
    /// True if this ring came from a purely linear subpath (so fine flattening
    /// produces the exact same vertices and can be reused without re-flattening).
    pub(super) is_linear: bool,
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

    // O(1) zero-allocation fast path for a single closed linear triangle or quad.
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
        return Some(true);
    }
    if len == 4 {
        let tol = ScaleTol::from_subpaths(std::slice::from_ref(sp));
        let [p0, p1, p2, p3, _] = pts;
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

#[cfg(test)]
mod tests {
    use super::*;
    use boundary::{stitch_boundary_loops, DirectedEdge};
    use intersect::RingIndex;

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
        let n = MAX_WINDING_RESOLVE_EDGES + 101;
        let step = 3;
        let n = if n.is_multiple_of(3) { n + 1 } else { n };
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
