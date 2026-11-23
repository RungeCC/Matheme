//! Lattice translation calculation and paint inspection helpers for 2D vector tilings.

use kurbo::{Affine, Point};

use crate::ir::{PaintStyle, TilingPaint, VectorPrimitive};

/// Returns `true` if any primitive in `primitives` uses a gradient, tiling,
/// nested clip, or embedded image (i.e. anything whose orientation, `Inset`, or
/// texture coordinates should not be nested inside a rotated `Inset`).
pub fn primitives_contain_non_solid_paint(primitives: &[VectorPrimitive]) -> bool {
    primitives.iter().any(|p| match p {
        VectorPrimitive::Paths { fill, .. } => {
            matches!(fill, Some(PaintStyle::Gradient(_) | PaintStyle::Tiling(_)))
        }
        VectorPrimitive::Clip { .. } | VectorPrimitive::Image { .. } => true,
        VectorPrimitive::Glyphs { .. } | VectorPrimitive::Line { .. } => false,
    })
}

/// Compute all tile instance page translations `[tx, ty]` whose tile cells
/// intersect `ctrl_bounds`.
///
/// Returns `None` if `grid_to_page` is singular or if the required tile count
/// exceeds `max_tiles` (in which case callers can fall back to `PatternFilling`).
pub fn tile_grid_translations(
    tiling: &TilingPaint,
    ctrl_bounds: [[f64; 2]; 2],
    max_tiles: usize,
) -> Option<Vec<[f64; 2]>> {
    let step_x = tiling.step[0];
    let step_y = tiling.step[1];
    if step_x.abs() <= 1e-9 || step_y.abs() <= 1e-9 {
        return None;
    }

    let aff = Affine::new(tiling.grid_to_page);
    let det = aff.determinant();
    if !det.is_finite() || det.abs() <= 1e-18 {
        return None;
    }
    let inv = aff.inverse();

    let [[x0, x1], [y0, y1]] = ctrl_bounds;
    let corners = [
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ];

    let mut gx_min = f64::INFINITY;
    let mut gx_max = f64::NEG_INFINITY;
    let mut gy_min = f64::INFINITY;
    let mut gy_max = f64::NEG_INFINITY;
    for c in corners {
        let g = inv * c;
        gx_min = gx_min.min(g.x);
        gx_max = gx_max.max(g.x);
        gy_min = gy_min.min(g.y);
        gy_max = gy_max.max(g.y);
    }

    let ox = tiling.offset[0];
    let oy = tiling.offset[1];
    let i_min = ((gx_min - ox - tiling.tile_size[0].abs()) / step_x).floor() as i64;
    let i_max = ((gx_max - ox + tiling.tile_size[0].abs()) / step_x).ceil() as i64;
    let j_min = ((gy_min - oy - tiling.tile_size[1].abs()) / step_y).floor() as i64;
    let j_max = ((gy_max - oy + tiling.tile_size[1].abs()) / step_y).ceil() as i64;

    let nx = (i_max - i_min + 1).max(0) as usize;
    let ny = (j_max - j_min + 1).max(0) as usize;
    let total = nx.checked_mul(ny)?;
    if total == 0 || total > max_tiles {
        return None;
    }

    let mut out = Vec::with_capacity(total);
    for j in j_min..=j_max {
        let gy = oy + (j as f64) * step_y;
        for i in i_min..=i_max {
            let gx = ox + (i as f64) * step_x;
            let p = aff * Point::new(gx, gy);
            out.push([p.x, p.y]);
        }
    }
    Some(out)
}
