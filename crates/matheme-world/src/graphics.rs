//! Host-agnostic vector graphics extraction from a compiled Typst [`PagedDocument`].
//!
//! Walks each [`Page`]'s [`Frame`] tree directly, outlines glyphs via
//! `ttf-parser` using lightweight generation-tracked caches (never pinning
//! font byte buffers), extracts embedded SVGs via `usvg`, resolves non-zero
//! winding rules for even-odd hosts, computes tight stroke/clip bounds via
//! `kurbo`, and produces a [`VectorExport`] containing [`VectorPage`]s and any
//! fidelity-degradation [`Diagnostic`] warnings.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

use kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, QuadBez, Rect, Shape as _, Stroke, StrokeOpts};
use ttf_parser::{GlyphId, OutlineBuilder};
use typst::foundations::{Bytes, Smart};
use typst::layout::{Abs, Axes, Frame, FrameItem, GroupItem, PageRanges, Ratio, Size, Transform};
use typst::text::color::{glyph_frame, should_outline, GlyphFrameItem};
use typst::text::{FontInstance, TextItem};
use typst::visualize::{
    Color, Curve, CurveItem, FillRule, FixedStroke, Geometry, Image, ImageKind, LineCap, LineJoin,
    Paint, RatioOrAngle, Shape,
};
use typst_layout::{Page, PagedDocument};
use typst_svg::WebImage;
use usvg::tiny_skia_path;

use crate::diagnostics::{self, Diagnostic};
use crate::winding;

/// Upper bound on process-wide normalized glyph outline entries to bound
/// memory growth in long-running sessions.
const MAX_UNIT_GLYPH_ENTRIES: usize = 8192;

/// Scale factor used to quantize linear transform components (`1e-6` precision)
/// so floating-point noise never pollutes the glyph key space.
const LINEAR_QUANT_SCALE: f64 = 1_000_000.0;

/// RGBA color with components in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub fn from_color(color: &Color) -> Self {
        let rgb = color.to_rgb();
        Self {
            r: f64::from(rgb.red),
            g: f64::from(rgb.green),
            b: f64::from(rgb.blue),
            a: f64::from(rgb.alpha),
        }
    }

    fn from_paint(paint: &Paint, warnings: &mut DegradationWarnings) -> Self {
        match paint {
            Paint::Solid(color) => Self::from_color(color),
            Paint::Gradient(gradient) => {
                warnings.gradient = true;
                Self::from_color(&gradient.sample(RatioOrAngle::Ratio(Ratio::new(0.5))))
            }
            Paint::Tiling(_) => {
                warnings.tiling = true;
                Self {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCapStyle {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoinStyle {
    Miter,
    Round,
    Bevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindingRule {
    NonZero,
    EvenOdd,
}

impl From<FillRule> for WindingRule {
    fn from(rule: FillRule) -> Self {
        match rule {
            FillRule::NonZero => Self::NonZero,
            FillRule::EvenOdd => Self::EvenOdd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedImageFormat {
    Png,
    Jpg,
    Gif,
    Webp,
    Svg,
}

impl EmbeddedImageFormat {
    fn from_extension(ext: &str) -> Self {
        match ext {
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpg,
            "gif" => Self::Gif,
            "webp" => Self::Webp,
            "svg" => Self::Svg,
            _ => Self::Png,
        }
    }

    /// Canonical format name accepted by Wolfram Language's `ImportByteArray`.
    pub fn wl_format(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpg => "JPEG",
            Self::Gif => "GIF",
            Self::Webp => "WebP",
            Self::Svg => "SVG",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub color: Rgba,
    /// Line thickness in Typst points (`pt`).
    pub thickness: f64,
    pub cap: LineCapStyle,
    pub join: LineJoinStyle,
    pub miter_limit: f64,
    /// Optional `(dash_array_in_pt, phase_in_pt)`.
    pub dash: Option<(Vec<f64>, f64)>,
}

/// One segment of a [`Subpath`], starting from the previous endpoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Line([f64; 2]),
    Cubic([f64; 2], [f64; 2], [f64; 2]),
}

/// A connected contour of line and cubic Bézier segments.
#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    pub start: [f64; 2],
    pub segments: Vec<Segment>,
    pub closed: bool,
}

/// Lightweight, non-owning key identifying a normalized unit-em glyph outline
/// within a live font generation. Holds zero `Font` / byte-buffer references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitGlyphKey {
    pub font_gen: u64,
    pub data_ptr: usize,
    pub data_len: usize,
    pub face_index: u32,
    pub variations_hash: u64,
    pub glyph_id: u16,
}

impl UnitGlyphKey {
    fn from_instance(font: &FontInstance, glyph_id: u16) -> Self {
        let mut hasher = DefaultHasher::new();
        font.variations().hash(&mut hasher);
        let base = font.font();
        Self {
            font_gen: crate::fonts::font_generation(),
            data_ptr: base.data().as_ptr() as usize,
            data_len: base.data().len(),
            face_index: base.index(),
            variations_hash: hasher.finish(),
            glyph_id,
        }
    }
}

/// Identifies a specific glyph under a specific quantized 2x2 linear
/// transformation, invariant to page translation `(tx, ty)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub unit: UnitGlyphKey,
    /// Quantized `[sx, ky, kx, sy]` scaled by [`LINEAR_QUANT_SCALE`].
    pub linear_q: [i64; 4],
}

impl GlyphKey {
    fn new(unit: UnitGlyphKey, linear: Affine) -> Self {
        let [sx, ky, kx, sy, _, _] = linear.as_coeffs();
        let q = |v: f64| -> i64 {
            if !v.is_finite() {
                return 0;
            }
            let scaled = (v * LINEAR_QUANT_SCALE).round();
            if scaled == 0.0 {
                0
            } else {
                scaled.clamp(i64::MIN as f64, i64::MAX as f64) as i64
            }
        };
        Self {
            unit,
            linear_q: [q(sx), q(ky), q(kx), q(sy)],
        }
    }
}

/// All occurrences of a single [`GlyphKey`] within a coalesced glyph run,
/// sharing one origin-centered outline (`subpaths`) and a list of `[tx, ty]`
/// page translation vectors.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphInstances {
    pub key: GlyphKey,
    pub subpaths: Arc<Vec<Subpath>>,
    pub positions: Vec<[f64; 2]>,
}

/// Scaled glyph outline cached for the lifetime of one [`export_vector`] call.
#[derive(Debug, Clone)]
struct ScaledGlyph {
    bez_path: Arc<BezPath>,
    subpaths: Arc<Vec<Subpath>>,
    fill_bbox: Rect,
}

type UnitGlyphMap = HashMap<UnitGlyphKey, Option<Arc<Vec<Subpath>>>>;

static CACHE_GENERATION: AtomicU64 = AtomicU64::new(1);

static UNIT_GLYPH_CACHE: LazyLock<RwLock<UnitGlyphMap>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Current glyph-cache generation, incremented every time [`clear_glyph_cache`] runs.
pub fn cache_generation() -> u64 {
    CACHE_GENERATION.load(Ordering::Acquire)
}

/// Drop all cached unit glyph outlines and advance [`cache_generation`].
pub fn clear_glyph_cache() {
    if let Ok(mut guard) = UNIT_GLYPH_CACHE.write() {
        guard.clear();
    }
    CACHE_GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// A host-neutral vector/raster drawing primitive on a [`VectorPage`].
#[derive(Debug, Clone, PartialEq)]
pub enum VectorPrimitive {
    /// Instanced text glyphs sharing the same fill and stroke.
    Glyphs {
        glyphs: Vec<GlyphInstances>,
        fill: Option<Rgba>,
        stroke: Option<StrokeStyle>,
    },
    /// One or more compound paths (e.g. a `Shape` or flattened SVG path).
    Paths {
        paths: Vec<Vec<Subpath>>,
        fill: Option<Rgba>,
        stroke: Option<StrokeStyle>,
        winding: WindingRule,
    },
    /// A stroked two-point line segment.
    Line {
        start: [f64; 2],
        end: [f64; 2],
        stroke: StrokeStyle,
    },
    /// An embedded image with local `[0..size[0]] × [0..size[1]]` Y-Up frame
    /// and column-major affine `[sx, ky, kx, sy, tx, ty]` mapping into page points.
    Image {
        format: EmbeddedImageFormat,
        data: Bytes,
        size: [f64; 2],
        transform: [f64; 6],
    },
    /// Axis-aligned rectangular clip region enclosing nested primitives.
    Clip {
        bounds: [[f64; 2]; 2],
        children: Vec<VectorPrimitive>,
    },
}

/// One exported page in Y-Up point coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorPage {
    /// Nominal Typst page `[width, height]` in points.
    pub size: [f64; 2],
    /// Union of the page box `[[0, width], [0, height]]` and all visible content
    /// (including stroke envelopes and clipped children), `[[x_min, x_max], [y_min, y_max]]`.
    pub bounds: [[f64; 2]; 2],
    pub primitives: Vec<VectorPrimitive>,
}

/// Complete vector export output plus any fidelity-degradation warnings.
#[derive(Debug, Clone)]
pub struct VectorExport {
    pub pages: Vec<VectorPage>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
struct DegradationWarnings {
    gradient: bool,
    tiling: bool,
    dash_phase: bool,
    non_rect_clip: bool,
    svg_feature: bool,
}

impl DegradationWarnings {
    fn into_diagnostics(self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if self.gradient {
            out.push(diagnostics::warning(
                "Gradient fills/strokes are approximated by midpoint solid colors in Graphics output.",
            ));
        }
        if self.tiling {
            out.push(diagnostics::warning(
                "Tiling patterns are approximated by solid gray in Graphics output.",
            ));
        }
        if self.dash_phase {
            out.push(diagnostics::warning(
                "Non-zero stroke dash phases are ignored in Graphics output.",
            ));
        }
        if self.non_rect_clip {
            out.push(diagnostics::warning(
                "Non-rectangular clip paths are approximated by their axis-aligned bounding box in Graphics output.",
            ));
        }
        if self.svg_feature {
            out.push(diagnostics::warning(
                "Unsupported SVG features (filters/masks) inside embedded SVGs are ignored in Graphics output.",
            ));
        }
        out
    }
}

/// Export selected pages of a [`PagedDocument`] into [`VectorPage`]s plus
/// fidelity-degradation warnings.
pub fn export_vector(document: &PagedDocument, pages: Option<&PageRanges>) -> VectorExport {
    let mut extractor = Extractor::default();
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
        diagnostics: extractor.warnings.into_diagnostics(),
    }
}

#[derive(Debug, Clone, Copy)]
struct BoundsTracker {
    rect: Option<Rect>,
}

impl BoundsTracker {
    fn empty() -> Self {
        Self { rect: None }
    }

    fn from_page(width: f64, height: f64) -> Self {
        Self {
            rect: Some(Rect::new(0.0, 0.0, width, height)),
        }
    }

    fn include_point(&mut self, p: Point) {
        if p.x.is_finite() && p.y.is_finite() {
            self.rect = Some(match self.rect {
                Some(r) => r.union_pt(p),
                None => Rect::from_points(p, p),
            });
        }
    }

    fn include_rect(&mut self, r: Rect) {
        let r = r.abs();
        if r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite() {
            self.rect = Some(match self.rect {
                Some(cur) => cur.union(r),
                None => r,
            });
        }
    }

    fn include_path(&mut self, path: &BezPath, stroke: Option<&StrokeStyle>) {
        if path.elements().is_empty() {
            return;
        }
        let bbox = match stroke.filter(|s| s.thickness > 0.0) {
            Some(style) => stroke_bounding_box(path, style),
            None => path.bounding_box(),
        };
        self.include_rect(bbox);
    }

    fn into_bounds(self) -> [[f64; 2]; 2] {
        match self.rect {
            Some(r) => [[r.x0, r.x1], [r.y0, r.y1]],
            None => [[0.0, 1.0], [0.0, 1.0]],
        }
    }
}

#[derive(Default)]
struct Extractor {
    /// Per-extraction L1 cache of scaled glyph contours at origin `(0, 0)`.
    local_scaled_cache: HashMap<GlyphKey, Option<ScaledGlyph>>,
    /// Per-extraction cache of stroked glyph origin bounding boxes `[dx0, dy0, dx1, dy1]`.
    stroked_glyph_bboxes: HashMap<(GlyphKey, u64), Rect>,
    /// Maps a [`GlyphKey`] to its slot inside the active trailing [`VectorPrimitive::Glyphs`].
    active_glyph_indices: HashMap<GlyphKey, usize>,
    warnings: DegradationWarnings,
}

impl Extractor {
    fn extract_page(&mut self, page: &Page) -> VectorPage {
        self.active_glyph_indices.clear();
        let size = page.frame.size().max(Size::splat(Abs::pt(1.0)));
        let width = size.x.to_pt();
        let height = size.y.to_pt();

        let background = match &page.fill {
            Smart::Auto => None,
            Smart::Custom(fill) => fill.as_ref().map(|p| Rgba::from_paint(p, &mut self.warnings)),
        };

        // Flip from Typst's top-left Y-Down frame to Y-Up Cartesian points:
        // (x, y) -> (x, height - y).
        let root_ts = Transform::translate(Abs::zero(), size.y)
            .pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));

        let mut bbox = BoundsTracker::from_page(width, height);
        let mut primitives = Vec::new();

        if let Some(bg) = background {
            let rect_path = rect_bez_path(size, affine_from_typst(root_ts));
            let subpaths = bez_path_to_subpaths(&rect_path);
            if !subpaths.is_empty() {
                primitives.push(VectorPrimitive::Paths {
                    paths: vec![subpaths],
                    fill: Some(bg),
                    stroke: None,
                    winding: WindingRule::NonZero,
                });
            }
        }

        self.extract_frame(&page.frame, root_ts, &mut bbox, &mut primitives);

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
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        for (pos, item) in frame.items() {
            let item_ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
            match item {
                FrameItem::Group(group) => self.extract_group(group, item_ts, bbox, primitives),
                FrameItem::Text(text) => self.extract_text(text, item_ts, bbox, primitives),
                FrameItem::Shape(shape, _) => self.extract_shape(shape, item_ts, bbox, primitives),
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
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        let group_ts = ts.pre_concat(group.transform);
        let Some(ref clip_curve) = group.clip else {
            self.extract_frame(&group.frame, group_ts, bbox, primitives);
            return;
        };

        self.active_glyph_indices.clear();

        let clip_bez = curve_to_bez_path(clip_curve, affine_from_typst(group_ts));
        let clip_rect = clip_bez.bounding_box().abs();
        if !is_axis_aligned_rect(&clip_bez) {
            self.warnings.non_rect_clip = true;
        }

        let mut child_primitives = Vec::new();
        let mut child_bbox = BoundsTracker::empty();
        self.extract_frame(&group.frame, group_ts, &mut child_bbox, &mut child_primitives);
        self.active_glyph_indices.clear();

        if let Some(cb) = child_bbox.rect {
            let visible = cb.intersect(clip_rect);
            if visible.width() > 0.0 && visible.height() > 0.0 {
                bbox.include_rect(visible);
            }
        }

        if !child_primitives.is_empty() && clip_rect.width() > 0.0 && clip_rect.height() > 0.0 {
            primitives.push(VectorPrimitive::Clip {
                bounds: [[clip_rect.x0, clip_rect.x1], [clip_rect.y0, clip_rect.y1]],
                children: child_primitives,
            });
        }
    }

    fn push_glyph_instance(
        &mut self,
        primitives: &mut Vec<VectorPrimitive>,
        fill: Option<Rgba>,
        stroke: &Option<StrokeStyle>,
        key: GlyphKey,
        subpaths: Arc<Vec<Subpath>>,
        pos: [f64; 2],
    ) {
        let matches_last = matches!(
            primitives.last(),
            Some(VectorPrimitive::Glyphs {
                fill: last_fill,
                stroke: last_stroke,
                ..
            }) if *last_fill == fill && last_stroke == stroke
        );
        if !matches_last {
            self.active_glyph_indices.clear();
            primitives.push(VectorPrimitive::Glyphs {
                glyphs: Vec::new(),
                fill,
                stroke: stroke.clone(),
            });
        }
        let Some(VectorPrimitive::Glyphs { glyphs, .. }) = primitives.last_mut() else {
            unreachable!();
        };
        if let Some(&idx) = self.active_glyph_indices.get(&key) {
            glyphs[idx].positions.push(pos);
        } else {
            let idx = glyphs.len();
            glyphs.push(GlyphInstances {
                key,
                subpaths,
                positions: vec![pos],
            });
            self.active_glyph_indices.insert(key, idx);
        }
    }

    fn extract_text(
        &mut self,
        text: &TextItem,
        ts: Transform,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        // Font outlines are defined in Y-Up, whereas Typst's local text frame
        // is Y-Down. Flip Y locally so composed with `root_ts`, glyphs end up Y-Up.
        let text_ts = ts.pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
        let font_size_pt = text.size.to_pt();
        let text_affine = affine_from_typst(text_ts);

        let fill = Some(Rgba::from_paint(&text.fill, &mut self.warnings));
        let stroke = text
            .stroke
            .as_ref()
            .map(|s| convert_stroke(s, affine_stroke_scale(text_affine), &mut self.warnings));

        let scale_ts = Transform::scale(Ratio::new(font_size_pt), Ratio::new(font_size_pt));
        let linear_affine =
            strip_translation(affine_from_typst(text_ts.pre_concat(scale_ts)));

        let mut x = Abs::zero();
        let mut y = Abs::zero();

        for glyph in &text.glyphs {
            let id = GlyphId(glyph.id);
            let x_offset = x + glyph.x_offset.at(text.size);
            let y_offset = y + glyph.y_offset.at(text.size);

            if should_outline(&text.font, id) {
                let unit_key = UnitGlyphKey::from_instance(&text.font, id.0);
                let key = GlyphKey::new(unit_key, linear_affine);
                if let Some(scaled) = self.scaled_glyph(&text.font, key, linear_affine) {
                    let pos = text_affine * Point::new(x_offset.to_pt(), y_offset.to_pt());
                    let glyph_bbox = self.glyph_origin_bbox(key, &scaled, stroke.as_ref());
                    bbox.include_rect(glyph_bbox + pos.to_vec2());
                    self.push_glyph_instance(
                        primitives,
                        fill,
                        &stroke,
                        key,
                        scaled.subpaths,
                        [pos.x, pos.y],
                    );
                }
            } else if let Some(gframe) = glyph_frame(&text.font, id.0) {
                let scale = Ratio::new(font_size_pt / text.font.units_per_em());
                let item_pos = gframe.item.pos();
                let glyph_ts = text_ts
                    .pre_concat(Transform::translate(x_offset, y_offset))
                    .pre_concat(Transform::scale(scale, -scale))
                    .pre_concat(Transform::translate(item_pos.x, item_pos.y));
                match &gframe.item {
                    GlyphFrameItem::Tofu(_, shape) => {
                        self.extract_shape(shape, glyph_ts, bbox, primitives);
                    }
                    GlyphFrameItem::Image(_, image, size) => {
                        self.extract_image(image, size, glyph_ts, bbox, primitives);
                    }
                }
            }

            x += glyph.x_advance.at(text.size);
            y += glyph.y_advance.at(text.size);
        }
    }

    fn glyph_origin_bbox(
        &mut self,
        key: GlyphKey,
        scaled: &ScaledGlyph,
        stroke: Option<&StrokeStyle>,
    ) -> Rect {
        let Some(style) = stroke.filter(|s| s.thickness > 0.0) else {
            return scaled.fill_bbox;
        };
        let s_key = (key, stroke_hash(style));
        if let Some(&cached) = self.stroked_glyph_bboxes.get(&s_key) {
            return cached;
        }
        let stroked = stroke_bounding_box(&scaled.bez_path, style);
        self.stroked_glyph_bboxes.insert(s_key, stroked);
        stroked
    }

    fn scaled_glyph(
        &mut self,
        font: &FontInstance,
        key: GlyphKey,
        linear: Affine,
    ) -> Option<ScaledGlyph> {
        if let Some(cached) = self.local_scaled_cache.get(&key) {
            return cached.clone();
        }
        let computed = outline_glyph_unit(font, key.unit).and_then(|unit_subpaths| {
            let mut bez_path = subpaths_to_bez_path(&unit_subpaths);
            bez_path.apply_affine(linear);
            let subpaths = bez_path_to_subpaths(&bez_path);
            if subpaths.is_empty() {
                return None;
            }
            let fill_bbox = bez_path.bounding_box();
            Some(ScaledGlyph {
                bez_path: Arc::new(bez_path),
                subpaths: Arc::new(subpaths),
                fill_bbox,
            })
        });
        self.local_scaled_cache.insert(key, computed.clone());
        computed
    }

    fn extract_shape(
        &mut self,
        shape: &Shape,
        ts: Transform,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        self.active_glyph_indices.clear();
        let fill = shape
            .fill
            .as_ref()
            .map(|p| Rgba::from_paint(p, &mut self.warnings));
        let affine = affine_from_typst(ts);
        let stroke = shape
            .stroke
            .as_ref()
            .map(|s| convert_stroke(s, affine_stroke_scale(affine), &mut self.warnings));

        if fill.is_none() && stroke.is_none() {
            return;
        }

        match &shape.geometry {
            Geometry::Line(end) => {
                if let Some(stroke) = stroke {
                    let start_pt = affine * Point::ZERO;
                    let end_pt = affine * Point::new(end.x.to_pt(), end.y.to_pt());
                    let mut line_bez = BezPath::new();
                    line_bez.move_to(start_pt);
                    line_bez.line_to(end_pt);
                    bbox.include_path(&line_bez, Some(&stroke));
                    primitives.push(VectorPrimitive::Line {
                        start: [start_pt.x, start_pt.y],
                        end: [end_pt.x, end_pt.y],
                        stroke,
                    });
                }
            }
            Geometry::Rect(size) => {
                let bez = rect_bez_path(*size, affine);
                bbox.include_path(&bez, stroke.as_ref());
                let subpaths = bez_path_to_subpaths(&bez);
                if !subpaths.is_empty() {
                    primitives.push(VectorPrimitive::Paths {
                        paths: vec![subpaths],
                        fill,
                        stroke,
                        winding: shape.fill_rule.into(),
                    });
                }
            }
            Geometry::Curve(curve) => {
                let bez = curve_to_bez_path(curve, affine);
                bbox.include_path(&bez, stroke.as_ref());
                let subpaths = bez_path_to_subpaths(&bez);
                push_resolved_paths(
                    primitives,
                    subpaths,
                    fill,
                    stroke,
                    shape.fill_rule.into(),
                );
            }
        }
    }

    fn extract_image(
        &mut self,
        image: &Image,
        size: &Axes<Abs>,
        ts: Transform,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        self.active_glyph_indices.clear();
        let w = size.x.to_pt().max(1e-6);
        let h = size.y.to_pt().max(1e-6);

        if let ImageKind::Svg(svg_img) = image.kind() {
            let tree = svg_img.tree();
            let tree_size = tree.size();
            let sx = w / f64::from(tree_size.width()).max(1e-6);
            let sy = h / f64::from(tree_size.height()).max(1e-6);
            let canvas_to_page = affine_from_typst(ts) * Affine::scale_non_uniform(sx, sy);
            self.extract_usvg_group(tree.root(), canvas_to_page, 1.0, bbox, primitives);
            return;
        }

        // Raster / PDF fallback: orient local `[0..w] × [0..h]` image frame as Y-Up
        // so `(0, 0)` corresponds to Bottom-Left of `Inset`.
        let img_ts = ts
            .pre_concat(Transform::translate(Abs::zero(), size.y))
            .pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
        let img_affine = affine_from_typst(img_ts);
        include_transformed_box(w, h, img_affine, bbox);

        let web = WebImage::new(image);
        primitives.push(VectorPrimitive::Image {
            format: EmbeddedImageFormat::from_extension(web.format.extension()),
            data: web.data,
            size: [w, h],
            transform: img_affine.as_coeffs(),
        });
    }

    fn extract_usvg_group(
        &mut self,
        group: &usvg::Group,
        canvas_to_page: Affine,
        parent_opacity: f64,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        if group.mask().is_some() || !group.filters().is_empty() {
            self.warnings.svg_feature = true;
        }
        let opacity = (parent_opacity * f64::from(group.opacity().get())).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }

        if let Some(clip) = group.clip_path() {
            if let Some((clip_rect, is_rect)) = usvg_clip_rect(clip, canvas_to_page) {
                if !is_rect {
                    self.warnings.non_rect_clip = true;
                }
                let mut child_primitives = Vec::new();
                let mut child_bbox = BoundsTracker::empty();
                self.extract_usvg_children(
                    group.children(),
                    canvas_to_page,
                    opacity,
                    &mut child_bbox,
                    &mut child_primitives,
                );
                if let Some(cb) = child_bbox.rect {
                    let visible = cb.intersect(clip_rect);
                    if visible.width() > 0.0 && visible.height() > 0.0 {
                        bbox.include_rect(visible);
                    }
                }
                if !child_primitives.is_empty()
                    && clip_rect.width() > 0.0
                    && clip_rect.height() > 0.0
                {
                    primitives.push(VectorPrimitive::Clip {
                        bounds: [[clip_rect.x0, clip_rect.x1], [clip_rect.y0, clip_rect.y1]],
                        children: child_primitives,
                    });
                }
                return;
            }
        }

        self.extract_usvg_children(group.children(), canvas_to_page, opacity, bbox, primitives);
    }

    fn extract_usvg_children(
        &mut self,
        children: &[usvg::Node],
        canvas_to_page: Affine,
        opacity: f64,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        for node in children {
            match node {
                usvg::Node::Group(g) => {
                    self.extract_usvg_group(g, canvas_to_page, opacity, bbox, primitives);
                }
                usvg::Node::Path(path) => {
                    if !path.is_visible() {
                        continue;
                    }
                    let path_affine = canvas_to_page * affine_from_usvg(path.abs_transform());
                    let bez = usvg_path_to_bez(path.data(), path_affine);
                    let fill = path
                        .fill()
                        .map(|f| usvg_paint_rgba(f.paint(), f.opacity().get(), opacity, &mut self.warnings));
                    let fill_rule = path
                        .fill()
                        .map_or(WindingRule::NonZero, |f| usvg_fill_rule(f.rule()));
                    let stroke = path.stroke().map(|s| {
                        usvg_stroke(s, affine_stroke_scale(path_affine), opacity, &mut self.warnings)
                    });
                    if fill.is_none() && stroke.is_none() {
                        continue;
                    }
                    bbox.include_path(&bez, stroke.as_ref());
                    let subpaths = bez_path_to_subpaths(&bez);
                    push_resolved_paths(primitives, subpaths, fill, stroke, fill_rule);
                }
                usvg::Node::Text(text) => {
                    self.extract_usvg_group(text.flattened(), canvas_to_page, opacity, bbox, primitives);
                }
                usvg::Node::Image(img) => {
                    if !img.is_visible() {
                        continue;
                    }
                    let img_size = img.size();
                    let iw = f64::from(img_size.width()).max(1e-6);
                    let ih = f64::from(img_size.height()).max(1e-6);
                    let img_to_canvas = affine_from_usvg(img.abs_transform());
                    match img.kind() {
                        usvg::ImageKind::SVG(nested) => {
                            let n_size = nested.size();
                            let nested_affine = canvas_to_page
                                * img_to_canvas
                                * Affine::scale_non_uniform(
                                    iw / f64::from(n_size.width()).max(1e-6),
                                    ih / f64::from(n_size.height()).max(1e-6),
                                );
                            self.extract_usvg_group(
                                nested.root(),
                                nested_affine,
                                opacity,
                                bbox,
                                primitives,
                            );
                        }
                        usvg::ImageKind::JPEG(bytes)
                        | usvg::ImageKind::PNG(bytes)
                        | usvg::ImageKind::GIF(bytes)
                        | usvg::ImageKind::WEBP(bytes) => {
                            let format = match img.kind() {
                                usvg::ImageKind::JPEG(_) => EmbeddedImageFormat::Jpg,
                                usvg::ImageKind::PNG(_) => EmbeddedImageFormat::Png,
                                usvg::ImageKind::GIF(_) => EmbeddedImageFormat::Gif,
                                usvg::ImageKind::WEBP(_) => EmbeddedImageFormat::Webp,
                                usvg::ImageKind::SVG(_) => unreachable!(),
                            };
                            let local_flip =
                                Affine::translate((0.0, ih)) * Affine::scale_non_uniform(1.0, -1.0);
                            let page_affine = canvas_to_page * img_to_canvas * local_flip;
                            include_transformed_box(iw, ih, page_affine, bbox);
                            struct ArcBytes(Arc<Vec<u8>>);
                            impl AsRef<[u8]> for ArcBytes {
                                fn as_ref(&self) -> &[u8] {
                                    &self.0
                                }
                            }
                            primitives.push(VectorPrimitive::Image {
                                format,
                                data: Bytes::new(ArcBytes(Arc::clone(bytes))),
                                size: [iw, ih],
                                transform: page_affine.as_coeffs(),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn outline_glyph_unit(font: &FontInstance, key: UnitGlyphKey) -> Option<Arc<Vec<Subpath>>> {
    if let Ok(guard) = UNIT_GLYPH_CACHE.read() {
        if let Some(cached) = guard.get(&key) {
            return cached.clone();
        }
    }

    let computed = (|| {
        let upem = font.units_per_em();
        if upem <= 0.0 {
            return None;
        }
        let mut builder = BezOutlineBuilder::default();
        font.ttf().outline_glyph(GlyphId(key.glyph_id), &mut builder)?;
        let mut path = builder.0;
        let inv_upem = 1.0 / upem;
        path.apply_affine(Affine::scale(inv_upem));
        let raw_subpaths = bez_path_to_subpaths(&path);
        if raw_subpaths.is_empty() {
            return None;
        }
        let resolved = winding::resolve_nonzero_subpaths(&raw_subpaths);
        (!resolved.is_empty()).then(|| Arc::new(resolved))
    })();

    if let Ok(mut guard) = UNIT_GLYPH_CACHE.write() {
        if guard.len() >= MAX_UNIT_GLYPH_ENTRIES {
            guard.clear();
        }
        guard.insert(key, computed.clone());
    }
    computed
}

fn push_resolved_paths(
    primitives: &mut Vec<VectorPrimitive>,
    subpaths: Vec<Subpath>,
    fill: Option<Rgba>,
    stroke: Option<StrokeStyle>,
    winding: WindingRule,
) {
    if subpaths.is_empty() {
        return;
    }
    if fill.is_some() && winding == WindingRule::NonZero {
        let resolved = winding::resolve_nonzero_subpaths(&subpaths);
        let modified = resolved != subpaths;
        if modified {
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
    }
    primitives.push(VectorPrimitive::Paths {
        paths: vec![subpaths],
        fill,
        stroke,
        winding,
    });
}

fn affine_from_typst(ts: Transform) -> Affine {
    Affine::new([
        ts.sx.get(),
        ts.ky.get(),
        ts.kx.get(),
        ts.sy.get(),
        ts.tx.to_pt(),
        ts.ty.to_pt(),
    ])
}

fn affine_from_usvg(ts: tiny_skia_path::Transform) -> Affine {
    Affine::new([
        f64::from(ts.sx),
        f64::from(ts.ky),
        f64::from(ts.kx),
        f64::from(ts.sy),
        f64::from(ts.tx),
        f64::from(ts.ty),
    ])
}

fn strip_translation(affine: Affine) -> Affine {
    let [sx, ky, kx, sy, _, _] = affine.as_coeffs();
    Affine::new([sx, ky, kx, sy, 0.0, 0.0])
}

fn affine_stroke_scale(affine: Affine) -> f64 {
    let [sx, ky, kx, sy, _, _] = affine.as_coeffs();
    0.5 * (sx.hypot(ky) + kx.hypot(sy))
}

fn include_transformed_box(w: f64, h: f64, affine: Affine, bbox: &mut BoundsTracker) {
    for pt in [
        Point::new(0.0, 0.0),
        Point::new(w, 0.0),
        Point::new(w, h),
        Point::new(0.0, h),
    ] {
        bbox.include_point(affine * pt);
    }
}

fn stroke_hash(s: &StrokeStyle) -> u64 {
    let mut h = DefaultHasher::new();
    s.thickness.to_bits().hash(&mut h);
    (s.cap as u8).hash(&mut h);
    (s.join as u8).hash(&mut h);
    s.miter_limit.to_bits().hash(&mut h);
    h.finish()
}

fn stroke_bounding_box(path: &BezPath, style: &StrokeStyle) -> Rect {
    let kstroke = Stroke {
        width: style.thickness,
        join: match style.join {
            LineJoinStyle::Miter => Join::Miter,
            LineJoinStyle::Round => Join::Round,
            LineJoinStyle::Bevel => Join::Bevel,
        },
        miter_limit: style.miter_limit.max(1.0),
        start_cap: match style.cap {
            LineCapStyle::Butt => Cap::Butt,
            LineCapStyle::Round => Cap::Round,
            LineCapStyle::Square => Cap::Square,
        },
        end_cap: match style.cap {
            LineCapStyle::Butt => Cap::Butt,
            LineCapStyle::Round => Cap::Round,
            LineCapStyle::Square => Cap::Square,
        },
        ..Stroke::default()
    };
    let outlined = kurbo::stroke(path.iter(), &kstroke, &StrokeOpts::default(), 0.25);
    let r = outlined.bounding_box();
    let fallback = path
        .bounding_box()
        .inflate(0.5 * style.thickness, 0.5 * style.thickness);
    if !r.x0.is_finite() || !r.y0.is_finite() || (r.width() == 0.0 && r.height() == 0.0) {
        fallback
    } else {
        r.union(fallback)
    }
}

fn rect_bez_path(size: Size, affine: Affine) -> BezPath {
    let mut bez = Rect::new(0.0, 0.0, size.x.to_pt(), size.y.to_pt()).to_path(0.1);
    bez.apply_affine(affine);
    bez
}

fn curve_to_bez_path(curve: &Curve, affine: Affine) -> BezPath {
    let mut bez = BezPath::new();
    for item in &curve.0 {
        match *item {
            CurveItem::Move(p) => bez.move_to(Point::new(p.x.to_pt(), p.y.to_pt())),
            CurveItem::Line(p) => bez.line_to(Point::new(p.x.to_pt(), p.y.to_pt())),
            CurveItem::Cubic(p1, p2, p3) => bez.curve_to(
                Point::new(p1.x.to_pt(), p1.y.to_pt()),
                Point::new(p2.x.to_pt(), p2.y.to_pt()),
                Point::new(p3.x.to_pt(), p3.y.to_pt()),
            ),
            CurveItem::Close => bez.close_path(),
        }
    }
    bez.apply_affine(affine);
    bez
}

fn usvg_path_to_bez(path: &tiny_skia_path::Path, affine: Affine) -> BezPath {
    let mut bez = BezPath::new();
    for seg in path.segments() {
        match seg {
            tiny_skia_path::PathSegment::MoveTo(p) => {
                bez.move_to(Point::new(f64::from(p.x), f64::from(p.y)));
            }
            tiny_skia_path::PathSegment::LineTo(p) => {
                bez.line_to(Point::new(f64::from(p.x), f64::from(p.y)));
            }
            tiny_skia_path::PathSegment::QuadTo(p1, p2) => {
                bez.quad_to(
                    Point::new(f64::from(p1.x), f64::from(p1.y)),
                    Point::new(f64::from(p2.x), f64::from(p2.y)),
                );
            }
            tiny_skia_path::PathSegment::CubicTo(p1, p2, p3) => {
                bez.curve_to(
                    Point::new(f64::from(p1.x), f64::from(p1.y)),
                    Point::new(f64::from(p2.x), f64::from(p2.y)),
                    Point::new(f64::from(p3.x), f64::from(p3.y)),
                );
            }
            tiny_skia_path::PathSegment::Close => bez.close_path(),
        }
    }
    bez.apply_affine(affine);
    bez
}

fn subpaths_to_bez_path(subpaths: &[Subpath]) -> BezPath {
    let mut path = BezPath::new();
    for sp in subpaths {
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
    }
    path
}

fn bez_path_to_subpaths(path: &BezPath) -> Vec<Subpath> {
    let mut subpaths = Vec::new();
    let mut current_start: Option<Point> = None;
    let mut current_pos = Point::ZERO;
    let mut segments: Vec<Segment> = Vec::new();

    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if let Some(start) = current_start.take() {
                    if !segments.is_empty() {
                        subpaths.push(Subpath {
                            start: [start.x, start.y],
                            segments: std::mem::take(&mut segments),
                            closed: false,
                        });
                    }
                }
                current_start = Some(p);
                current_pos = p;
            }
            PathEl::LineTo(p) => {
                current_start.get_or_insert(current_pos);
                if (p.x - current_pos.x).abs() > 1e-9 || (p.y - current_pos.y).abs() > 1e-9 {
                    segments.push(Segment::Line([p.x, p.y]));
                    current_pos = p;
                }
            }
            PathEl::QuadTo(q1, p3) => {
                current_start.get_or_insert(current_pos);
                let c = QuadBez::new(current_pos, q1, p3).raise();
                segments.push(Segment::Cubic(
                    [c.p1.x, c.p1.y],
                    [c.p2.x, c.p2.y],
                    [c.p3.x, c.p3.y],
                ));
                current_pos = p3;
            }
            PathEl::CurveTo(c1, c2, p3) => {
                current_start.get_or_insert(current_pos);
                segments.push(Segment::Cubic([c1.x, c1.y], [c2.x, c2.y], [p3.x, p3.y]));
                current_pos = p3;
            }
            PathEl::ClosePath => {
                if let Some(start) = current_start.take() {
                    if !segments.is_empty() {
                        if (current_pos.x - start.x).abs() > 1e-9
                            || (current_pos.y - start.y).abs() > 1e-9
                        {
                            segments.push(Segment::Line([start.x, start.y]));
                        }
                        subpaths.push(Subpath {
                            start: [start.x, start.y],
                            segments: std::mem::take(&mut segments),
                            closed: true,
                        });
                    }
                    current_pos = start;
                }
            }
        }
    }

    if let Some(start) = current_start {
        if !segments.is_empty() {
            subpaths.push(Subpath {
                start: [start.x, start.y],
                segments,
                closed: false,
            });
        }
    }

    subpaths
}

fn is_axis_aligned_rect(path: &BezPath) -> bool {
    let subpaths = bez_path_to_subpaths(path);
    let [sp] = subpaths.as_slice() else {
        return false;
    };
    if !sp.closed || sp.segments.len() != 4 {
        return false;
    }
    let mut prev = sp.start;
    for seg in &sp.segments {
        let Segment::Line(p) = *seg else {
            return false;
        };
        let is_h = (p[1] - prev[1]).abs() <= 1e-4 && (p[0] - prev[0]).abs() > 1e-4;
        let is_v = (p[0] - prev[0]).abs() <= 1e-4 && (p[1] - prev[1]).abs() > 1e-4;
        if !is_h && !is_v {
            return false;
        }
        prev = p;
    }
    true
}

fn usvg_clip_rect(clip: &usvg::ClipPath, canvas_to_page: Affine) -> Option<(Rect, bool)> {
    let mut all_rect = true;
    let mut tracker = BoundsTracker::empty();
    let clip_affine = canvas_to_page * affine_from_usvg(clip.transform());
    collect_usvg_clip_bounds(clip.root(), clip_affine, &mut all_rect, &mut tracker);
    tracker.rect.map(|r| (r.abs(), all_rect))
}

fn collect_usvg_clip_bounds(
    group: &usvg::Group,
    clip_affine: Affine,
    all_rect: &mut bool,
    tracker: &mut BoundsTracker,
) {
    let mut group_paths = 0_usize;
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => {
                collect_usvg_clip_bounds(g, clip_affine, all_rect, tracker);
            }
            usvg::Node::Path(path) => {
                group_paths += 1;
                let p_aff = clip_affine * affine_from_usvg(path.abs_transform());
                let bez = usvg_path_to_bez(path.data(), p_aff);
                if !is_axis_aligned_rect(&bez) {
                    *all_rect = false;
                }
                tracker.include_path(&bez, None);
            }
            usvg::Node::Text(text) => {
                *all_rect = false;
                collect_usvg_clip_bounds(text.flattened(), clip_affine, all_rect, tracker);
            }
            usvg::Node::Image(_) => {}
        }
    }
    if group_paths > 1 {
        *all_rect = false;
    }
}

#[derive(Default)]
struct BezOutlineBuilder(BezPath);

impl OutlineBuilder for BezOutlineBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(Point::new(f64::from(x), f64::from(y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(Point::new(f64::from(x), f64::from(y)));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(
            Point::new(f64::from(x1), f64::from(y1)),
            Point::new(f64::from(x), f64::from(y)),
        );
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.curve_to(
            Point::new(f64::from(x1), f64::from(y1)),
            Point::new(f64::from(x2), f64::from(y2)),
            Point::new(f64::from(x), f64::from(y)),
        );
    }

    fn close(&mut self) {
        self.0.close_path();
    }
}

fn convert_stroke(
    stroke: &FixedStroke,
    scale: f64,
    warnings: &mut DegradationWarnings,
) -> StrokeStyle {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dash = stroke.dash.as_ref().map(|d| {
        let phase = d.phase.to_pt() * s;
        if phase.abs() > 1e-6 {
            warnings.dash_phase = true;
        }
        (
            d.array.iter().map(|len| len.to_pt() * s).collect(),
            phase,
        )
    });
    StrokeStyle {
        color: Rgba::from_paint(&stroke.paint, warnings),
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

fn usvg_fill_rule(rule: usvg::FillRule) -> WindingRule {
    match rule {
        usvg::FillRule::NonZero => WindingRule::NonZero,
        usvg::FillRule::EvenOdd => WindingRule::EvenOdd,
    }
}

fn usvg_paint_rgba(
    paint: &usvg::Paint,
    paint_opacity: f32,
    group_opacity: f64,
    warnings: &mut DegradationWarnings,
) -> Rgba {
    let a = (f64::from(paint_opacity) * group_opacity).clamp(0.0, 1.0);
    let color = match paint {
        usvg::Paint::Color(c) => *c,
        usvg::Paint::LinearGradient(g) => {
            warnings.gradient = true;
            sample_stops(g.stops())
        }
        usvg::Paint::RadialGradient(g) => {
            warnings.gradient = true;
            sample_stops(g.stops())
        }
        usvg::Paint::Pattern(_) => {
            warnings.tiling = true;
            usvg::Color::new_rgb(128, 128, 128)
        }
    };
    Rgba {
        r: f64::from(color.red) / 255.0,
        g: f64::from(color.green) / 255.0,
        b: f64::from(color.blue) / 255.0,
        a,
    }
}

fn sample_stops(stops: &[usvg::Stop]) -> usvg::Color {
    if stops.is_empty() {
        return usvg::Color::black();
    }
    stops[stops.len() / 2].color()
}

fn usvg_stroke(
    stroke: &usvg::Stroke,
    scale: f64,
    group_opacity: f64,
    warnings: &mut DegradationWarnings,
) -> StrokeStyle {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dash = stroke.dasharray().map(|arr| {
        let phase = f64::from(stroke.dashoffset()) * s;
        if phase.abs() > 1e-6 {
            warnings.dash_phase = true;
        }
        (arr.iter().map(|v| f64::from(*v) * s).collect(), phase)
    });
    StrokeStyle {
        color: usvg_paint_rgba(stroke.paint(), stroke.opacity().get(), group_opacity, warnings),
        thickness: (f64::from(stroke.width().get()) * s).max(0.0),
        cap: match stroke.linecap() {
            usvg::LineCap::Butt => LineCapStyle::Butt,
            usvg::LineCap::Round => LineCapStyle::Round,
            usvg::LineCap::Square => LineCapStyle::Square,
        },
        join: match stroke.linejoin() {
            usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoinStyle::Miter,
            usvg::LineJoin::Round => LineJoinStyle::Round,
            usvg::LineJoin::Bevel => LineJoinStyle::Bevel,
        },
        miter_limit: f64::from(stroke.miterlimit().get()),
        dash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::MathemeWorld;
    use std::path::PathBuf;
    use typst::foundations::Dict;

    fn compile_typst(src: &str) -> VectorExport {
        let world = MathemeWorld::new(src.into(), Dict::new(), PathBuf::from("."));
        let doc = typst::compile::<PagedDocument>(&world)
            .output
            .expect("typst compile");
        export_vector(&doc, None)
    }

    #[test]
    fn extracts_glyph_outlines_and_expands_descender_bbox() {
        let export = compile_typst("#set page(width: auto, height: auto, margin: 0pt, fill: none)\nxxyyzz");
        assert_eq!(export.pages.len(), 1);
        let page = &export.pages[0];
        assert!(page.size[0] > 20.0);
        assert!(page.size[1] > 5.0);
        assert!(
            page.bounds[1][0] < -1.0,
            "expected negative y_min for 'y' descender, got {:?}",
            page.bounds
        );
        let (unique_glyphs, total_instances): (usize, usize) = page
            .primitives
            .iter()
            .map(|p| match p {
                VectorPrimitive::Glyphs { glyphs, .. } => {
                    (glyphs.len(), glyphs.iter().map(|g| g.positions.len()).sum())
                }
                _ => (0, 0),
            })
            .fold((0, 0), |(u, t), (du, dt)| (u + du, t + dt));
        assert_eq!(unique_glyphs, 3);
        assert_eq!(total_instances, 6);
    }

    #[test]
    fn extracts_unicode_math_curves_not_tofu() {
        let export = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$ x^y + integral_0^infinity e^(-x^2) dif x $",
        );
        assert_eq!(export.pages.len(), 1);
        let cubic_count: usize = export.pages[0]
            .primitives
            .iter()
            .flat_map(|p| match p {
                VectorPrimitive::Glyphs { glyphs, .. } => glyphs
                    .iter()
                    .flat_map(|g| g.subpaths.as_slice())
                    .collect::<Vec<_>>(),
                VectorPrimitive::Paths { paths, .. } => paths.iter().flatten().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .flat_map(|sp| &sp.segments)
            .filter(|seg| matches!(seg, Segment::Cubic(..)))
            .count();
        assert!(
            cubic_count > 50,
            "expected curved glyph outlines for unicode-math, got {cubic_count}"
        );
    }

    #[test]
    fn expands_bounds_for_stroked_rect_and_emits_clip_primitive() {
        let stroked = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#rect(width: 40pt, height: 25pt, stroke: 4pt + red)",
        );
        let b = stroked.pages[0].bounds;
        assert!(b[0][0] <= -1.9 && b[0][1] >= 41.9, "stroke x bounds: {b:?}");
        assert!(b[1][0] <= -1.9 && b[1][1] >= 26.9, "stroke y bounds: {b:?}");

        let clipped = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#box(width: 20pt, height: 15pt, clip: true, rect(width: 60pt, height: 60pt, fill: red))",
        );
        let cb = clipped.pages[0].bounds;
        assert!((cb[0][1] - cb[0][0] - 20.0).abs() < 0.01, "clipped width: {cb:?}");
        assert!((cb[1][1] - cb[1][0] - 15.0).abs() < 0.01, "clipped height: {cb:?}");
        assert!(
            clipped.pages[0]
                .primitives
                .iter()
                .any(|p| matches!(p, VectorPrimitive::Clip { .. })),
            "expected VectorPrimitive::Clip"
        );
    }

    #[test]
    fn emits_diagnostics_for_gradient_and_font_reset_advances_generation() {
        let g1 = crate::fonts::font_generation();
        let c1 = cache_generation();
        crate::fonts::reset();
        assert!(crate::fonts::font_generation() > g1);
        assert!(cache_generation() > c1);

        let grad = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#rect(width: 20pt, height: 10pt, fill: gradient.linear(red, blue))",
        );
        assert!(
            grad.diagnostics
                .iter()
                .any(|d| d.message.contains("Gradient")),
            "expected gradient approximation diagnostic: {:?}",
            grad.diagnostics
        );
    }
}
