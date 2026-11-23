//! Gradient and tiling coordinate/stop mapping math shared between extraction
//! and host encoders.

pub mod gradient;
pub mod tiling;

pub use crate::geometry::{
    subpaths_axis_aligned_rect, subpaths_ctrl_bounds, subpaths_oriented_rect, OrientedRect,
};
pub use crate::ir::{
    ClipRegion, GradientPaint, GradientSpread, GradientStop, PaintStyle, Rgba, TilingPaint,
};
pub use gradient::{
    apply_spread, eval_radial_2circle, expand_typst_gradient_stops, linear_paint_from_local,
    map_conic_stops_to_wl, project_linear_stops_to_unit, rasterize_gradient_texture, sample_stops,
    typst_gradient_to_paint, ProjectedLinearStops,
};
pub use tiling::{primitives_contain_non_solid_paint, tile_grid_translations};
