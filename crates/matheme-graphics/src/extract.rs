//! Host-agnostic vector graphics extraction from a compiled Typst [`PagedDocument`].
//!
//! Walks each [`Page`]'s [`Frame`] tree directly, outlines glyphs via
//! `ttf-parser` using lightweight generation-tracked caches (never pinning
//! font byte buffers), extracts embedded SVGs via `usvg`, resolves non-zero
//! winding rules for even-odd hosts, computes tight stroke/clip bounds via
//! `kurbo`, and produces a [`VectorExport`] containing [`VectorPage`]s and any
//! fidelity-degradation [`DegradationWarning`]s.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use kurbo::{Affine, BezPath, Cap, CubicBez, Join, PathEl, Point, QuadBez, Rect, Shape as _, Stroke, StrokeOpts};
use ttf_parser::{GlyphId, OutlineBuilder};
use typst::foundations::{Bytes, Smart};
use typst::layout::{Abs, Axes, Frame, FrameItem, GroupItem, PageRanges, Ratio, Size, Transform};
use typst::text::color::{glyph_frame, should_outline, GlyphFrameItem};
use typst::text::{Font, FontInstance, TextItem};
use typst::visualize::{
    Color, Curve, CurveItem, FillRule, FixedStroke, Geometry, Image, ImageKind, LineCap, LineJoin,
    Paint, RatioOrAngle, Shape,
};
use typst_layout::{Page, PagedDocument};
use typst_svg::WebImage;
use usvg::tiny_skia_path;

use crate::lru::{lock_unpoisoned, LruCache};
use crate::winding::{self, WindingResolution};

/// Shared upper bound across all glyph cache tiers (unit outlines, scaled
/// outlines, and host-encoded expressions) using O(1) LRU eviction.
pub(crate) const GLYPH_CACHE_CAPACITY: usize = 8192;

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
    pub(crate) fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpg),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            "svg" => Some(Self::Svg),
            _ => None,
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
/// within a live font generation. Holds zero `Font` / byte-buffer references
/// and zero raw pointers (content-hashed to avoid allocator ABA collisions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitGlyphKey {
    pub font_gen: u64,
    pub font_hash: u64,
    pub face_index: u32,
    pub variations_hash: u64,
    pub glyph_id: u16,
}

/// Precomputed per-`TextItem` font & variation identity, hoisted out of the
/// per-glyph loop so `variations()` and font bytes are hashed once per run.
#[derive(Debug, Clone, Copy)]
struct FontRunKey {
    font_gen: u64,
    font_hash: u64,
    face_index: u32,
    variations_hash: u64,
}

impl FontRunKey {
    fn unit_key(self, glyph_id: u16) -> UnitGlyphKey {
        UnitGlyphKey {
            font_gen: self.font_gen,
            font_hash: self.font_hash,
            face_index: self.face_index,
            variations_hash: self.variations_hash,
            glyph_id,
        }
    }
}

/// Compute a fast, content-derived fingerprint of a [`Font`] without retaining
/// any reference to its bytes or relying on its heap address.
fn hash_font_content(base: &Font) -> u64 {
    let mut h = DefaultHasher::new();
    let bytes: &[u8] = base.data().as_ref();
    bytes.len().hash(&mut h);
    base.index().hash(&mut h);
    base.info().family.hash(&mut h);
    base.info().variant.hash(&mut h);
    let n = bytes.len();
    bytes[..n.min(64)].hash(&mut h);
    if n > 128 {
        let mid = (n / 2).saturating_sub(32);
        bytes[mid..(mid + 64).min(n)].hash(&mut h);
    }
    if n > 64 {
        bytes[n.saturating_sub(64)..].hash(&mut h);
    }
    h.finish()
}

/// Quantize a 2×2 linear transform `[sx, ky, kx, sy]` to `1e-6` precision once
/// per text run.
fn quantize_linear(linear: Affine) -> [i64; 4] {
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
    [q(sx), q(ky), q(kx), q(sy)]
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
    fn from_quantized(unit: UnitGlyphKey, linear_q: [i64; 4]) -> Self {
        Self { unit, linear_q }
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

/// Scaled glyph outline cached across exports in a bounded LRU.
#[derive(Debug, Clone)]
struct ScaledGlyph {
    subpaths: Arc<Vec<Subpath>>,
    fill_bbox: Rect,
}

#[derive(Clone)]
struct CachedUnitGlyph {
    subpaths: Option<Arc<Vec<Subpath>>>,
    winding_degraded: bool,
}

struct UnitGlyphCache {
    font_gen: u64,
    entries: LruCache<UnitGlyphKey, CachedUnitGlyph>,
}

impl UnitGlyphCache {
    fn new() -> Self {
        Self {
            font_gen: 0,
            entries: LruCache::new(GLYPH_CACHE_CAPACITY),
        }
    }
}

struct ScaledGlyphCache {
    generation: u64,
    entries: LruCache<GlyphKey, Option<ScaledGlyph>>,
}

impl ScaledGlyphCache {
    fn new() -> Self {
        Self {
            generation: 0,
            entries: LruCache::new(GLYPH_CACHE_CAPACITY),
        }
    }
}

static CACHE_GENERATION: AtomicU64 = AtomicU64::new(1);
static LAST_FONT_GEN: AtomicU64 = AtomicU64::new(0);

static UNIT_GLYPH_CACHE: LazyLock<Mutex<UnitGlyphCache>> =
    LazyLock::new(|| Mutex::new(UnitGlyphCache::new()));

static SCALED_GLYPH_CACHE: LazyLock<Mutex<ScaledGlyphCache>> =
    LazyLock::new(|| Mutex::new(ScaledGlyphCache::new()));

/// Current glyph-cache generation, incremented atomically whenever
/// [`clear_glyph_cache`] runs or the host font generation changes.
pub fn cache_generation() -> u64 {
    CACHE_GENERATION.load(Ordering::Acquire)
}

/// Drop all cached unit and scaled glyph outlines and atomically advance
/// [`cache_generation`].
pub fn clear_glyph_cache() {
    let new_gen = {
        let mut guard = lock_unpoisoned(&UNIT_GLYPH_CACHE);
        guard.entries.clear();
        CACHE_GENERATION.fetch_add(1, Ordering::AcqRel) + 1
    };
    let mut scaled = lock_unpoisoned(&SCALED_GLYPH_CACHE);
    scaled.entries.clear();
    scaled.generation = new_gen;
}

fn sync_font_generation(font_gen: u64) {
    if LAST_FONT_GEN.load(Ordering::Acquire) == font_gen {
        return;
    }
    let new_gen = {
        let mut guard = lock_unpoisoned(&UNIT_GLYPH_CACHE);
        if guard.font_gen == font_gen {
            LAST_FONT_GEN.store(font_gen, Ordering::Release);
            return;
        }
        guard.entries.clear();
        guard.font_gen = font_gen;
        let next = CACHE_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
        LAST_FONT_GEN.store(font_gen, Ordering::Release);
        next
    };
    let mut scaled = lock_unpoisoned(&SCALED_GLYPH_CACHE);
    scaled.entries.clear();
    scaled.generation = new_gen;
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

/// Host-neutral warning describing a Typst visual feature approximated during
/// vector extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradationWarning {
    GradientApproximated,
    TilingApproximated,
    DashPhaseIgnored,
    NonRectClipApproximated,
    UnsupportedSvgFeature,
    NonZeroWindingApproximated,
    UnsupportedImageFormat,
}

impl DegradationWarning {
    /// Host-neutral human-readable warning text describing the approximation.
    pub fn message(self) -> &'static str {
        match self {
            Self::GradientApproximated => {
                "Gradient fills/strokes are approximated by midpoint solid colors during vector extraction."
            }
            Self::TilingApproximated => {
                "Tiling patterns are approximated by solid gray during vector extraction."
            }
            Self::DashPhaseIgnored => {
                "Non-zero stroke dash phases are ignored during vector extraction."
            }
            Self::NonRectClipApproximated => {
                "Non-rectangular clip paths are approximated by their axis-aligned bounding box during vector extraction."
            }
            Self::UnsupportedSvgFeature => {
                "Unsupported SVG features (filters/masks) inside embedded SVGs are ignored during vector extraction."
            }
            Self::NonZeroWindingApproximated => {
                "Complex non-zero fill contours exceeded planar winding resolution limits or failed loop closure and were kept as-is."
            }
            Self::UnsupportedImageFormat => {
                "An embedded image used an unsupported format extension and was omitted during vector extraction."
            }
        }
    }
}

/// Complete vector export output plus any fidelity-degradation warnings.
#[derive(Debug, Clone)]
pub struct VectorExport {
    pub pages: Vec<VectorPage>,
    pub warnings: Vec<DegradationWarning>,
}

#[derive(Default)]
struct DegradationWarnings {
    gradient: bool,
    tiling: bool,
    dash_phase: bool,
    non_rect_clip: bool,
    svg_feature: bool,
    winding: bool,
    unsupported_image: bool,
}

impl DegradationWarnings {
    fn into_warnings(self) -> Vec<DegradationWarning> {
        let mut out = Vec::new();
        if self.gradient {
            out.push(DegradationWarning::GradientApproximated);
        }
        if self.tiling {
            out.push(DegradationWarning::TilingApproximated);
        }
        if self.dash_phase {
            out.push(DegradationWarning::DashPhaseIgnored);
        }
        if self.non_rect_clip {
            out.push(DegradationWarning::NonRectClipApproximated);
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
    sync_font_generation(font_generation);
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

struct Extractor {
    font_gen: u64,
    /// Per-extraction fast memo for content-derived font hashes (safe because
    /// the borrowed `PagedDocument` keeps every `Font` alive for this call).
    font_content_hashes: HashMap<(usize, usize, u32), u64>,
    /// Per-extraction cache of scaled glyph contours at origin `(0, 0)` (backed
    /// by the process-wide LRU `SCALED_GLYPH_CACHE` across calls).
    local_scaled_cache: HashMap<GlyphKey, Option<ScaledGlyph>>,
    /// Per-extraction cache of stroked glyph origin bounding boxes `[dx0, dy0, dx1, dy1]`.
    stroked_glyph_bboxes: HashMap<(GlyphKey, u64), Rect>,
    /// Per-extraction cache of stroked curve bounding boxes `(path_hash, stroke_hash)`.
    stroked_path_bboxes: HashMap<(u64, u64), Rect>,
    /// Maps a [`GlyphKey`] to its slot inside the active trailing [`VectorPrimitive::Glyphs`].
    active_glyph_indices: HashMap<GlyphKey, usize>,
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
            warnings: DegradationWarnings::default(),
        }
    }

    fn font_run_key(&mut self, font: &FontInstance) -> FontRunKey {
        let base = font.font();
        let ptr_key = (base.data().as_ptr() as usize, base.data().len(), base.index());
        let font_hash = *self
            .font_content_hashes
            .entry(ptr_key)
            .or_insert_with(|| hash_font_content(base));
        let mut var_hasher = DefaultHasher::new();
        font.variations().hash(&mut var_hasher);
        FontRunKey {
            font_gen: self.font_gen,
            font_hash,
            face_index: base.index(),
            variations_hash: var_hasher.finish(),
        }
    }

    fn include_stroked_path(&mut self, bbox: &mut BoundsTracker, path: &BezPath, stroke: Option<&StrokeStyle>) {
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

        let saved_glyph_indices = std::mem::take(&mut self.active_glyph_indices);

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
        } else {
            // Nothing was pushed onto `primitives`, so parent glyph coalescing
            // state remains valid.
            self.active_glyph_indices = saved_glyph_indices;
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
        let linear_q = quantize_linear(linear_affine);
        let run_key = self.font_run_key(&text.font);

        let mut x = Abs::zero();
        let mut y = Abs::zero();

        for glyph in &text.glyphs {
            let id = GlyphId(glyph.id);
            let x_offset = x + glyph.x_offset.at(text.size);
            let y_offset = y + glyph.y_offset.at(text.size);

            if should_outline(&text.font, id) {
                let unit_key = run_key.unit_key(id.0);
                let key = GlyphKey::from_quantized(unit_key, linear_q);
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
        let stroked = stroke_bounding_box(&subpaths_to_bez_path(&scaled.subpaths), style);
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

        let gen_now = cache_generation();
        {
            let mut global = lock_unpoisoned(&SCALED_GLYPH_CACHE);
            if global.generation != gen_now {
                global.entries.clear();
                global.generation = gen_now;
            } else if let Some(cached) = global.entries.get(&key) {
                let hit = cached.clone();
                self.local_scaled_cache.insert(key, hit.clone());
                return hit;
            }
        }

        let unit = outline_glyph_unit(font, key.unit);
        if unit.winding_degraded {
            self.warnings.winding = true;
        }
        let computed = unit.subpaths.and_then(|unit_subpaths| {
            let (subpaths, fill_bbox) = transform_subpaths_and_bbox(&unit_subpaths, linear)?;
            Some(ScaledGlyph {
                subpaths: Arc::new(subpaths),
                fill_bbox,
            })
        });

        {
            let mut global = lock_unpoisoned(&SCALED_GLYPH_CACHE);
            if global.generation != gen_now {
                global.entries.clear();
                global.generation = gen_now;
            }
            global.entries.insert(key, computed.clone());
        }
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
                }
            }
            Geometry::Rect(size) => {
                let bez = rect_bez_path(*size, affine);
                let [_, ky, kx, _, _, _] = affine.as_coeffs();
                if kx.abs() <= 1e-9 && ky.abs() <= 1e-9 {
                    let base_rect = bez.bounding_box().abs();
                    let r = match stroke.as_ref().filter(|s| s.thickness > 0.0) {
                        Some(style) => axis_aligned_rect_stroke_bbox(base_rect, style),
                        None => base_rect,
                    };
                    bbox.include_rect(r);
                } else {
                    self.include_stroked_path(bbox, &bez, stroke.as_ref());
                }
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
                self.include_stroked_path(bbox, &bez, stroke.as_ref());
                let subpaths = bez_path_to_subpaths(&bez);
                push_resolved_paths(
                    primitives,
                    subpaths,
                    fill,
                    stroke,
                    shape.fill_rule.into(),
                    &mut self.warnings,
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
            // Typst's layout engine has already resolved `image(fit: ...)` into
            // `size`, so mapping `tree.size()` onto `(w, h)` composed with the
            // full 6-parameter affine `ts` preserves rotation, skew, and fit.
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
        let Some(format) = EmbeddedImageFormat::from_extension(web.format.extension()) else {
            self.warnings.unsupported_image = true;
            return;
        };
        primitives.push(VectorPrimitive::Image {
            format,
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
                    self.include_stroked_path(bbox, &bez, stroke.as_ref());
                    let subpaths = bez_path_to_subpaths(&bez);
                    push_resolved_paths(
                        primitives,
                        subpaths,
                        fill,
                        stroke,
                        fill_rule,
                        &mut self.warnings,
                    );
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

fn outline_glyph_unit(font: &FontInstance, key: UnitGlyphKey) -> CachedUnitGlyph {
    {
        let mut guard = lock_unpoisoned(&UNIT_GLYPH_CACHE);
        if guard.font_gen == key.font_gen {
            if let Some(cached) = guard.entries.get(&key) {
                return cached.clone();
            }
        }
    }

    let computed = (|| {
        let upem = font.units_per_em();
        if upem <= 0.0 {
            return CachedUnitGlyph {
                subpaths: None,
                winding_degraded: false,
            };
        }
        let mut builder = BezOutlineBuilder::default();
        if font.ttf().outline_glyph(GlyphId(key.glyph_id), &mut builder).is_none() {
            return CachedUnitGlyph {
                subpaths: None,
                winding_degraded: false,
            };
        }
        let mut path = builder.0;
        let inv_upem = 1.0 / upem;
        path.apply_affine(Affine::scale(inv_upem));
        let raw_subpaths = bez_path_to_subpaths(&path);
        if raw_subpaths.is_empty() {
            return CachedUnitGlyph {
                subpaths: None,
                winding_degraded: false,
            };
        }
        let (final_subpaths, winding_degraded) = match winding::resolve_nonzero_subpaths(&raw_subpaths)
        {
            WindingResolution::Unchanged => (raw_subpaths, false),
            WindingResolution::Resolved(resolved) => (resolved, false),
            WindingResolution::BudgetExceeded | WindingResolution::ExtractionFailed => {
                (raw_subpaths, true)
            }
        };
        CachedUnitGlyph {
            subpaths: (!final_subpaths.is_empty()).then(|| Arc::new(final_subpaths)),
            winding_degraded,
        }
    })();

    let mut guard = lock_unpoisoned(&UNIT_GLYPH_CACHE);
    if guard.font_gen != key.font_gen {
        guard.entries.clear();
        guard.font_gen = key.font_gen;
        let next = CACHE_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
        LAST_FONT_GEN.store(key.font_gen, Ordering::Release);
        guard.entries.insert(key, computed.clone());
        drop(guard);
        let mut scaled = lock_unpoisoned(&SCALED_GLYPH_CACHE);
        scaled.entries.clear();
        scaled.generation = next;
    } else {
        guard.entries.insert(key, computed.clone());
    }
    computed
}

fn push_resolved_paths(
    primitives: &mut Vec<VectorPrimitive>,
    subpaths: Vec<Subpath>,
    fill: Option<Rgba>,
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

fn bez_path_hash(path: &BezPath) -> u64 {
    let mut h = DefaultHasher::new();
    let elems = path.elements();
    elems.len().hash(&mut h);
    for el in elems {
        match *el {
            PathEl::MoveTo(p) => {
                0u8.hash(&mut h);
                p.x.to_bits().hash(&mut h);
                p.y.to_bits().hash(&mut h);
            }
            PathEl::LineTo(p) => {
                1u8.hash(&mut h);
                p.x.to_bits().hash(&mut h);
                p.y.to_bits().hash(&mut h);
            }
            PathEl::QuadTo(p1, p2) => {
                2u8.hash(&mut h);
                p1.x.to_bits().hash(&mut h);
                p1.y.to_bits().hash(&mut h);
                p2.x.to_bits().hash(&mut h);
                p2.y.to_bits().hash(&mut h);
            }
            PathEl::CurveTo(p1, p2, p3) => {
                3u8.hash(&mut h);
                p1.x.to_bits().hash(&mut h);
                p1.y.to_bits().hash(&mut h);
                p2.x.to_bits().hash(&mut h);
                p2.y.to_bits().hash(&mut h);
                p3.x.to_bits().hash(&mut h);
                p3.y.to_bits().hash(&mut h);
            }
            PathEl::ClosePath => 4u8.hash(&mut h),
        }
    }
    h.finish()
}

/// Exact analytical O(1) stroke bounding box for a single straight line segment,
/// respecting `Butt`, `Square`, and `Round` line caps without allocating or
/// expanding a `kurbo::stroke` outline.
fn line_stroke_bbox(start: Point, end: Point, style: &StrokeStyle) -> Rect {
    let hw = 0.5 * style.thickness;
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = dx.hypot(dy);
    if len <= 1e-12 {
        return match style.cap {
            LineCapStyle::Butt => Rect::from_points(start, end),
            LineCapStyle::Round | LineCapStyle::Square => {
                Rect::new(start.x - hw, start.y - hw, start.x + hw, start.y + hw)
            }
        };
    }
    let tx = dx / len;
    let ty = dy / len;
    let nx = -ty;
    let ny = tx;
    let (pad_x, pad_y) = match style.cap {
        LineCapStyle::Butt => (hw * nx.abs(), hw * ny.abs()),
        LineCapStyle::Square => (hw * (tx.abs() + nx.abs()), hw * (ty.abs() + ny.abs())),
        LineCapStyle::Round => (hw, hw),
    };
    Rect::new(
        start.x.min(end.x) - pad_x,
        start.y.min(end.y) - pad_y,
        start.x.max(end.x) + pad_x,
        start.y.max(end.y) + pad_y,
    )
}

/// Exact analytical O(1) stroke bounding box for a closed axis-aligned
/// rectangle (all 90° corners project to `±thickness/2` in X and Y for Miter,
/// Round, and Bevel joins).
fn axis_aligned_rect_stroke_bbox(rect: Rect, style: &StrokeStyle) -> Rect {
    let hw = 0.5 * style.thickness;
    rect.abs().inflate(hw, hw)
}

fn stroke_bounding_box(path: &BezPath, style: &StrokeStyle) -> Rect {
    // Fast exact path for a single 2-point line segment.
    if let [PathEl::MoveTo(start), PathEl::LineTo(end)] = path.elements() {
        return line_stroke_bbox(*start, *end, style);
    }

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
    if r.x0.is_finite()
        && r.y0.is_finite()
        && r.x1.is_finite()
        && r.y1.is_finite()
        && (r.width() > 0.0 || r.height() > 0.0)
    {
        r
    } else {
        path.bounding_box()
            .inflate(0.5 * style.thickness, 0.5 * style.thickness)
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

/// Directly transform unit-em [`Subpath`]s by a 2×2 linear [`Affine`] and
/// compute their exact bounding box in one pass, avoiding an intermediate
/// `BezPath` round-trip on every glyph cache miss.
fn transform_subpaths_and_bbox(subpaths: &[Subpath], linear: Affine) -> Option<(Vec<Subpath>, Rect)> {
    let [sx, ky, kx, sy, tx, ty] = linear.as_coeffs();
    let xf = |p: [f64; 2]| -> [f64; 2] { [sx * p[0] + kx * p[1] + tx, ky * p[0] + sy * p[1] + ty] };

    let mut out = Vec::with_capacity(subpaths.len());
    let mut bbox: Option<Rect> = None;
    let mut include_rect = |r: Rect| {
        bbox = Some(match bbox {
            Some(cur) => cur.union(r),
            None => r,
        });
    };

    for sp in subpaths {
        if sp.segments.is_empty() {
            continue;
        }
        let start = xf(sp.start);
        let mut prev = Point::new(start[0], start[1]);
        let mut segs = Vec::with_capacity(sp.segments.len());

        for seg in &sp.segments {
            match *seg {
                Segment::Line(p) => {
                    let tp = xf(p);
                    let pt = Point::new(tp[0], tp[1]);
                    include_rect(Rect::from_points(prev, pt).abs());
                    segs.push(Segment::Line(tp));
                    prev = pt;
                }
                Segment::Cubic(c1, c2, p) => {
                    let tc1 = xf(c1);
                    let tc2 = xf(c2);
                    let tp = xf(p);
                    let pt = Point::new(tp[0], tp[1]);
                    let cubic = CubicBez::new(
                        prev,
                        Point::new(tc1[0], tc1[1]),
                        Point::new(tc2[0], tc2[1]),
                        pt,
                    );
                    include_rect(cubic.bounding_box());
                    segs.push(Segment::Cubic(tc1, tc2, tp));
                    prev = pt;
                }
            }
        }
        out.push(Subpath {
            start,
            segments: segs,
            closed: sp.closed,
        });
    }

    if out.is_empty() {
        None
    } else {
        Some((out, bbox.unwrap_or(Rect::ZERO)))
    }
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
    use matheme_world::MathemeWorld;
    use std::path::PathBuf;
    use typst::foundations::Dict;

    fn compile_doc(src: &str) -> PagedDocument {
        let world = MathemeWorld::new(src.into(), Dict::new(), PathBuf::from("."));
        typst::compile::<PagedDocument>(&world)
            .output
            .expect("typst compile")
    }

    fn compile_typst(src: &str) -> VectorExport {
        compile_typst_with_gen(src, 1)
    }

    fn compile_typst_with_gen(src: &str, font_gen: u64) -> VectorExport {
        let doc = compile_doc(src);
        export_vector(&doc, None, font_gen)
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
        assert!(
            (b[0][0] - (-2.0)).abs() < 0.05 && (b[0][1] - 42.0).abs() < 0.05,
            "stroke x bounds must match exact [-2, 42]: {b:?}"
        );
        assert!(
            (b[1][0] - (-2.0)).abs() < 0.05 && (b[1][1] - 27.0).abs() < 0.05,
            "stroke y bounds must match exact [-2, 27]: {b:?}"
        );

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
    fn analytical_line_stroke_bbox_respects_cap_style_without_overexpanding_butt() {
        let butt = StrokeStyle {
            color: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
            thickness: 6.0,
            cap: LineCapStyle::Butt,
            join: LineJoinStyle::Miter,
            miter_limit: 4.0,
            dash: None,
        };
        let b = line_stroke_bbox(Point::new(0.0, 10.0), Point::new(40.0, 10.0), &butt);
        assert!((b.x0 - 0.0).abs() < 1e-6 && (b.x1 - 40.0).abs() < 1e-6, "butt cap must not expand x: {b:?}");
        assert!((b.y0 - 7.0).abs() < 1e-6 && (b.y1 - 13.0).abs() < 1e-6, "butt cap y: {b:?}");

        let square = StrokeStyle { cap: LineCapStyle::Square, ..butt.clone() };
        let sq = line_stroke_bbox(Point::new(0.0, 10.0), Point::new(40.0, 10.0), &square);
        assert!((sq.x0 - (-3.0)).abs() < 1e-6 && (sq.x1 - 43.0).abs() < 1e-6, "square cap x: {sq:?}");

        let round = StrokeStyle { cap: LineCapStyle::Round, ..butt };
        let rd = line_stroke_bbox(Point::new(0.0, 10.0), Point::new(40.0, 10.0), &round);
        assert!((rd.x0 - (-3.0)).abs() < 1e-6 && (rd.x1 - 43.0).abs() < 1e-6, "round cap x: {rd:?}");
    }

    #[test]
    fn unknown_image_extension_returns_none_instead_of_png() {
        assert_eq!(EmbeddedImageFormat::from_extension("png"), Some(EmbeddedImageFormat::Png));
        assert_eq!(EmbeddedImageFormat::from_extension("JPEG"), Some(EmbeddedImageFormat::Jpg));
        assert_eq!(EmbeddedImageFormat::from_extension("bmp"), None);
        assert_eq!(EmbeddedImageFormat::from_extension("tiff"), None);
    }

    #[test]
    fn emits_warnings_for_gradient_and_advances_cache_generation_on_font_gen_change() {
        // Use local stub generations (decoupled from host font store state).
        let doc = compile_doc("#set page(width: auto, height: auto, margin: 0pt, fill: none)\nHello");
        let _ = export_vector(&doc, None, 101);
        let c1 = cache_generation();

        let grad = compile_typst_with_gen(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#rect(width: 20pt, height: 10pt, fill: gradient.linear(red, blue))",
            102,
        );
        assert!(cache_generation() > c1);
        assert!(
            grad.warnings.contains(&DegradationWarning::GradientApproximated),
            "expected gradient approximation warning: {:?}",
            grad.warnings
        );
    }

    #[test]
    fn winding_budget_exceeded_emits_degradation_warning() {
        let mut src = String::from(
            "#set page(width: 200pt, height: 200pt, margin: 0pt)\n#polygon(fill: blue, fill-rule: \"non-zero\"",
        );
        let n = 1501;
        for k in 0..n {
            let v = (k * 4) % n;
            let a = std::f64::consts::TAU * (v as f64) / (n as f64);
            src.push_str(&format!(", ({:.2}pt, {:.2}pt)", 100.0 + 80.0 * a.cos(), 100.0 + 80.0 * a.sin()));
        }
        src.push(')');
        let export = compile_typst(&src);
        assert!(
            export
                .warnings
                .contains(&DegradationWarning::NonZeroWindingApproximated),
            "expected NonZeroWindingApproximated warning, got {:?}",
            export.warnings
        );
    }
}
