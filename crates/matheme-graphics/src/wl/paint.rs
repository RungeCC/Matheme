//! Paint, gradient, tiling, and compound path encoding for Wolfram `Graphics`.

use std::f64::consts::{PI, TAU};

use matheme_wl::{fundamental, graphics, list, normal, rule, Expr};

use super::curves::{instanced_expr, point_expr, rgba_expr, stroke_directives};
use super::WlEncoder;
use crate::geometry::{subpaths_axis_aligned_rect, subpaths_ctrl_bounds, subpaths_oriented_rect};
use crate::ir::{
    GradientPaint, GradientSpread, GradientStop, PaintStyle, Rgba, StrokeStyle, Subpath,
    TilingPaint,
};
use crate::paint::{
    map_conic_stops_to_wl, primitives_contain_non_solid_paint, project_linear_stops_to_unit,
    rasterize_gradient_texture, tile_grid_translations,
};

/// Maximum tile instances emitted into a vector-instanced `Texture[Graphics[...]]`
/// before falling back to Wolfram's `PatternFilling`.
const MAX_VECTOR_TILES: usize = 1024;

/// Maximum pixel dimension for smooth rasterized fallback textures (off-center
/// focal radial gradients and skewed conic/radial gradients).
const GRADIENT_TEXTURE_MAX_DIM: usize = 96;

impl WlEncoder {
    pub(super) fn paths_to_expr(
        &self,
        paths: &[Vec<Subpath>],
        fill: Option<&PaintStyle>,
        stroke: Option<&StrokeStyle>,
        plot_width: f64,
        has_texture: &mut bool,
    ) -> Option<Expr> {
        let valid_paths: Vec<&[Subpath]> = paths
            .iter()
            .map(Vec::as_slice)
            .filter(|sp| sp.iter().any(|s| !s.segments.is_empty()))
            .collect();
        if valid_paths.is_empty() {
            return None;
        }

        let all_closed = valid_paths.iter().all(|sp| sp.iter().all(|s| s.closed));

        match fill {
            None => self.styled_group(
                &valid_paths,
                None,
                stroke,
                all_closed,
                plot_width,
                |sp| self.build_curve_exprs(sp, false).0,
                |sp| self.build_joined_curve_expr(sp),
            ),
            Some(PaintStyle::Solid(c)) => self.styled_group(
                &valid_paths,
                Some(*c),
                stroke,
                all_closed,
                plot_width,
                |sp| self.build_curve_exprs(sp, false).0,
                |sp| self.build_joined_curve_expr(sp),
            ),
            Some(paint) => {
                let combined: Vec<Subpath>;
                let all_subpaths: &[Subpath] = if valid_paths.len() == 1 {
                    valid_paths[0]
                } else {
                    combined = valid_paths.iter().flat_map(|s| s.iter().cloned()).collect();
                    &combined
                };
                let ctrl_bounds = subpaths_ctrl_bounds(all_subpaths)?;

                // Pure-vector fast path for rectangular tiling fills: emit an
                // exact clipped `Inset[Graphics[...]]` instead of a rasterized
                // `Texture[Graphics[...]]`.
                if let PaintStyle::Tiling(tiling) = paint {
                    if let Some(inset_fill) =
                        self.tiling_rect_inset_expr(tiling, all_subpaths, ctrl_bounds)
                    {
                        return match stroke {
                            None => Some(inset_fill),
                            Some(stroke_style) => {
                                let mut stroke_group = stroke_directives(stroke_style, plot_width);
                                for sp in &valid_paths {
                                    stroke_group.push(self.build_joined_curve_expr(sp));
                                }
                                Some(list![inset_fill, Expr::list(stroke_group)])
                            }
                        };
                    }
                }

                let (fill_dir, is_texture) = self.paint_fill_directive(paint, ctrl_bounds)?;
                if is_texture {
                    *has_texture = true;
                }
                let (filled_curve, _) = self.build_curve_exprs(all_subpaths, false);

                match stroke {
                    None => Some(list![self.edge_form_none.clone(), fill_dir, filled_curve]),
                    Some(stroke_style) if all_closed && !is_texture => Some(list![
                        fill_dir,
                        normal!(
                            &graphics::EDGE_FORM,
                            Expr::list(stroke_directives(stroke_style, plot_width)),
                        ),
                        filled_curve,
                    ]),
                    Some(stroke_style) => {
                        let fill_group =
                            list![self.edge_form_none.clone(), fill_dir, filled_curve];
                        let mut stroke_group = stroke_directives(stroke_style, plot_width);
                        for sp in &valid_paths {
                            stroke_group.push(self.build_joined_curve_expr(sp));
                        }
                        Some(list![fill_group, Expr::list(stroke_group)])
                    }
                }
            }
        }
    }

    fn tiling_rect_inset_expr(
        &self,
        tiling: &TilingPaint,
        all_subpaths: &[Subpath],
        ctrl_bounds: [[f64; 2]; 2],
    ) -> Option<Expr> {
        if tiling.primitives.is_empty() {
            return None;
        }
        if let Some(rect @ [[x0, x1], [y0, y1]]) = subpaths_axis_aligned_rect(all_subpaths) {
            let cw = (x1 - x0).abs().max(1e-6);
            let ch = (y1 - y0).abs().max(1e-6);
            let translations = tile_grid_translations(tiling, rect, MAX_VECTOR_TILES)?;
            let mut tile_exprs = Vec::with_capacity(tiling.primitives.len());
            let mut inner_has_texture = false;
            for prim in &tiling.primitives {
                if let Some(e) = self.primitive_to_expr(prim, cw, &mut inner_has_texture) {
                    tile_exprs.push(e);
                }
            }
            if tile_exprs.is_empty() {
                return None;
            }
            let instanced = instanced_expr(Expr::list(tile_exprs), &translations);
            let mut elements = Vec::with_capacity(2);
            if inner_has_texture {
                elements.push(self.texture_srgb_sentinel());
            }
            elements.push(instanced);
            let inner = normal!(
                &graphics::GRAPHICS,
                Expr::list(elements),
                &graphics::PLOT_RANGE => list![list![x0, x1], list![y0, y1]],
                &graphics::PLOT_RANGE_PADDING => &fundamental::NONE,
                &graphics::IMAGE_PADDING => &fundamental::NONE,
                &graphics::PLOT_RANGE_CLIPPING => true,
                &graphics::ASPECT_RATIO => &fundamental::FULL,
            );
            return Some(normal!(
                &graphics::INSET,
                inner,
                point_expr([x0, y0]),
                self.origin_left_bottom.clone(),
                list![cw, ch],
            ));
        }

        if !primitives_contain_non_solid_paint(&tiling.primitives) {
            if let Some(orect) = subpaths_oriented_rect(all_subpaths) {
                let translations = tile_grid_translations(tiling, ctrl_bounds, MAX_VECTOR_TILES)?;
                let mut tile_exprs = Vec::with_capacity(tiling.primitives.len());
                let mut inner_has_texture = false;
                for prim in &tiling.primitives {
                    if let Some(e) =
                        self.primitive_to_expr(prim, orect.width, &mut inner_has_texture)
                    {
                        tile_exprs.push(e);
                    }
                }
                if tile_exprs.is_empty() || inner_has_texture {
                    return None;
                }
                let instanced = instanced_expr(Expr::list(tile_exprs), &translations);
                let [c, s] = orect.dir;
                let [ox, oy] = orect.origin;
                let inv_rot_mat = list![list![c, s], list![-s, c]];
                let inv_trans = point_expr([-c * ox - s * oy, s * ox - c * oy]);
                let local_tiles = normal!(
                    &graphics::GEOMETRIC_TRANSFORMATION,
                    instanced,
                    list![inv_rot_mat, inv_trans],
                );
                let inner = normal!(
                    &graphics::GRAPHICS,
                    list![local_tiles],
                    &graphics::PLOT_RANGE => list![
                        list![0.0_f64, orect.width],
                        list![0.0_f64, orect.height],
                    ],
                    &graphics::PLOT_RANGE_PADDING => &fundamental::NONE,
                    &graphics::IMAGE_PADDING => &fundamental::NONE,
                    &graphics::PLOT_RANGE_CLIPPING => true,
                    &graphics::ASPECT_RATIO => &fundamental::FULL,
                );
                return Some(normal!(
                    &graphics::INSET,
                    inner,
                    point_expr(orect.origin),
                    self.origin_left_bottom.clone(),
                    list![orect.width, orect.height],
                    point_expr(orect.dir),
                ));
            }
        }

        None
    }

    fn paint_fill_directive(
        &self,
        paint: &PaintStyle,
        ctrl_bounds: [[f64; 2]; 2],
    ) -> Option<(Expr, bool)> {
        match paint {
            PaintStyle::Solid(c) => Some((normal!(&graphics::FACE_FORM, rgba_expr(*c)), false)),
            PaintStyle::Gradient(grad) => Some(gradient_fill_directive(grad, ctrl_bounds)),
            PaintStyle::Tiling(tiling) => self.tiling_fill_directive(tiling, ctrl_bounds),
        }
    }

    fn tiling_fill_directive(
        &self,
        tiling: &TilingPaint,
        ctrl_bounds: [[f64; 2]; 2],
    ) -> Option<(Expr, bool)> {
        if tiling.primitives.is_empty() {
            return None;
        }
        let [[x0, x1], _] = ctrl_bounds;
        let cw = (x1 - x0).abs().max(1e-6);

        if let Some(translations) = tile_grid_translations(tiling, ctrl_bounds, MAX_VECTOR_TILES) {
            let mut tile_exprs = Vec::with_capacity(tiling.primitives.len());
            let mut inner_has_texture = false;
            for prim in &tiling.primitives {
                if let Some(e) = self.primitive_to_expr(prim, cw, &mut inner_has_texture) {
                    tile_exprs.push(e);
                }
            }
            if tile_exprs.is_empty() {
                return None;
            }
            let instanced = instanced_expr(Expr::list(tile_exprs), &translations);
            let mut elements = Vec::with_capacity(2);
            if inner_has_texture {
                elements.push(self.texture_srgb_sentinel());
            }
            elements.push(instanced);
            let inner = self.sub_graphics_for_texture(elements, ctrl_bounds);
            return Some((normal!(&graphics::TEXTURE, inner), true));
        }

        let [sx, ky, kx, sy, _, _] = tiling.grid_to_page;
        let step_w = (tiling.step[0] * sx.hypot(ky)).abs().max(1e-3);
        let step_h = (tiling.step[1] * kx.hypot(sy)).abs().max(1e-3);
        let cell_w = (tiling.tile_size[0] * sx.hypot(ky)).abs().max(1e-3);
        let cell_h = (tiling.tile_size[1] * kx.hypot(sy)).abs().max(1e-3);

        let (px0, px1) = if sx >= 0.0 {
            (0.0, step_w.max(cell_w))
        } else {
            (-step_w.max(cell_w), 0.0)
        };
        let (py0, py1) = if sy >= 0.0 {
            (0.0, step_h.max(cell_h))
        } else {
            (-step_h.max(cell_h), 0.0)
        };
        let mut tile_exprs = Vec::with_capacity(tiling.primitives.len() + 1);
        let mut inner_has_texture = false;
        for prim in &tiling.primitives {
            if let Some(e) = self.primitive_to_expr(
                prim,
                (px1 - px0).abs().max(1e-6),
                &mut inner_has_texture,
            ) {
                tile_exprs.push(e);
            }
        }
        if tile_exprs.is_empty() {
            return None;
        }
        if inner_has_texture {
            tile_exprs.insert(0, self.texture_srgb_sentinel());
        }
        let tile_graphics = self.sub_graphics_for_texture(tile_exprs, [[px0, px1], [py0, py1]]);
        Some((
            normal!(&graphics::PATTERN_FILLING, tile_graphics, list![step_w, step_h]),
            false,
        ))
    }
}

fn gradient_fill_directive(grad: &GradientPaint, ctrl_bounds: [[f64; 2]; 2]) -> (Expr, bool) {
    let [[x0, x1], [y0, y1]] = ctrl_bounds;
    let cx = 0.5 * (x0 + x1);
    let cy = 0.5 * (y0 + y1);
    let m = (x1 - x0).abs().max((y1 - y0).abs()).max(1e-9);
    let black_face = || {
        normal!(
            &graphics::FACE_FORM,
            rgba_expr(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
        )
    };

    match grad {
        GradientPaint::Linear {
            origin,
            grad: gvec,
            stops,
            spread,
        } => {
            if stops.is_empty() {
                return (black_face(), false);
            }
            let g_norm = gvec[0].hypot(gvec[1]);
            if g_norm <= 1e-12 {
                return (normal!(&graphics::FACE_FORM, rgba_expr(stops[0].color)), false);
            }
            let dx = gvec[0] / g_norm;
            let dy = gvec[1] / g_norm;
            let theta = dy.atan2(dx);
            let t_start = 0.5 + ((origin[0] - cx) * dx + (origin[1] - cy) * dy) / m;
            let span = 1.0 / (m * g_norm);
            let proj = project_linear_stops_to_unit(stops, t_start, span, *spread);
            let stops_rule = gradient_stops_rule(&proj.stops);
            let dir = if proj.spread == GradientSpread::Pad {
                normal!(&graphics::LINEAR_GRADIENT_FILLING, stops_rule, theta)
            } else {
                normal!(
                    &graphics::LINEAR_GRADIENT_FILLING,
                    stops_rule,
                    theta,
                    wl_spread_name(proj.spread),
                )
            };
            (dir, false)
        }
        GradientPaint::Radial {
            local_to_page,
            center,
            radius,
            focal_center,
            focal_radius,
            stops,
            spread,
        } => {
            if stops.is_empty() {
                return (black_face(), false);
            }
            let [sx, ky, kx, sy, tx, ty] = *local_to_page;
            let concentric = (focal_center[0] - center[0]).abs() <= 1e-5
                && (focal_center[1] - center[1]).abs() <= 1e-5
                && *focal_radius <= 1e-5;
            let axis_aligned = kx.abs() <= 1e-5 && ky.abs() <= 1e-5;

            if concentric && axis_aligned {
                let pc_x = sx * center[0] + tx;
                let pc_y = sy * center[1] + ty;
                let u_c = 0.5 + (pc_x - cx) / m;
                let v_c = 0.5 + (pc_y - cy) / m;
                let r_u = (radius * sx.abs()).max(1e-9) / m;
                let r_v = (radius * sy.abs()).max(1e-9) / m;
                let stops_rule = gradient_stops_rule(stops);
                let geom = list![list![u_c, v_c], list![r_u, r_v]];
                let dir = if *spread == GradientSpread::Pad {
                    normal!(&graphics::RADIAL_GRADIENT_FILLING, stops_rule, geom)
                } else {
                    normal!(
                        &graphics::RADIAL_GRADIENT_FILLING,
                        stops_rule,
                        geom,
                        wl_spread_name(*spread),
                    )
                };
                return (dir, false);
            }

            if let Some(tex) = gradient_texture_expr(grad, ctrl_bounds) {
                return (tex, true);
            }
            (
                normal!(
                    &graphics::FACE_FORM,
                    rgba_expr(stops[stops.len() / 2].color),
                ),
                false,
            )
        }
        GradientPaint::Conic {
            local_to_page,
            container_size,
            center,
            angle,
            stops,
        } => {
            if stops.is_empty() {
                return (black_face(), false);
            }
            let [sx, ky, kx, sy, tx, ty] = *local_to_page;
            let cw_loc = container_size[0].abs().max(1e-9);
            let ch_loc = container_size[1].abs().max(1e-9);
            let ax = sx / cw_loc;
            let ay = ky / cw_loc;
            let bx = kx / ch_loc;
            let by = sy / ch_loc;
            let len_a = ax.hypot(ay);
            let len_b = bx.hypot(by);
            let dot = ax * bx + ay * by;
            let is_similarity = len_a > 1e-12
                && len_b > 1e-12
                && (len_a - len_b).abs() <= 1e-3 * len_a.max(len_b)
                && dot.abs() <= 1e-3 * len_a * len_b;

            if is_similarity {
                let pc_x = sx * center[0] + kx * center[1] + tx;
                let pc_y = ky * center[0] + sy * center[1] + ty;
                let u_c = 0.5 + (pc_x - cx) / m;
                let v_c = 0.5 + (pc_y - cy) / m;
                let det = ax * by - bx * ay;
                let phi_x = ay.atan2(ax);
                let (start_angle_wl, clockwise_wl) = if det < 0.0 {
                    ((phi_x + PI - *angle).rem_euclid(TAU), true)
                } else {
                    ((phi_x + *angle - PI).rem_euclid(TAU), false)
                };
                let wl_stops = map_conic_stops_to_wl(stops, start_angle_wl, clockwise_wl);
                if wl_stops.len() >= 2 {
                    let angles = Expr::list(wl_stops.iter().map(|(th, _)| *th));
                    let colors = Expr::list(wl_stops.iter().map(|(_, c)| rgba_expr(*c)));
                    return (
                        normal!(
                            &graphics::CONIC_GRADIENT_FILLING,
                            rule!(angles => colors),
                            list![u_c, v_c],
                        ),
                        false,
                    );
                }
            }

            if let Some(tex) = gradient_texture_expr(grad, ctrl_bounds) {
                return (tex, true);
            }
            (
                normal!(
                    &graphics::FACE_FORM,
                    rgba_expr(stops[stops.len() / 2].color),
                ),
                false,
            )
        }
    }
}

fn gradient_stops_rule(stops: &[GradientStop]) -> Expr {
    let offsets = Expr::list(stops.iter().map(|s| s.offset));
    let colors = Expr::list(stops.iter().map(|s| rgba_expr(s.color)));
    Expr::from(rule!(offsets => colors))
}

fn wl_spread_name(spread: GradientSpread) -> &'static str {
    match spread {
        GradientSpread::Pad => "Fixed",
        GradientSpread::Repeat => "Periodic",
        GradientSpread::Reflect => "Reflected",
    }
}

/// Encode a rasterized fallback gradient as `Texture[Image[NumericArray[...], "Byte", ColorSpace -> "RGB"]]`.
///
/// Transmitting the pixel buffer as a packed `UnsignedInteger8` `NumericArray`
/// uses WXF (`BinaryDeserialize[ByteArray[...]]`) over WSTP instead of
/// constructing ~46k individual `List`/`Integer` expressions.
fn gradient_texture_expr(grad: &GradientPaint, ctrl_bounds: [[f64; 2]; 2]) -> Option<Expr> {
    let (w, h, pixels) = rasterize_gradient_texture(grad, ctrl_bounds, GRADIENT_TEXTURE_MAX_DIM)?;
    let na = Expr::numeric_array_u8(&[h, w, 4], &pixels);
    let img = normal!(
        &graphics::IMAGE,
        na,
        "Byte",
        &graphics::COLOR_SPACE => "RGB",
    );
    Some(normal!(&graphics::TEXTURE, img))
}
