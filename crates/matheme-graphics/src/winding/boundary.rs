//! Planar boundary extraction and closed-loop stitching for non-zero regions.

use std::collections::{HashMap, HashSet};

use kurbo::{Point, Rect, Vec2};

use super::intersect::{RingIndex, seg_bbox, segment_split_pair};
use super::{MAX_WINDING_RESOLVE_EDGES, MAX_WINDING_SPLIT_EDGES, Ring, ScaleTol};
use crate::ir::{Segment, Subpath};

pub(super) type VKey = (i64, i64);

#[derive(Debug, Clone, Copy)]
struct RawSeg {
    a: Point,
    b: Point,
    bbox: Rect,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DirectedEdge {
    pub(super) u: VKey,
    pub(super) v: VKey,
    pub(super) a: Point,
    pub(super) b: Point,
}

pub(super) enum ExtractError {
    BudgetExceeded,
    Failed,
}

fn vkey(p: Point, quant_scale: f64) -> VKey {
    (
        (p.x * quant_scale).round() as i64,
        (p.y * quant_scale).round() as i64,
    )
}

fn undirected_edge_key(u: VKey, v: VKey) -> (VKey, VKey) {
    if u <= v { (u, v) } else { (v, u) }
}

pub(super) fn extract_nonzero_boundary(
    rings: &[Ring],
    tol: ScaleTol,
) -> Result<Vec<Subpath>, ExtractError> {
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

    raw_segs.sort_unstable_by(|x, y| x.bbox.x0.total_cmp(&y.bbox.x0));

    let seg_count = raw_segs.len();
    let mut all_splits: Vec<Vec<f64>> = vec![vec![0.0, 1.0]; seg_count];
    let mut estimated_subsegs = seg_count;

    for i in 0..seg_count {
        let si = raw_segs[i];
        let x_limit = si.bbox.x1 + tol.eps;
        let (left, right) = all_splits.split_at_mut(i + 1);
        let splits_i = &mut left[i];

        for (offset, splits_j) in right.iter_mut().enumerate() {
            let j = i + 1 + offset;
            let sj = raw_segs[j];
            if sj.bbox.x0 > x_limit {
                break;
            }
            if si.bbox.y0 > sj.bbox.y1 + tol.eps || si.bbox.y1 + tol.eps < sj.bbox.y0 {
                continue;
            }
            let before = splits_i.len() + splits_j.len();
            segment_split_pair(si.a, si.b, sj.a, sj.b, tol, splits_i, splits_j);
            let added = (splits_i.len() + splits_j.len()) - before;
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

    for (seg, mut split_params) in raw_segs.into_iter().zip(all_splits) {
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
pub(super) fn stitch_boundary_loops(edges: &[DirectedEdge]) -> (Vec<Subpath>, bool) {
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
