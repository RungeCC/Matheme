//! Conversion of [`crate::extract::VectorPage`] into native Wolfram
//! Language ``System`Graphics`` [`Expr`]s.

use std::f64::consts::{PI, TAU};
use std::sync::{LazyLock, Mutex};

use matheme_wl::{fundamental, graphics, Expr, Symbol};

use crate::extract::{
    cache_generation, EmbeddedImageFormat, GlyphInstances, GlyphKey, LineCapStyle, LineJoinStyle,
    Segment, StrokeStyle, Subpath, VectorPage, VectorPrimitive, GLYPH_CACHE_CAPACITY,
};
use crate::lru::{lock_unpoisoned, LruCache};
use crate::paint::{
    map_conic_stops_to_wl, project_linear_stops_to_unit, rasterize_gradient_texture,
    subpaths_ctrl_bounds, tile_grid_translations, ClipRegion, GradientPaint, GradientSpread,
    GradientStop, PaintStyle, Rgba, TilingPaint,
};

/// Maximum tile instances emitted into a vector-instanced `Texture[Graphics[...]]`
/// before falling back to Wolfram's `PatternFilling`.
const MAX_VECTOR_TILES: usize = 1024;

/// Maximum pixel dimension for smooth rasterized fallback textures (off-center
/// focal radial gradients and skewed conic/radial gradients).
const GRADIENT_TEXTURE_MAX_DIM: usize = 96;

#[derive(Clone)]
struct CachedGlyphExpr {
    filled: Expr,
    joined: Option<Expr>,
}

struct GlyphExprCache {
    generation: u64,
    entries: LruCache<GlyphKey, CachedGlyphExpr>,
}

impl GlyphExprCache {
    fn new() -> Self {
        Self {
            generation: 0,
            entries: LruCache::new(GLYPH_CACHE_CAPACITY),
        }
    }
}

static GLYPH_EXPR_CACHE: LazyLock<Mutex<GlyphExprCache>> =
    LazyLock::new(|| Mutex::new(GlyphExprCache::new()));

pub(crate) fn clear_expr_cache() {
    let mut guard = lock_unpoisoned(&GLYPH_EXPR_CACHE);
    guard.entries.clear();
    guard.generation = cache_generation();
}

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
    fn new() -> Self {
        Self {
            edge_form_none: Expr::normal(&graphics::EDGE_FORM, [Expr::symbol(&fundamental::NONE)]),
            origin_left_bottom: Expr::list([
                Expr::symbol(&graphics::LEFT),
                Expr::symbol(&graphics::BOTTOM),
            ]),
            seg_line_first: Expr::list([0_i64, 2_i64, 0_i64]),
            seg_line_cont: Expr::list([0_i64, 1_i64, 0_i64]),
            seg_cubic_first: Expr::list([1_i64, 4_i64, 3_i64]),
            seg_cubic_cont: Expr::list([1_i64, 3_i64, 3_i64]),
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
    fn texture_srgb_sentinel(&self) -> Expr {
        let transparent = Expr::normal(&fundamental::RGB_COLOR, [0.0_f64, 0.0, 0.0, 0.0]);
        let grad = Expr::normal(
            &graphics::LINEAR_GRADIENT_FILLING,
            [Expr::list([transparent.clone(), transparent])],
        );
        let empty_poly = Expr::normal(&graphics::POLYGON, [Expr::list(std::iter::empty::<Expr>())]);
        Expr::list([self.edge_form_none.clone(), grad, empty_poly])
    }

    fn page_to_expr(&self, page: &VectorPage) -> Expr {
        let [[x0, x1], [y0, y1]] = page.bounds;
        let img_w = (x1 - x0).abs().max(1e-6);
        let img_h = (y1 - y0).abs().max(1e-6);
        let plot_width = img_w;

        let mut elements = Vec::with_capacity(page.primitives.len() + 1);
        let mut has_texture = false;
        for primitive in &page.primitives {
            if let Some(expr) = self.primitive_to_expr(primitive, plot_width, &mut has_texture) {
                elements.push(expr);
            }
        }
        if has_texture {
            elements.insert(0, self.texture_srgb_sentinel());
        }

        Expr::normal(
            &graphics::GRAPHICS,
            [
                Expr::list(elements),
                rule_sym_expr(&graphics::IMAGE_SIZE, Expr::list([img_w, img_h])),
                rule_sym_expr(
                    &graphics::PLOT_RANGE,
                    Expr::list([Expr::list([x0, x1]), Expr::list([y0, y1])]),
                ),
                rule_sym_sym(&graphics::PLOT_RANGE_PADDING, &fundamental::NONE),
                rule_sym_sym(&graphics::IMAGE_PADDING, &fundamental::NONE),
                rule_sym_sym(&graphics::ASPECT_RATIO, &fundamental::AUTOMATIC),
            ],
        )
    }

    fn primitive_to_expr(
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
            } => self.paths_to_expr(paths, fill.as_ref(), stroke.as_ref(), plot_width, has_texture),
            VectorPrimitive::Line { start, end, stroke } => {
                let mut group = stroke_directives(stroke, plot_width);
                group.push(Expr::normal(
                    &graphics::LINE,
                    [Expr::list([point_expr(*start), point_expr(*end)])],
                ));
                Some(Expr::list(group))
            }
            VectorPrimitive::Image {
                format,
                data,
                size,
                transform,
            } => Some(self.image_to_expr(*format, data.as_ref(), *size, *transform)),
            VectorPrimitive::Clip { region, children } => {
                self.clip_to_expr(region, children, has_texture)
            }
        }
    }

    fn image_to_expr(
        &self,
        format: EmbeddedImageFormat,
        data: &[u8],
        size: [f64; 2],
        transform: [f64; 6],
    ) -> Expr {
        let imported = Expr::normal(
            &graphics::IMPORT_BYTE_ARRAY,
            [Expr::bytes(data), Expr::string(wl_image_format(format))],
        );
        let stretched = Expr::normal(
            &graphics::SHOW,
            [
                imported,
                rule_sym_sym(&graphics::ASPECT_RATIO, &fundamental::FULL),
            ],
        );
        let [sx, ky, kx, sy, tx, ty] = transform;

        if kx.abs() < 1e-9 && ky.abs() < 1e-9 && sx > 0.0 && sy > 0.0 {
            Expr::normal(
                &graphics::INSET,
                [
                    stretched,
                    point_expr([tx, ty]),
                    self.origin_left_bottom.clone(),
                    Expr::list([size[0] * sx, size[1] * sy]),
                ],
            )
        } else {
            let inset = Expr::normal(
                &graphics::INSET,
                [
                    stretched,
                    point_expr([0.0, 0.0]),
                    self.origin_left_bottom.clone(),
                    Expr::list([size[0], size[1]]),
                ],
            );
            // Column-major `[sx, ky, kx, sy]` corresponds to WL 2×2 row-major matrix
            // `{{sx, kx}, {ky, sy}}` with translation vector `{tx, ty}`.
            let matrix = Expr::list([Expr::list([sx, kx]), Expr::list([ky, sy])]);
            Expr::normal(
                &graphics::GEOMETRIC_TRANSFORMATION,
                [inset, Expr::list([matrix, point_expr([tx, ty])])],
            )
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
            // Strokes inside clipped subgraphics normalize relative to `cw` so
            // `Thickness[stroke_pt / cw]` preserves exact typographic point widths.
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

        let inner = Expr::normal(
            &graphics::GRAPHICS,
            [
                Expr::list(elements),
                rule_sym_expr(
                    &graphics::PLOT_RANGE,
                    Expr::list([Expr::list([x0, x1]), Expr::list([y0, y1])]),
                ),
                rule_sym_sym(&graphics::PLOT_RANGE_PADDING, &fundamental::NONE),
                rule_sym_sym(&graphics::IMAGE_PADDING, &fundamental::NONE),
                rule_sym_expr(&graphics::PLOT_RANGE_CLIPPING, Expr::boolean(true)),
                rule_sym_sym(&graphics::ASPECT_RATIO, &fundamental::FULL),
            ],
        );
        Some(Expr::normal(
            &graphics::INSET,
            [
                inner,
                point_expr([x0, y0]),
                self.origin_left_bottom.clone(),
                Expr::list([cw, ch]),
            ],
        ))
    }

    /// Encode an arbitrary curved clip path using Wolfram's native
    /// `Texture[Graphics[...]]` applied to the clip `FilledCurve`.
    ///
    /// Using the clip curve's control-point bounding box (`ctrl_bounds`) as the
    /// inner `Graphics`'s `PlotRange` matches Wolfram's `FilledCurve` UV
    /// parameterization exactly (even on bulging cubic Beziers), while
    /// `Background -> None` preserves full transparency and FrontEnd renders
    /// the inner vector `Graphics` at the target output DPI.
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
        Some(Expr::list([
            self.edge_form_none.clone(),
            Expr::normal(&graphics::TEXTURE, [inner]),
            clip_fc,
        ]))
    }

    fn sub_graphics_for_texture(&self, elements: Vec<Expr>, bounds: [[f64; 2]; 2]) -> Expr {
        let [[x0, x1], [y0, y1]] = bounds;
        let cw = (x1 - x0).abs().max(1e-6);
        let ch = (y1 - y0).abs().max(1e-6);
        // Supersample small Texture sub-graphics (up to 4×, targeting >= 288 pt
        // on the long edge) so non-rectangular clips and tilings stay crisp
        // under high-DPI rasterization and Notebook zoom; `Thickness` is
        // proportional to width so visual stroke widths remain identical.
        let max_dim = cw.max(ch);
        let tex_scale = if max_dim < 288.0 {
            (288.0 / max_dim).clamp(1.0, 4.0)
        } else {
            1.0
        };
        Expr::normal(
            &graphics::GRAPHICS,
            [
                Expr::list(elements),
                rule_sym_expr(
                    &graphics::PLOT_RANGE,
                    Expr::list([Expr::list([x0, x1]), Expr::list([y0, y1])]),
                ),
                rule_sym_sym(&graphics::PLOT_RANGE_PADDING, &fundamental::NONE),
                rule_sym_sym(&graphics::IMAGE_PADDING, &fundamental::NONE),
                rule_sym_expr(&graphics::PLOT_RANGE_CLIPPING, Expr::boolean(true)),
                rule_sym_sym(&graphics::ASPECT_RATIO, &fundamental::FULL),
                rule_sym_expr(
                    &graphics::IMAGE_SIZE,
                    Expr::list([cw * tex_scale, ch * tex_scale]),
                ),
                rule_sym_sym(&graphics::BACKGROUND, &fundamental::NONE),
            ],
        )
    }

    fn build_curve_exprs(&self, subpaths: &[Subpath], need_joined: bool) -> (Expr, Option<Expr>) {
        let (all_segs, all_pts, closed_flags) = self.encode_subpaths(subpaths);
        let segs_expr = Expr::list(all_segs);
        let pts_expr = Expr::list(all_pts);
        let filled = Expr::normal(&graphics::FILLED_CURVE, [segs_expr.clone(), pts_expr.clone()]);
        let joined = need_joined.then(|| joined_curve_from_encoded(segs_expr, pts_expr, closed_flags));
        (filled, joined)
    }

    fn build_joined_curve_expr(&self, subpaths: &[Subpath]) -> Expr {
        let (all_segs, all_pts, closed_flags) = self.encode_subpaths(subpaths);
        joined_curve_from_encoded(Expr::list(all_segs), Expr::list(all_pts), closed_flags)
    }

    fn cached_glyph_expr(&self, instance: &GlyphInstances, need_joined: bool) -> (Expr, Option<Expr>) {
        let gen_now = cache_generation();
        let mut existing_filled: Option<Expr> = None;

        {
            let mut guard = lock_unpoisoned(&GLYPH_EXPR_CACHE);
            if guard.generation != gen_now {
                guard.entries.clear();
                guard.generation = gen_now;
            } else if let Some(cached) = guard.entries.get(&instance.key) {
                if !need_joined || cached.joined.is_some() {
                    return (cached.filled.clone(), cached.joined.clone());
                }
                // FilledCurve is already cached; only lazily build JoinedCurve below.
                existing_filled = Some(cached.filled.clone());
            }
        }

        let (filled, joined) = match existing_filled {
            Some(filled) => (filled, Some(self.build_joined_curve_expr(&instance.subpaths))),
            None => self.build_curve_exprs(&instance.subpaths, need_joined),
        };

        let mut guard = lock_unpoisoned(&GLYPH_EXPR_CACHE);
        if guard.generation != gen_now {
            guard.entries.clear();
            guard.generation = gen_now;
        }

        if let Some(existing) = guard.entries.get_mut(&instance.key) {
            if need_joined && existing.joined.is_none() {
                existing.joined = joined.clone();
            }
            return (existing.filled.clone(), existing.joined.clone());
        }

        guard.entries.insert(
            instance.key,
            CachedGlyphExpr {
                filled: filled.clone(),
                joined: joined.clone(),
            },
        );

        (filled, joined)
    }

    /// Shared `(solid_fill, stroke, all_closed)` primitive styling assembler
    /// used by solid-color glyphs and paths.
    fn styled_group<I>(
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
                group.push(Expr::normal(&graphics::FACE_FORM, [rgba_expr(fill_color)]));
                for item in items {
                    group.push(make_filled(item));
                }
                Some(Expr::list(group))
            }
            (Some(fill_color), Some(stroke_style)) if all_closed => {
                let mut group = Vec::with_capacity(items.len() + 2);
                group.push(Expr::normal(&graphics::FACE_FORM, [rgba_expr(fill_color)]));
                group.push(Expr::normal(
                    &graphics::EDGE_FORM,
                    [Expr::list(stroke_directives(stroke_style, plot_width))],
                ));
                for item in items {
                    group.push(make_filled(item));
                }
                Some(Expr::list(group))
            }
            (Some(fill_color), Some(stroke_style)) => {
                let mut fill_group = Vec::with_capacity(items.len() + 2);
                fill_group.push(self.edge_form_none.clone());
                fill_group.push(Expr::normal(&graphics::FACE_FORM, [rgba_expr(fill_color)]));
                let mut stroke_group = stroke_directives(stroke_style, plot_width);
                for item in items {
                    fill_group.push(make_filled(item));
                    stroke_group.push(make_joined(item));
                }
                Some(Expr::list([
                    Expr::list(fill_group),
                    Expr::list(stroke_group),
                ]))
            }
            (None, Some(stroke_style)) => {
                let mut group = stroke_directives(stroke_style, plot_width);
                for item in items {
                    group.push(make_joined(item));
                }
                Some(Expr::list(group))
            }
            (None, None) => None,
        }
    }

    fn glyphs_to_expr(
        &self,
        glyphs: &[GlyphInstances],
        fill: Option<Rgba>,
        stroke: Option<&StrokeStyle>,
        plot_width: f64,
    ) -> Option<Expr> {
        let valid_inputs: Vec<&GlyphInstances> = glyphs
            .iter()
            .filter(|g| !g.positions.is_empty() && g.subpaths.iter().any(|s| !s.segments.is_empty()))
            .collect();
        if valid_inputs.is_empty() {
            return None;
        }

        let all_closed = valid_inputs
            .iter()
            .all(|g| g.subpaths.iter().all(|s| s.closed));
        let need_joined = stroke.is_some() && (fill.is_none() || !all_closed);

        let prepared: Vec<(&GlyphInstances, Expr, Option<Expr>)> = valid_inputs
            .into_iter()
            .map(|g| {
                let (filled, joined) = self.cached_glyph_expr(g, need_joined);
                (g, filled, joined)
            })
            .collect();

        self.styled_group(
            &prepared,
            fill,
            stroke,
            all_closed,
            plot_width,
            |(g, filled, _)| instanced_expr(filled.clone(), &g.positions),
            |(g, _, joined)| {
                instanced_expr(joined.clone().expect("joined curve built"), &g.positions)
            },
        )
    }

    fn paths_to_expr(
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

        // Fast path: solid fill or stroke-only.
        match fill {
            None => {
                return self.styled_group(
                    &valid_paths,
                    None,
                    stroke,
                    all_closed,
                    plot_width,
                    |sp| self.build_curve_exprs(sp, false).0,
                    |sp| self.build_joined_curve_expr(sp),
                );
            }
            Some(PaintStyle::Solid(c)) => {
                return self.styled_group(
                    &valid_paths,
                    Some(*c),
                    stroke,
                    all_closed,
                    plot_width,
                    |sp| self.build_curve_exprs(sp, false).0,
                    |sp| self.build_joined_curve_expr(sp),
                );
            }
            Some(paint) => {
                // Combine all contours into one FilledCurve so gradient/tiling
                // fills span the entire compound shape (or text run) cleanly.
                let combined: Vec<Subpath>;
                let all_subpaths: &[Subpath] = if valid_paths.len() == 1 {
                    valid_paths[0]
                } else {
                    combined = valid_paths.iter().flat_map(|s| s.iter().cloned()).collect();
                    &combined
                };
                let ctrl_bounds = subpaths_ctrl_bounds(all_subpaths)?;
                let (fill_dir, is_texture) = self.paint_fill_directive(paint, ctrl_bounds)?;
                if is_texture {
                    *has_texture = true;
                }
                let (filled_curve, _) = self.build_curve_exprs(all_subpaths, false);

                match stroke {
                    None => Some(Expr::list([
                        self.edge_form_none.clone(),
                        fill_dir,
                        filled_curve,
                    ])),
                    Some(stroke_style) if all_closed && !is_texture => Some(Expr::list([
                        fill_dir,
                        Expr::normal(
                            &graphics::EDGE_FORM,
                            [Expr::list(stroke_directives(stroke_style, plot_width))],
                        ),
                        filled_curve,
                    ])),
                    Some(stroke_style) => {
                        let fill_group = Expr::list([
                            self.edge_form_none.clone(),
                            fill_dir,
                            filled_curve,
                        ]);
                        let mut stroke_group = stroke_directives(stroke_style, plot_width);
                        for sp in &valid_paths {
                            stroke_group.push(self.build_joined_curve_expr(sp));
                        }
                        Some(Expr::list([fill_group, Expr::list(stroke_group)]))
                    }
                }
            }
        }
    }

    /// Encode a [`PaintStyle`] into a Wolfram fill directive plus a flag
    /// indicating whether the directive is `Texture[...]` (which should keep
    /// `EdgeForm[None]` separated from strokes and activate the modern 2D
    /// sRGB sentinel on the enclosing `Graphics`).
    fn paint_fill_directive(
        &self,
        paint: &PaintStyle,
        ctrl_bounds: [[f64; 2]; 2],
    ) -> Option<(Expr, bool)> {
        match paint {
            PaintStyle::Solid(c) => {
                Some((Expr::normal(&graphics::FACE_FORM, [rgba_expr(*c)]), false))
            }
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
            return Some((Expr::normal(&graphics::TEXTURE, [inner]), true));
        }

        // High-density tiling fallback (> MAX_VECTOR_TILES instances): build a
        // single cell graphic and delegate repetition to Wolfram's `PatternFilling`.
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
            Expr::normal(
                &graphics::PATTERN_FILLING,
                [tile_graphics, Expr::list([step_w, step_h])],
            ),
            false,
        ))
    }

    fn encode_subpaths(&self, subpaths: &[Subpath]) -> (Vec<Expr>, Vec<Expr>, Vec<i64>) {
        let mut all_segs = Vec::with_capacity(subpaths.len());
        let mut all_pts = Vec::with_capacity(subpaths.len());
        let mut closed_flags = Vec::with_capacity(subpaths.len());

        for sp in subpaths {
            if sp.segments.is_empty() {
                continue;
            }
            let mut seg_descs = Vec::with_capacity(sp.segments.len());
            let mut pts = Vec::with_capacity(sp.segments.len() * 3 + 1);
            pts.push(point_expr(sp.start));

            for (idx, seg) in sp.segments.iter().enumerate() {
                let is_first = idx == 0;
                match *seg {
                    Segment::Line(p) => {
                        seg_descs.push(if is_first {
                            self.seg_line_first.clone()
                        } else {
                            self.seg_line_cont.clone()
                        });
                        pts.push(point_expr(p));
                    }
                    Segment::Cubic(c1, c2, p) => {
                        seg_descs.push(if is_first {
                            self.seg_cubic_first.clone()
                        } else {
                            self.seg_cubic_cont.clone()
                        });
                        pts.push(point_expr(c1));
                        pts.push(point_expr(c2));
                        pts.push(point_expr(p));
                    }
                }
            }

            all_segs.push(Expr::list(seg_descs));
            all_pts.push(Expr::list(pts));
            closed_flags.push(if sp.closed { 1_i64 } else { 0_i64 });
        }

        (all_segs, all_pts, closed_flags)
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

fn gradient_fill_directive(grad: &GradientPaint, ctrl_bounds: [[f64; 2]; 2]) -> (Expr, bool) {
    let [[x0, x1], [y0, y1]] = ctrl_bounds;
    let cx = 0.5 * (x0 + x1);
    let cy = 0.5 * (y0 + y1);
    let m = (x1 - x0).abs().max((y1 - y0).abs()).max(1e-9);

    match grad {
        GradientPaint::Linear {
            origin,
            grad: gvec,
            stops,
            spread,
        } => {
            if stops.is_empty() {
                return (
                    Expr::normal(
                        &graphics::FACE_FORM,
                        [rgba_expr(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 })],
                    ),
                    false,
                );
            }
            let g_norm = gvec[0].hypot(gvec[1]);
            if g_norm <= 1e-12 {
                return (
                    Expr::normal(&graphics::FACE_FORM, [rgba_expr(stops[0].color)]),
                    false,
                );
            }
            let dx = gvec[0] / g_norm;
            let dy = gvec[1] / g_norm;
            let theta = dy.atan2(dx);
            let t_start = 0.5 + ((origin[0] - cx) * dx + (origin[1] - cy) * dy) / m;
            let span = 1.0 / (m * g_norm);
            let proj = project_linear_stops_to_unit(stops, t_start, span, *spread);
            let stops_rule = gradient_stops_rule(&proj.stops);
            let dir = if proj.spread == GradientSpread::Pad {
                Expr::normal(
                    &graphics::LINEAR_GRADIENT_FILLING,
                    [stops_rule, Expr::from(theta)],
                )
            } else {
                Expr::normal(
                    &graphics::LINEAR_GRADIENT_FILLING,
                    [
                        stops_rule,
                        Expr::from(theta),
                        Expr::string(wl_spread_name(proj.spread)),
                    ],
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
                return (
                    Expr::normal(
                        &graphics::FACE_FORM,
                        [rgba_expr(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 })],
                    ),
                    false,
                );
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
                let geom = Expr::list([Expr::list([u_c, v_c]), Expr::list([r_u, r_v])]);
                let dir = if *spread == GradientSpread::Pad {
                    Expr::normal(&graphics::RADIAL_GRADIENT_FILLING, [stops_rule, geom])
                } else {
                    Expr::normal(
                        &graphics::RADIAL_GRADIENT_FILLING,
                        [stops_rule, geom, Expr::string(wl_spread_name(*spread))],
                    )
                };
                return (dir, false);
            }

            // Off-center focal point, non-zero focal radius, or rotated/skewed ellipse:
            // render an exact smooth RGBA texture mapped 1:1 over `ctrl_bounds`.
            if let Some(tex) = gradient_texture_expr(grad, ctrl_bounds) {
                return (tex, true);
            }
            (
                Expr::normal(&graphics::FACE_FORM, [rgba_expr(stops[stops.len() / 2].color)]),
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
                return (
                    Expr::normal(
                        &graphics::FACE_FORM,
                        [rgba_expr(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 })],
                    ),
                    false,
                );
            }
            let [sx, ky, kx, sy, tx, ty] = *local_to_page;
            let cw_loc = container_size[0].abs().max(1e-9);
            let ch_loc = container_size[1].abs().max(1e-9);
            // Physical container (dx_pt, dy_pt) -> page linear map:
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
                    let rule = Expr::normal(&fundamental::RULE, [angles, colors]);
                    return (
                        Expr::normal(
                            &graphics::CONIC_GRADIENT_FILLING,
                            [rule, Expr::list([u_c, v_c])],
                        ),
                        false,
                    );
                }
            }

            if let Some(tex) = gradient_texture_expr(grad, ctrl_bounds) {
                return (tex, true);
            }
            (
                Expr::normal(&graphics::FACE_FORM, [rgba_expr(stops[stops.len() / 2].color)]),
                false,
            )
        }
    }
}

fn gradient_stops_rule(stops: &[GradientStop]) -> Expr {
    let offsets = Expr::list(stops.iter().map(|s| s.offset));
    let colors = Expr::list(stops.iter().map(|s| rgba_expr(s.color)));
    Expr::normal(&fundamental::RULE, [offsets, colors])
}

fn wl_spread_name(spread: GradientSpread) -> &'static str {
    match spread {
        GradientSpread::Pad => "Fixed",
        GradientSpread::Repeat => "Periodic",
        GradientSpread::Reflect => "Reflected",
    }
}

fn gradient_texture_expr(grad: &GradientPaint, ctrl_bounds: [[f64; 2]; 2]) -> Option<Expr> {
    let (w, h, pixels) = rasterize_gradient_texture(grad, ctrl_bounds, GRADIENT_TEXTURE_MAX_DIM)?;
    let mut rows = Vec::with_capacity(h);
    for iy in 0..h {
        let mut row = Vec::with_capacity(w);
        for ix in 0..w {
            let [r, g, b, a] = pixels[iy * w + ix];
            row.push(Expr::list([
                i64::from(r),
                i64::from(g),
                i64::from(b),
                i64::from(a),
            ]));
        }
        rows.push(Expr::list(row));
    }
    let img = Expr::normal(
        &graphics::IMAGE,
        [
            Expr::list(rows),
            Expr::string("Byte"),
            rule_sym_expr(&graphics::COLOR_SPACE, Expr::string("RGB")),
        ],
    );
    Some(Expr::normal(&graphics::TEXTURE, [img]))
}

fn rule_sym_expr(lhs: &Symbol, rhs: Expr) -> Expr {
    Expr::normal(&fundamental::RULE, [Expr::symbol(lhs), rhs])
}

fn rule_sym_sym(lhs: &Symbol, rhs: &Symbol) -> Expr {
    rule_sym_expr(lhs, Expr::symbol(rhs))
}

/// Canonical format name accepted by Wolfram Language's `ImportByteArray`.
fn wl_image_format(format: EmbeddedImageFormat) -> &'static str {
    match format {
        EmbeddedImageFormat::Png => "PNG",
        EmbeddedImageFormat::Jpg => "JPEG",
        EmbeddedImageFormat::Gif => "GIF",
        EmbeddedImageFormat::Webp => "WebP",
        EmbeddedImageFormat::Svg => "SVG",
    }
}

fn joined_curve_from_encoded(segs_expr: Expr, pts_expr: Expr, closed_flags: Vec<i64>) -> Expr {
    Expr::normal(
        &graphics::JOINED_CURVE,
        [
            segs_expr,
            pts_expr,
            rule_sym_expr(&graphics::CURVE_CLOSED, Expr::list(closed_flags)),
        ],
    )
}

fn instanced_expr(prim_expr: Expr, positions: &[[f64; 2]]) -> Expr {
    // Wolfram's `GeometricTransformation[prim, {{x0, y0}, {x1, y1}}]` parses a
    // 2×2 numeric list as an affine matrix instead of two translations, so wrap
    // length-2 translation lists as `{{{x0, y0}}, {{x1, y1}}}`.
    let translations = match positions {
        [single] => point_expr(*single),
        [p0, p1] => Expr::list([Expr::list([point_expr(*p0)]), Expr::list([point_expr(*p1)])]),
        many => Expr::list(many.iter().copied().map(point_expr)),
    };
    Expr::normal(&graphics::GEOMETRIC_TRANSFORMATION, [prim_expr, translations])
}

fn point_expr(p: [f64; 2]) -> Expr {
    Expr::list([p[0], p[1]])
}

fn rgba_expr(c: Rgba) -> Expr {
    if (c.a - 1.0).abs() <= 1e-6 {
        Expr::normal(&fundamental::RGB_COLOR, [c.r, c.g, c.b])
    } else {
        Expr::normal(&fundamental::RGB_COLOR, [c.r, c.g, c.b, c.a])
    }
}

fn stroke_directives(s: &StrokeStyle, plot_width: f64) -> Vec<Expr> {
    let mut out = Vec::with_capacity(5);
    out.push(rgba_expr(s.color));
    out.push(Expr::normal(&graphics::THICKNESS, [s.thickness / plot_width]));
    let cap_name = match s.cap {
        LineCapStyle::Butt => "Butt",
        LineCapStyle::Round => "Round",
        LineCapStyle::Square => "Square",
    };
    out.push(Expr::normal(&graphics::CAP_FORM, [cap_name]));

    let join_expr = match s.join {
        LineJoinStyle::Miter => Expr::normal(
            &graphics::JOIN_FORM,
            [Expr::list([Expr::from("Miter"), Expr::from(s.miter_limit)])],
        ),
        LineJoinStyle::Round => Expr::normal(&graphics::JOIN_FORM, ["Round"]),
        LineJoinStyle::Bevel => Expr::normal(&graphics::JOIN_FORM, ["Bevel"]),
    };
    out.push(join_expr);

    if let Some((ref pattern, phase)) = s.dash {
        if !pattern.is_empty() {
            let lengths = Expr::list(pattern.iter().map(|len| *len / plot_width));
            if phase.abs() <= 1e-9 && s.cap == LineCapStyle::Butt {
                out.push(Expr::normal(&graphics::DASHING, [lengths]));
            } else {
                out.push(Expr::normal(
                    &graphics::DASHING,
                    [lengths, Expr::from(phase / plot_width), Expr::from(cap_name)],
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{clear_glyph_cache, UnitGlyphKey, WindingRule};
    use typst::foundations::Bytes;

    #[test]
    fn encodes_filled_and_joined_curves_with_exact_options() {
        let subpath = Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line([10.0, 0.0]),
                Segment::Cubic([10.0, 5.0], [5.0, 10.0], [0.0, 10.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        };
        let page = VectorPage {
            size: [10.5, 10.25],
            bounds: [[0.0, 10.5], [0.0, 10.25]],
            primitives: vec![VectorPrimitive::Paths {
                paths: vec![vec![subpath]],
                fill: Some(PaintStyle::Solid(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                })),
                stroke: None,
                winding: WindingRule::NonZero,
            }],
        };

        let expr = page_to_expr(&page);
        let normal = expr.as_normal().expect("Graphics[...]");
        assert!(normal.has_head(&graphics::GRAPHICS));
        let rendered = expr.to_string();
        assert!(rendered.contains("System`FilledCurve"), "{rendered}");
        assert!(rendered.contains("System`PlotRange"), "{rendered}");
        assert!(rendered.contains("System`PlotRangePadding"), "{rendered}");
        assert!(rendered.contains("System`ImagePadding"), "{rendered}");
        // No Texture in page => no sRGB sentinel emitted.
        assert!(!rendered.contains("System`Polygon"), "{rendered}");
        // Fractional ImageSize preserved (no .ceil()).
        assert!(rendered.contains("10.5, 10.25"), "{rendered}");
    }

    #[test]
    fn encodes_rect_clip_path_clip_and_rotated_image_primitives() {
        let clipped_child = VectorPrimitive::Paths {
            paths: vec![vec![Subpath {
                start: [0.0, 0.0],
                segments: vec![
                    Segment::Line([40.0, 0.0]),
                    Segment::Line([40.0, 40.0]),
                    Segment::Line([0.0, 40.0]),
                    Segment::Line([0.0, 0.0]),
                ],
                closed: true,
            }]],
            fill: Some(PaintStyle::Solid(Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            })),
            stroke: None,
            winding: WindingRule::EvenOdd,
        };
        let rect_clip = VectorPrimitive::Clip {
            region: ClipRegion::Rect([[10.0, 30.0], [10.0, 25.0]]),
            children: vec![clipped_child.clone()],
        };
        let tri_clip = VectorPrimitive::Clip {
            region: ClipRegion::Path {
                subpaths: vec![Subpath {
                    start: [10.0, 10.0],
                    segments: vec![
                        Segment::Line([30.0, 10.0]),
                        Segment::Line([20.0, 30.0]),
                        Segment::Line([10.0, 10.0]),
                    ],
                    closed: true,
                }],
                bounds: [[10.0, 30.0], [10.0, 30.0]],
                ctrl_bounds: [[10.0, 30.0], [10.0, 30.0]],
            },
            children: vec![clipped_child],
        };
        let img_prim = VectorPrimitive::Image {
            format: EmbeddedImageFormat::Png,
            data: Bytes::new([0x89, b'P', b'N', b'G']),
            size: [20.0, 10.0],
            transform: [0.0, 1.0, -1.0, 0.0, 35.0, 5.0],
        };
        let page = VectorPage {
            size: [40.0, 40.0],
            bounds: [[0.0, 40.0], [0.0, 40.0]],
            primitives: vec![rect_clip, tri_clip, img_prim],
        };
        let rendered = page_to_expr(&page).to_string();
        assert!(rendered.contains("System`Inset"), "{rendered}");
        assert!(rendered.contains("System`Texture"), "{rendered}");
        assert!(rendered.contains("System`Polygon[{}]"), "{rendered}");
        assert!(rendered.contains("System`Background"), "{rendered}");
        assert!(
            rendered.contains("System`Left, System`Bottom"),
            "{rendered}"
        );
        assert!(rendered.contains("System`PlotRangeClipping"), "{rendered}");
        assert!(rendered.contains("System`True"), "{rendered}");
        assert!(rendered.contains("System`ImportByteArray"), "{rendered}");
        assert!(rendered.contains("\"PNG\""), "{rendered}");
        assert!(
            rendered.contains("System`GeometricTransformation"),
            "{rendered}"
        );
    }

    #[test]
    fn encodes_gradients_tiling_and_dash_phase_natively() {
        let box_sp = Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line([100.0, 0.0]),
                Segment::Line([100.0, 100.0]),
                Segment::Line([0.0, 100.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        };
        let stops = vec![
            GradientStop {
                offset: 0.0,
                color: Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 },
            },
            GradientStop {
                offset: 1.0,
                color: Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 },
            },
        ];
        let page = VectorPage {
            size: [100.0, 100.0],
            bounds: [[0.0, 100.0], [0.0, 100.0]],
            primitives: vec![
                VectorPrimitive::Paths {
                    paths: vec![vec![box_sp.clone()]],
                    fill: Some(PaintStyle::Gradient(GradientPaint::Linear {
                        origin: [0.0, 0.0],
                        grad: [0.01, 0.0],
                        stops: stops.clone(),
                        spread: GradientSpread::Pad,
                    })),
                    stroke: None,
                    winding: WindingRule::EvenOdd,
                },
                VectorPrimitive::Paths {
                    paths: vec![vec![box_sp.clone()]],
                    fill: Some(PaintStyle::Gradient(GradientPaint::Radial {
                        local_to_page: [100.0, 0.0, 0.0, -100.0, 0.0, 100.0],
                        center: [0.5, 0.5],
                        radius: 0.5,
                        focal_center: [0.5, 0.5],
                        focal_radius: 0.0,
                        stops: stops.clone(),
                        spread: GradientSpread::Pad,
                    })),
                    stroke: None,
                    winding: WindingRule::EvenOdd,
                },
                VectorPrimitive::Paths {
                    paths: vec![vec![box_sp]],
                    fill: Some(PaintStyle::Gradient(GradientPaint::Conic {
                        local_to_page: [100.0, 0.0, 0.0, -100.0, 0.0, 100.0],
                        container_size: [100.0, 100.0],
                        center: [0.5, 0.5],
                        angle: 0.0,
                        stops,
                    })),
                    stroke: None,
                    winding: WindingRule::EvenOdd,
                },
                VectorPrimitive::Line {
                    start: [0.0, 50.0],
                    end: [100.0, 50.0],
                    stroke: StrokeStyle {
                        color: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
                        thickness: 2.0,
                        cap: LineCapStyle::Round,
                        join: LineJoinStyle::Miter,
                        miter_limit: 4.0,
                        dash: Some((vec![10.0, 5.0], 3.0)),
                    },
                },
            ],
        };
        let rendered = page_to_expr(&page).to_string();
        assert!(rendered.contains("System`LinearGradientFilling"), "{rendered}");
        assert!(rendered.contains("System`RadialGradientFilling"), "{rendered}");
        assert!(rendered.contains("System`ConicGradientFilling"), "{rendered}");
        assert!(
            rendered.contains("System`Dashing[{0.1, 0.05}, 0.03, \"Round\"]"),
            "{rendered}"
        );
    }

    #[test]
    fn glyph_expr_cache_invalidates_on_world_cache_generation() {
        let key = GlyphKey {
            unit: UnitGlyphKey {
                font_gen: 1,
                font_hash: 0x1234_5678,
                face_index: 0,
                variations_hash: 0,
                glyph_id: 42,
            },
            linear_q: [10_000_000, 0, 0, 10_000_000],
        };
        let inst = GlyphInstances {
            key,
            subpaths: std::sync::Arc::new(vec![Subpath {
                start: [0.0, 0.0],
                segments: vec![
                    Segment::Line([5.0, 0.0]),
                    Segment::Line([5.0, 5.0]),
                    Segment::Line([0.0, 0.0]),
                ],
                closed: true,
            }]),
            positions: vec![[0.0, 0.0]],
        };

        let encoder = WlEncoder::new();
        let _ = encoder.cached_glyph_expr(&inst, false);
        assert!(lock_unpoisoned(&GLYPH_EXPR_CACHE).entries.contains_key(&key));
        clear_glyph_cache();
        // After cache clear, next lookup clears old generation entries and repopulates only current key.
        let (_, joined) = encoder.cached_glyph_expr(&inst, true);
        assert!(joined.is_some());
        assert_eq!(
            lock_unpoisoned(&GLYPH_EXPR_CACHE).generation,
            cache_generation()
        );
    }

    #[test]
    fn sub_point_page_preserves_image_size_aspect_ratio() {
        let page = VectorPage {
            size: [0.5, 0.25],
            bounds: [[0.0, 0.5], [0.0, 0.25]],
            primitives: Vec::new(),
        };
        let rendered = page_to_expr(&page).to_string();
        assert!(
            rendered.contains("0.5, 0.25"),
            "expected exact sub-pt ImageSize: {rendered}"
        );
    }
}
