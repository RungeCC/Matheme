//! Host-agnostic vector graphics extraction from a compiled Typst [`PagedDocument`].
//!
//! Walks each [`Page`]'s [`Frame`] tree directly, outlines glyphs via
//! `ttf-parser` using lightweight generation-tracked LRU caches, extracts
//! embedded SVGs via `usvg` and decoded rasters directly into raw pixel
//! buffers, resolves non-zero winding rules for even-odd hosts, computes tight
//! stroke/clip bounds via `kurbo`, and produces a [`VectorExport`].

mod glyphs;
mod image;
mod svg;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::num::NonZeroUsize;

use kurbo::{Affine, BezPath, Point, Rect, Shape as _, StrokeOpts};
use typst::foundations::Smart;
use typst::layout::{
    Abs, Frame, FrameItem, FrameKind, GroupItem, PageRanges, Ratio, Size, Transform,
};
use typst::visualize::{
    FixedStroke, Geometry, LineCap, LineJoin, Paint, RelativeTo, Shape, Tiling,
};
use typst_layout::{Page, PagedDocument};

use crate::geometry::{
    BoundsTracker, affine_from_typst, affine_stroke_scale, axis_aligned_rect_stroke_bbox,
    bez_path_hash, bez_path_to_subpaths, curve_to_bez_path, is_axis_aligned_rect,
    kurbo_stroke_from_style, line_stroke_bbox, rect_bez_path, strip_translation,
    stroke_bounding_box, stroke_hash, subpaths_ctrl_bounds,
};
use crate::paint::typst_gradient_to_paint;
use crate::winding::{self, WindingResolution};

pub use crate::ir::{
    ClipRegion, DegradationWarning, EmbeddedImageFormat, GlyphInstances, GlyphKey, ImagePayload,
    LineCapStyle, LineJoinStyle, PaintStyle, RasterPixels, Rgba, Segment, StrokeStyle, Subpath,
    TilingPaint, UnitGlyphKey, VectorExport, VectorPage, VectorPrimitive, WindingRule,
};
#[cfg(feature = "wl")]
pub(crate) use glyphs::GLYPH_CACHE_CAPACITY;
pub use glyphs::{cache_generation, clear_glyph_cache};

/// Maximum nested tiling recursion depth before falling back to solid gray.
const MAX_TILING_DEPTH: usize = 4;

#[derive(Default)]
struct DegradationWarnings {
    tiling_depth: bool,
    svg_feature: bool,
    winding: bool,
    unsupported_image: bool,
}

impl DegradationWarnings {
    fn into_warnings(self) -> Vec<DegradationWarning> {
        let mut out = Vec::new();
        if self.tiling_depth {
            out.push(DegradationWarning::TilingDepthExceeded);
        }
        if self.svg_feature {
            out.push(DegradationWarning::UnsupportedSvgFeature);
        }
        if self.winding {
            out.push(DegradationWarning::NonZeroWindingApproximated);
        }
        if self.unsupported_image {
            out.push(DegradationWarning::UnsupportedImageFormat);
        }
        out
    }
}

/// Export selected pages of a [`PagedDocument`] into [`VectorPage`]s plus
/// fidelity-degradation warnings, keyed against the caller's `font_generation`.
pub fn export_vector(
    document: &PagedDocument,
    pages: Option<&PageRanges>,
    font_generation: u64,
) -> VectorExport {
    glyphs::sync_font_generation(font_generation);
    let mut extractor = Extractor::new(font_generation);
    let exported_pages = document
        .pages()
        .iter()
        .enumerate()
        .filter(|(idx, _)| {
            pages.is_none_or(|ranges| {
                NonZeroUsize::new(idx + 1).is_some_and(|n| ranges.includes_page(n))
            })
        })
        .map(|(_, page)| extractor.extract_page(page))
        .collect();

    VectorExport {
        pages: exported_pages,
        warnings: extractor.warnings.into_warnings(),
    }
}

#[derive(Debug, Clone, Copy)]
struct ParentContext {
    size: Size,
    transform: Transform,
}

struct Extractor {
    font_gen: u64,
    /// Per-extraction fast memo for content-derived font hashes.
    font_content_hashes: HashMap<(usize, usize, u32), u64>,
    /// Per-extraction cache of scaled glyph contours at origin `(0, 0)`.
    local_scaled_cache: HashMap<GlyphKey, Option<glyphs::ScaledGlyph>>,
    /// Per-extraction cache of stroked glyph origin bounding boxes.
    stroked_glyph_bboxes: HashMap<(GlyphKey, u64), Rect>,
    /// Per-extraction cache of stroked curve bounding boxes `(path_hash, stroke_hash)`.
    stroked_path_bboxes: HashMap<(u64, u64), Rect>,
    /// Maps a [`GlyphKey`] to its slot inside the active trailing [`VectorPrimitive::Glyphs`].
    active_glyph_indices: HashMap<GlyphKey, usize>,
    /// Current nesting depth of tiling extraction (guards against deep recursion).
    tiling_depth: usize,
    warnings: DegradationWarnings,
}

impl Extractor {
    fn new(font_gen: u64) -> Self {
        Self {
            font_gen,
            font_content_hashes: HashMap::new(),
            local_scaled_cache: HashMap::new(),
            stroked_glyph_bboxes: HashMap::new(),
            stroked_path_bboxes: HashMap::new(),
            active_glyph_indices: HashMap::new(),
            tiling_depth: 0,
            warnings: DegradationWarnings::default(),
        }
    }

    pub(super) fn include_stroked_path(
        &mut self,
        bbox: &mut BoundsTracker,
        path: &BezPath,
        stroke: Option<&StrokeStyle>,
    ) {
        if path.elements().is_empty() {
            return;
        }
        let Some(style) = stroke.filter(|s| s.thickness > 0.0) else {
            bbox.include_rect(path.bounding_box());
            return;
        };
        let key = (bez_path_hash(path), stroke_hash(style));
        let r = if let Some(&cached) = self.stroked_path_bboxes.get(&key) {
            cached
        } else {
            let computed = stroke_bounding_box(path, style);
            self.stroked_path_bboxes.insert(key, computed);
            computed
        };
        bbox.include_rect(r);
    }

    pub(super) fn convert_typst_paint(
        &mut self,
        paint: &Paint,
        self_container_to_page: Affine,
        self_container_size: Size,
        self_tiling_to_page: Affine,
        parent: ParentContext,
        on_text: bool,
    ) -> PaintStyle {
        match paint {
            Paint::Solid(color) => PaintStyle::Solid(Rgba::from_color(color)),
            Paint::Gradient(gradient) => match gradient.unwrap_relative(on_text) {
                RelativeTo::Self_ => {
                    typst_gradient_to_paint(gradient, self_container_to_page, self_container_size)
                }
                RelativeTo::Parent => {
                    let pw = parent.size.x.to_pt().abs().max(1e-6);
                    let ph = parent.size.y.to_pt().abs().max(1e-6);
                    let parent_container =
                        affine_from_typst(parent.transform) * Affine::scale_non_uniform(pw, ph);
                    typst_gradient_to_paint(gradient, parent_container, parent.size)
                }
            },
            Paint::Tiling(tiling) => {
                let grid_to_page = match tiling.unwrap_relative(on_text) {
                    RelativeTo::Self_ => self_tiling_to_page,
                    RelativeTo::Parent => affine_from_typst(parent.transform),
                };
                self.extract_typst_tiling(tiling, grid_to_page)
            }
        }
    }

    fn extract_typst_tiling(&mut self, tiling: &Tiling, grid_to_page: Affine) -> PaintStyle {
        if self.tiling_depth >= MAX_TILING_DEPTH {
            self.warnings.tiling_depth = true;
            return PaintStyle::Solid(Rgba {
                r: 0.5,
                g: 0.5,
                b: 0.5,
                a: 1.0,
            });
        }

        let tile_size = tiling.size();
        let tw = tile_size.x.to_pt().abs().max(1e-6);
        let th = tile_size.y.to_pt().abs().max(1e-6);
        let step_x = (tile_size.x + tiling.spacing().x).to_pt();
        let step_y = (tile_size.y + tiling.spacing().y).to_pt();
        let ox = tiling.offset().x.to_pt();
        let oy = tiling.offset().y.to_pt();

        let linear = strip_translation(grid_to_page);
        let [sx, ky, kx, sy, _, _] = linear.as_coeffs();
        let tile_linear_ts = Transform {
            sx: Ratio::new(sx),
            ky: Ratio::new(ky),
            kx: Ratio::new(kx),
            sy: Ratio::new(sy),
            tx: Abs::zero(),
            ty: Abs::zero(),
        };
        let tile_parent = ParentContext {
            size: tile_size,
            transform: tile_linear_ts,
        };

        let saved_glyph_indices = std::mem::take(&mut self.active_glyph_indices);
        self.tiling_depth += 1;

        let mut tile_primitives = Vec::new();
        let mut tile_bbox = BoundsTracker::empty();
        self.extract_frame(
            tiling.frame(),
            tile_linear_ts,
            tile_parent,
            &mut tile_bbox,
            &mut tile_primitives,
        );

        self.tiling_depth -= 1;
        self.active_glyph_indices = saved_glyph_indices;

        tile_primitives = self.clip_overflowing_tile(tile_primitives, tile_bbox, tile_size, linear);

        PaintStyle::Tiling(Box::new(TilingPaint {
            primitives: tile_primitives,
            tile_size: [tw, th],
            step: [step_x, step_y],
            offset: [ox, oy],
            grid_to_page: grid_to_page.as_coeffs(),
        }))
    }

    /// Clip overflowing tile cell content to the cell's `[0..w] × [0..h]` box.
    pub(super) fn clip_overflowing_tile(
        &mut self,
        tile_primitives: Vec<VectorPrimitive>,
        tile_bbox: BoundsTracker,
        cell_size: Size,
        linear: Affine,
    ) -> Vec<VectorPrimitive> {
        let cell_bez = rect_bez_path(cell_size, linear);
        let cell_rect = cell_bez.bounding_box().abs();
        let overflows = tile_bbox.rect.is_some_and(|tb| {
            tb.x0 < cell_rect.x0 - 1e-3
                || tb.x1 > cell_rect.x1 + 1e-3
                || tb.y0 < cell_rect.y0 - 1e-3
                || tb.y1 > cell_rect.y1 + 1e-3
        });
        if overflows
            && !tile_primitives.is_empty()
            && cell_rect.width() > 0.0
            && cell_rect.height() > 0.0
        {
            let region = self.bez_to_clip_region(&cell_bez, cell_rect);
            vec![VectorPrimitive::Clip {
                region,
                children: tile_primitives,
            }]
        } else {
            tile_primitives
        }
    }

    pub(super) fn bez_to_clip_region(&mut self, clip_bez: &BezPath, clip_rect: Rect) -> ClipRegion {
        let bounds = [[clip_rect.x0, clip_rect.x1], [clip_rect.y0, clip_rect.y1]];
        if is_axis_aligned_rect(clip_bez) {
            return ClipRegion::Rect(bounds);
        }
        let raw_subpaths = bez_path_to_subpaths(clip_bez);
        let resolved = match winding::resolve_nonzero_subpaths(&raw_subpaths) {
            WindingResolution::Unchanged => raw_subpaths,
            WindingResolution::Resolved(r) => r,
            WindingResolution::BudgetExceeded | WindingResolution::ExtractionFailed => {
                self.warnings.winding = true;
                raw_subpaths
            }
        };
        let ctrl_bounds = subpaths_ctrl_bounds(&resolved).unwrap_or(bounds);
        ClipRegion::Path {
            subpaths: resolved,
            bounds,
            ctrl_bounds,
        }
    }

    fn extract_page(&mut self, page: &Page) -> VectorPage {
        self.active_glyph_indices.clear();
        let size = page.frame.size().max(Size::splat(Abs::pt(1.0)));
        let width = size.x.to_pt();
        let height = size.y.to_pt();

        // Flip from Typst's top-left Y-Down frame to Y-Up Cartesian points:
        // (x, y) -> (x, height - y).
        let root_ts = Transform::translate(Abs::zero(), size.y)
            .pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
        let root_affine = affine_from_typst(root_ts);
        let parent = ParentContext {
            size,
            transform: root_ts,
        };

        let mut bbox = BoundsTracker::from_page(width, height);
        let mut primitives = Vec::new();

        if let Smart::Custom(Some(ref fill_paint)) = page.fill {
            let rect_path = rect_bez_path(size, root_affine);
            let subpaths = bez_path_to_subpaths(&rect_path);
            if !subpaths.is_empty() {
                let bg = self.convert_typst_paint(
                    fill_paint,
                    root_affine * Affine::scale_non_uniform(width, height),
                    size,
                    root_affine,
                    parent,
                    false,
                );
                primitives.push(VectorPrimitive::Paths {
                    paths: vec![subpaths],
                    fill: Some(bg),
                    stroke: None,
                    winding: WindingRule::NonZero,
                });
            }
        }

        self.extract_frame(&page.frame, root_ts, parent, &mut bbox, &mut primitives);

        VectorPage {
            size: [width, height],
            bounds: bbox.into_bounds(),
            primitives,
        }
    }

    fn extract_frame(
        &mut self,
        frame: &Frame,
        ts: Transform,
        parent: ParentContext,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        for (pos, item) in frame.items() {
            let item_ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
            match item {
                FrameItem::Group(group) => {
                    self.extract_group(group, item_ts, parent, bbox, primitives)
                }
                FrameItem::Text(text) => self.extract_text(text, item_ts, parent, bbox, primitives),
                FrameItem::Shape(shape, _) => {
                    self.extract_shape(shape, item_ts, parent, bbox, primitives)
                }
                FrameItem::Image(image, size, _) => {
                    self.extract_image(image, size, item_ts, bbox, primitives)
                }
                FrameItem::Link(_, _) | FrameItem::Tag(_) => {}
            }
        }
    }

    fn extract_group(
        &mut self,
        group: &GroupItem,
        ts: Transform,
        parent: ParentContext,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        let group_ts = ts.pre_concat(group.transform);
        let child_parent = match group.frame.kind() {
            FrameKind::Hard => ParentContext {
                size: group.frame.size(),
                transform: group_ts,
            },
            FrameKind::Soft => parent,
        };
        let Some(ref clip_curve) = group.clip else {
            self.extract_frame(&group.frame, group_ts, child_parent, bbox, primitives);
            return;
        };

        let saved_glyph_indices = std::mem::take(&mut self.active_glyph_indices);

        let clip_bez = curve_to_bez_path(clip_curve, affine_from_typst(group_ts));
        let clip_rect = clip_bez.bounding_box().abs();
        let region = self.bez_to_clip_region(&clip_bez, clip_rect);

        let mut child_primitives = Vec::new();
        let mut child_bbox = BoundsTracker::empty();
        self.extract_frame(
            &group.frame,
            group_ts,
            child_parent,
            &mut child_bbox,
            &mut child_primitives,
        );
        self.active_glyph_indices.clear();

        if let Some(cb) = child_bbox.rect {
            let visible = cb.intersect(clip_rect);
            if visible.width() > 0.0 && visible.height() > 0.0 {
                bbox.include_rect(visible);
            }
        }

        if !child_primitives.is_empty() && clip_rect.width() > 0.0 && clip_rect.height() > 0.0 {
            primitives.push(VectorPrimitive::Clip {
                region,
                children: child_primitives,
            });
        } else {
            self.active_glyph_indices = saved_glyph_indices;
        }
    }

    pub(super) fn extract_shape(
        &mut self,
        shape: &Shape,
        ts: Transform,
        parent: ParentContext,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        self.active_glyph_indices.clear();
        if shape.fill.is_none() && shape.stroke.is_none() {
            return;
        }

        let affine = affine_from_typst(ts);
        let stroke_scale = affine_stroke_scale(affine);

        let fill = shape.fill.as_ref().map(|p| {
            let (self_container, self_size) = shape_self_container(shape, affine, false);
            self.convert_typst_paint(p, self_container, self_size, affine, parent, false)
        });

        let (solid_stroke, painted_stroke) = match shape.stroke.as_ref() {
            None => (None, None),
            Some(s) if matches!(s.paint, Paint::Solid(_)) => {
                (Some(convert_solid_stroke(s, stroke_scale)), None)
            }
            Some(s) => {
                let (self_container, self_size) = shape_self_container(shape, affine, true);
                let paint = self.convert_typst_paint(
                    &s.paint,
                    self_container,
                    self_size,
                    affine,
                    parent,
                    false,
                );
                (
                    None,
                    Some((convert_stroke_geometry(s, stroke_scale), paint)),
                )
            }
        };

        let bbox_stroke = solid_stroke
            .as_ref()
            .or_else(|| painted_stroke.as_ref().map(|(s, _)| s));

        match &shape.geometry {
            Geometry::Line(end) => {
                let start_pt = affine * Point::ZERO;
                let end_pt = affine * Point::new(end.x.to_pt(), end.y.to_pt());
                if let Some(stroke) = solid_stroke {
                    if stroke.thickness > 0.0 {
                        bbox.include_rect(line_stroke_bbox(start_pt, end_pt, &stroke));
                    } else {
                        bbox.include_point(start_pt);
                        bbox.include_point(end_pt);
                    }
                    primitives.push(VectorPrimitive::Line {
                        start: [start_pt.x, start_pt.y],
                        end: [end_pt.x, end_pt.y],
                        stroke,
                    });
                } else if let Some((stroke_style, stroke_paint)) = painted_stroke
                    && stroke_style.thickness > 0.0
                {
                    bbox.include_rect(line_stroke_bbox(start_pt, end_pt, &stroke_style));
                    let mut line_bez = BezPath::new();
                    line_bez.move_to(start_pt);
                    line_bez.line_to(end_pt);
                    push_painted_stroke(
                        primitives,
                        &line_bez,
                        &stroke_style,
                        stroke_paint,
                        &mut self.warnings,
                    );
                }
            }
            Geometry::Rect(size) => {
                let bez = rect_bez_path(*size, affine);
                let [_, ky, kx, _, _, _] = affine.as_coeffs();
                if kx.abs() <= 1e-9 && ky.abs() <= 1e-9 {
                    let base_rect = bez.bounding_box().abs();
                    let r = match bbox_stroke.filter(|s| s.thickness > 0.0) {
                        Some(style) => axis_aligned_rect_stroke_bbox(base_rect, style),
                        None => base_rect,
                    };
                    bbox.include_rect(r);
                } else {
                    self.include_stroked_path(bbox, &bez, bbox_stroke);
                }
                let subpaths = bez_path_to_subpaths(&bez);
                if !subpaths.is_empty() && (fill.is_some() || solid_stroke.is_some()) {
                    primitives.push(VectorPrimitive::Paths {
                        paths: vec![subpaths],
                        fill,
                        stroke: solid_stroke,
                        winding: shape.fill_rule.into(),
                    });
                }
                if let Some((stroke_style, stroke_paint)) = painted_stroke {
                    push_painted_stroke(
                        primitives,
                        &bez,
                        &stroke_style,
                        stroke_paint,
                        &mut self.warnings,
                    );
                }
            }
            Geometry::Curve(curve) => {
                let bez = curve_to_bez_path(curve, affine);
                self.include_stroked_path(bbox, &bez, bbox_stroke);
                let subpaths = bez_path_to_subpaths(&bez);
                if fill.is_some() || solid_stroke.is_some() {
                    push_resolved_paths(
                        primitives,
                        subpaths,
                        fill,
                        solid_stroke,
                        shape.fill_rule.into(),
                        &mut self.warnings,
                    );
                }
                if let Some((stroke_style, stroke_paint)) = painted_stroke {
                    push_painted_stroke(
                        primitives,
                        &bez,
                        &stroke_style,
                        stroke_paint,
                        &mut self.warnings,
                    );
                }
            }
        }
    }
}

fn push_resolved_paths(
    primitives: &mut Vec<VectorPrimitive>,
    subpaths: Vec<Subpath>,
    fill: Option<PaintStyle>,
    stroke: Option<StrokeStyle>,
    winding: WindingRule,
    warnings: &mut DegradationWarnings,
) {
    if subpaths.is_empty() {
        return;
    }
    if fill.is_some() && winding == WindingRule::NonZero {
        match winding::resolve_nonzero_subpaths(&subpaths) {
            WindingResolution::Unchanged => {}
            WindingResolution::Resolved(resolved) => {
                if !resolved.is_empty() {
                    primitives.push(VectorPrimitive::Paths {
                        paths: vec![resolved],
                        fill,
                        stroke: None,
                        winding: WindingRule::EvenOdd,
                    });
                }
                if stroke.is_some() {
                    primitives.push(VectorPrimitive::Paths {
                        paths: vec![subpaths],
                        fill: None,
                        stroke,
                        winding,
                    });
                }
                return;
            }
            WindingResolution::BudgetExceeded | WindingResolution::ExtractionFailed => {
                warnings.winding = true;
            }
        }
    }
    primitives.push(VectorPrimitive::Paths {
        paths: vec![subpaths],
        fill,
        stroke,
        winding,
    });
}

/// Expand a non-solid (gradient or tiling) stroked path into a filled outline
/// using `kurbo::dash` + `kurbo::stroke`, resolving non-zero winding so
/// gradient/tiling strokes render with full fidelity.
fn push_painted_stroke(
    primitives: &mut Vec<VectorPrimitive>,
    path: &BezPath,
    style: &StrokeStyle,
    paint: PaintStyle,
    warnings: &mut DegradationWarnings,
) {
    if path.elements().is_empty() || style.thickness <= 0.0 {
        return;
    }
    let kstroke = kurbo_stroke_from_style(style);
    let outlined = match style.dash.as_ref() {
        Some((pattern, phase)) if !pattern.is_empty() && pattern.iter().any(|&v| v > 0.0) => {
            let dashed = kurbo::dash(path.iter(), *phase, pattern);
            kurbo::stroke(dashed, &kstroke, &StrokeOpts::default(), 0.1)
        }
        _ => kurbo::stroke(path.iter(), &kstroke, &StrokeOpts::default(), 0.1),
    };
    let subpaths = bez_path_to_subpaths(&outlined);
    push_resolved_paths(
        primitives,
        subpaths,
        Some(paint),
        None,
        WindingRule::NonZero,
        warnings,
    );
}

fn shape_self_container(
    shape: &Shape,
    item_affine: Affine,
    include_stroke: bool,
) -> (Affine, Size) {
    let bbox = shape.bbox(include_stroke);
    let mut offset = bbox.min;
    let mut size = bbox.size();
    if let Geometry::Rect(rect) = shape.geometry {
        if rect.x.signum() < 1.0 {
            offset.x += size.x;
            size.x *= -1.0;
        }
        if rect.y.signum() < 1.0 {
            offset.y += size.y;
            size.y *= -1.0;
        }
    }
    if size.x == Abs::zero() {
        size.x = Abs::pt(1.0);
    }
    if size.y == Abs::zero() {
        size.y = Abs::pt(1.0);
    }
    let container_to_page = item_affine
        * Affine::translate((offset.x.to_pt(), offset.y.to_pt()))
        * Affine::scale_non_uniform(size.x.to_pt(), size.y.to_pt());
    (container_to_page, Size::new(size.x.abs(), size.y.abs()))
}

pub(super) fn convert_stroke_geometry(stroke: &FixedStroke, scale: f64) -> StrokeStyle {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dash = stroke.dash.as_ref().map(|d| {
        let phase = d.phase.to_pt() * s;
        (d.array.iter().map(|len| len.to_pt() * s).collect(), phase)
    });
    StrokeStyle {
        color: Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
        thickness: (stroke.thickness.to_pt() * s).max(0.0),
        cap: match stroke.cap {
            LineCap::Butt => LineCapStyle::Butt,
            LineCap::Round => LineCapStyle::Round,
            LineCap::Square => LineCapStyle::Square,
        },
        join: match stroke.join {
            LineJoin::Miter => LineJoinStyle::Miter,
            LineJoin::Round => LineJoinStyle::Round,
            LineJoin::Bevel => LineJoinStyle::Bevel,
        },
        miter_limit: stroke.miter_limit.get(),
        dash,
    }
}

pub(super) fn convert_solid_stroke(stroke: &FixedStroke, scale: f64) -> StrokeStyle {
    let mut style = convert_stroke_geometry(stroke, scale);
    if let Paint::Solid(ref color) = stroke.paint {
        style.color = Rgba::from_color(color);
    }
    style
}
