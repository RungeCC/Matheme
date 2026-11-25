//! Conversion of [`crate::ir::VectorPage`] into native Wolfram Language
//! ``System`Graphics`` [`Expr`]s.

mod curves;
mod image;
mod paint;
#[cfg(test)]
mod tests;

use matheme_wl::{Expr, fundamental, graphics, list, normal};

use crate::ir::{ClipRegion, Rgba, StrokeStyle, Subpath, VectorPage, VectorPrimitive};

pub(crate) use curves::clear_expr_cache;

/// Per-export encoder holding reusable small `Expr` building blocks (segment
/// descriptors, `EdgeForm[None]`, `{Left, Bottom}`) so hot loops avoid repeated
/// allocations without relying on process-wide static `Expr` globals.
struct WlEncoder {
    edge_form_none: Expr,
    origin_left_bottom: Expr,
    seg_line_first: Expr,
    seg_line_cont: Expr,
    seg_cubic_first: Expr,
    seg_cubic_cont: Expr,
}

impl WlEncoder {
    pub(super) fn new() -> Self {
        Self {
            edge_form_none: normal!(&graphics::EDGE_FORM, &fundamental::NONE),
            origin_left_bottom: list![&graphics::LEFT, &graphics::BOTTOM],
            seg_line_first: list![0_i64, 2_i64, 0_i64],
            seg_line_cont: list![0_i64, 1_i64, 0_i64],
            seg_cubic_first: list![1_i64, 4_i64, 3_i64],
            seg_cubic_cont: list![1_i64, 3_i64, 3_i64],
        }
    }

    /// Zero-geometry sentinel `{EdgeForm[None], LinearGradientFilling[{transparent, transparent}], Polygon[{}]}`.
    ///
    /// In Wolfram 15 on Windows, a 2D `Graphics` containing `Texture[...]`
    /// without any `*GradientFilling` primitive falls back to a legacy
    /// rendering path that erroneously applies an extra sRGB gamma shift to
    /// the entire graphic. Prepending this zero-vertex sentinel opts the
    /// `Graphics` into Wolfram's modern 2D sRGB-accurate pipeline without
    /// drawing any pixels or altering `PlotRange`.
    pub(super) fn texture_srgb_sentinel(&self) -> Expr {
        let transparent = normal!(&fundamental::RGB_COLOR, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
        let grad = normal!(
            &graphics::LINEAR_GRADIENT_FILLING,
            list![transparent.clone(), transparent],
        );
        let empty_poly = normal!(&graphics::POLYGON, list![]);
        list![self.edge_form_none.clone(), grad, empty_poly]
    }

    pub(super) fn page_to_expr(&self, page: &VectorPage) -> Expr {
        let [[x0, x1], [y0, y1]] = page.bounds;
        let img_w = (x1 - x0).abs().max(1e-6);
        let img_h = (y1 - y0).abs().max(1e-6);

        let mut elements = Vec::with_capacity(page.primitives.len() + 1);
        let mut has_texture = false;
        for primitive in &page.primitives {
            if let Some(expr) = self.primitive_to_expr(primitive, img_w, &mut has_texture) {
                elements.push(expr);
            }
        }
        if has_texture {
            elements.insert(0, self.texture_srgb_sentinel());
        }

        normal!(
            &graphics::GRAPHICS,
            Expr::list(elements),
            &graphics::IMAGE_SIZE => list![img_w, img_h],
            &graphics::PLOT_RANGE => list![list![x0, x1], list![y0, y1]],
            &graphics::PLOT_RANGE_PADDING => &fundamental::NONE,
            &graphics::IMAGE_PADDING => &fundamental::NONE,
            &graphics::ASPECT_RATIO => &fundamental::AUTOMATIC,
        )
    }

    pub(super) fn primitive_to_expr(
        &self,
        primitive: &VectorPrimitive,
        plot_width: f64,
        has_texture: &mut bool,
    ) -> Option<Expr> {
        match primitive {
            VectorPrimitive::Glyphs {
                glyphs,
                fill,
                stroke,
            } => self.glyphs_to_expr(glyphs, *fill, stroke.as_ref(), plot_width),
            VectorPrimitive::Paths {
                paths,
                fill,
                stroke,
                ..
            } => self.paths_to_expr(
                paths,
                fill.as_ref(),
                stroke.as_ref(),
                plot_width,
                has_texture,
            ),
            VectorPrimitive::Line { start, end, stroke } => {
                let mut group = curves::stroke_directives(stroke, plot_width);
                group.push(normal!(
                    &graphics::LINE,
                    list![curves::point_expr(*start), curves::point_expr(*end)],
                ));
                Some(Expr::list(group))
            }
            VectorPrimitive::Image {
                payload,
                size,
                transform,
            } => Some(self.image_to_expr(payload, *size, *transform)),
            VectorPrimitive::Clip { region, children } => {
                self.clip_to_expr(region, children, has_texture)
            }
        }
    }

    fn clip_to_expr(
        &self,
        region: &ClipRegion,
        children: &[VectorPrimitive],
        has_texture: &mut bool,
    ) -> Option<Expr> {
        match region {
            ClipRegion::Rect(bounds) => self.rect_clip_to_expr(*bounds, children),
            ClipRegion::Path {
                subpaths,
                ctrl_bounds,
                ..
            } => {
                let expr = self.path_clip_to_expr(subpaths, *ctrl_bounds, children)?;
                *has_texture = true;
                Some(expr)
            }
        }
    }

    fn rect_clip_to_expr(
        &self,
        bounds: [[f64; 2]; 2],
        children: &[VectorPrimitive],
    ) -> Option<Expr> {
        let [[x0, x1], [y0, y1]] = bounds;
        let cw = (x1 - x0).max(1e-6);
        let ch = (y1 - y0).max(1e-6);

        let mut elements = Vec::with_capacity(children.len() + 1);
        let mut inner_has_texture = false;
        for child in children {
            if let Some(expr) = self.primitive_to_expr(child, cw, &mut inner_has_texture) {
                elements.push(expr);
            }
        }
        if elements.is_empty() {
            return None;
        }
        if inner_has_texture {
            elements.insert(0, self.texture_srgb_sentinel());
        }

        let inner = normal!(
            &graphics::GRAPHICS,
            Expr::list(elements),
            &graphics::PLOT_RANGE => list![list![x0, x1], list![y0, y1]],
            &graphics::PLOT_RANGE_PADDING => &fundamental::NONE,
            &graphics::IMAGE_PADDING => &fundamental::NONE,
            &graphics::PLOT_RANGE_CLIPPING => true,
            &graphics::ASPECT_RATIO => &fundamental::FULL,
        );
        Some(normal!(
            &graphics::INSET,
            inner,
            curves::point_expr([x0, y0]),
            self.origin_left_bottom.clone(),
            list![cw, ch],
        ))
    }

    /// Encode an arbitrary curved clip path using Wolfram's native
    /// `Texture[Graphics[...]]` applied to the clip `FilledCurve`.
    fn path_clip_to_expr(
        &self,
        subpaths: &[Subpath],
        ctrl_bounds: [[f64; 2]; 2],
        children: &[VectorPrimitive],
    ) -> Option<Expr> {
        if subpaths.is_empty() {
            return None;
        }
        let [[x0, x1], _] = ctrl_bounds;
        let cw = (x1 - x0).abs().max(1e-6);

        let mut elements = Vec::with_capacity(children.len() + 1);
        let mut inner_has_texture = false;
        for child in children {
            if let Some(expr) = self.primitive_to_expr(child, cw, &mut inner_has_texture) {
                elements.push(expr);
            }
        }
        if elements.is_empty() {
            return None;
        }
        if inner_has_texture {
            elements.insert(0, self.texture_srgb_sentinel());
        }

        let inner = self.sub_graphics_for_texture(elements, ctrl_bounds);
        let (clip_fc, _) = self.build_curve_exprs(subpaths, false);
        Some(list![
            self.edge_form_none.clone(),
            normal!(&graphics::TEXTURE, inner),
            clip_fc,
        ])
    }

    pub(super) fn sub_graphics_for_texture(
        &self,
        elements: Vec<Expr>,
        bounds: [[f64; 2]; 2],
    ) -> Expr {
        let [[x0, x1], [y0, y1]] = bounds;
        let cw = (x1 - x0).abs().max(1e-6);
        let ch = (y1 - y0).abs().max(1e-6);
        // Supersample Texture sub-graphics at 4× (288 DPI equivalent, smoothly
        // capped at 2048 pt on the long edge) so curved clips and non-rectangular
        // tilings stay crisp under high-DPI rasterization and Notebook zoom.
        let max_dim = cw.max(ch);
        let tex_scale = (2048.0 / max_dim).clamp(1.0, 4.0);
        normal!(
            &graphics::GRAPHICS,
            Expr::list(elements),
            &graphics::PLOT_RANGE => list![list![x0, x1], list![y0, y1]],
            &graphics::PLOT_RANGE_PADDING => &fundamental::NONE,
            &graphics::IMAGE_PADDING => &fundamental::NONE,
            &graphics::PLOT_RANGE_CLIPPING => true,
            &graphics::ASPECT_RATIO => &fundamental::FULL,
            &graphics::IMAGE_SIZE => list![cw * tex_scale, ch * tex_scale],
            &graphics::BACKGROUND => &fundamental::NONE,
        )
    }

    /// Shared `(solid_fill, stroke, all_closed)` primitive styling assembler
    /// used by solid-color glyphs and paths.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn styled_group<I>(
        &self,
        items: &[I],
        fill: Option<Rgba>,
        stroke: Option<&StrokeStyle>,
        all_closed: bool,
        plot_width: f64,
        mut make_filled: impl FnMut(&I) -> Expr,
        mut make_joined: impl FnMut(&I) -> Expr,
    ) -> Option<Expr> {
        match (fill, stroke) {
            (Some(fill_color), None) => {
                let mut group = Vec::with_capacity(items.len() + 2);
                group.push(self.edge_form_none.clone());
                group.push(normal!(&graphics::FACE_FORM, curves::rgba_expr(fill_color)));
                for item in items {
                    group.push(make_filled(item));
                }
                Some(Expr::list(group))
            }
            (Some(fill_color), Some(stroke_style)) if all_closed => {
                let mut group = Vec::with_capacity(items.len() + 2);
                group.push(normal!(&graphics::FACE_FORM, curves::rgba_expr(fill_color)));
                group.push(normal!(
                    &graphics::EDGE_FORM,
                    Expr::list(curves::stroke_directives(stroke_style, plot_width)),
                ));
                for item in items {
                    group.push(make_filled(item));
                }
                Some(Expr::list(group))
            }
            (Some(fill_color), Some(stroke_style)) => {
                let mut fill_group = Vec::with_capacity(items.len() + 2);
                fill_group.push(self.edge_form_none.clone());
                fill_group.push(normal!(&graphics::FACE_FORM, curves::rgba_expr(fill_color)));
                let mut stroke_group = curves::stroke_directives(stroke_style, plot_width);
                for item in items {
                    fill_group.push(make_filled(item));
                    stroke_group.push(make_joined(item));
                }
                Some(list![Expr::list(fill_group), Expr::list(stroke_group)])
            }
            (None, Some(stroke_style)) => {
                let mut group = curves::stroke_directives(stroke_style, plot_width);
                for item in items {
                    group.push(make_joined(item));
                }
                Some(Expr::list(group))
            }
            (None, None) => None,
        }
    }
}

/// Convert multiple [`VectorPage`]s into a Wolfram Language `List` of
/// ``System`Graphics`` [`Expr`]s.
pub fn pages_to_expr(pages: &[VectorPage]) -> Expr {
    let encoder = WlEncoder::new();
    Expr::list(pages.iter().map(|page| encoder.page_to_expr(page)))
}

/// Convert one [`VectorPage`] into a ``System`Graphics`` [`Expr`] with explicit
/// numeric `PlotRange`, zero padding, and exact pt `ImageSize`.
pub fn page_to_expr(page: &VectorPage) -> Expr {
    WlEncoder::new().page_to_expr(page)
}
