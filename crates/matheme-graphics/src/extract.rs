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
use typst::layout::{Abs, Axes, Frame, FrameItem, FrameKind, GroupItem, PageRanges, Ratio, Size, Transform};
use typst::text::color::{glyph_frame, should_outline, GlyphFrameItem};
use typst::text::{Font, FontInstance, TextItem};
use typst::visualize::{
    Curve, CurveItem, FillRule, FixedStroke, Geometry, Image, ImageKind, LineCap, LineJoin, Paint,
    RelativeTo, Shape, Tiling,
};
use typst_layout::{Page, PagedDocument};
use typst_svg::WebImage;
use usvg::tiny_skia_path;

use crate::lru::{lock_unpoisoned, LruCache};
use crate::paint::{
    linear_paint_from_local, subpaths_ctrl_bounds, typst_gradient_to_paint, ClipRegion,
    GradientPaint, GradientSpread, GradientStop, PaintStyle, Rgba, TilingPaint,
};
use crate::winding::{self, WindingResolution};

/// Shared upper bound across all glyph cache tiers (unit outlines, scaled
/// outlines, and host-encoded expressions) using O(1) LRU eviction.
pub(crate) const GLYPH_CACHE_CAPACITY: usize = 8192;

/// Maximum nested tiling recursion depth before falling back to solid gray.
const MAX_TILING_DEPTH: usize = 4;

/// Scale factor used to quantize linear transform components (`1e-6` precision)
/// so floating-point noise never pollutes the glyph key space.
const LINEAR_QUANT_SCALE: f64 = 1_000_000.0;

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
    /// Instanced text glyphs sharing the same solid fill and solid stroke.
    Glyphs {
        glyphs: Vec<GlyphInstances>,
        fill: Option<Rgba>,
        stroke: Option<StrokeStyle>,
    },
    /// One or more compound paths (e.g. a `Shape`, painted text run, or
    /// flattened SVG path).
    Paths {
        paths: Vec<Vec<Subpath>>,
        fill: Option<PaintStyle>,
        stroke: Option<StrokeStyle>,
        winding: WindingRule,
    },
    /// A stroked two-point line segment with solid stroke paint.
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
    /// Rectangular or arbitrary curved clip region enclosing nested primitives.
    Clip {
        region: ClipRegion,
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
    TilingDepthExceeded,
    UnsupportedSvgFeature,
    NonZeroWindingApproximated,
    UnsupportedImageFormat,
}

impl DegradationWarning {
    /// Host-neutral human-readable warning text describing the approximation.
    pub fn message(self) -> &'static str {
        match self {
            Self::TilingDepthExceeded => {
                "Deeply nested tiling patterns exceeded the maximum recursion depth and were approximated by solid gray."
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

#[derive(Debug, Clone, Copy)]
struct ParentContext {
    size: Size,
    transform: Transform,
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

    fn convert_typst_paint(
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

        // Clip overflowing tile cell content to the cell's `[0..tw] × [0..th]` box,
        // matching Typst's `tiling` cell clipping semantics.
        let cell_bez = rect_bez_path(tile_size, linear);
        let cell_rect = cell_bez.bounding_box().abs();
        let overflows = tile_bbox.rect.is_some_and(|tb| {
            tb.x0 < cell_rect.x0 - 1e-3
                || tb.x1 > cell_rect.x1 + 1e-3
                || tb.y0 < cell_rect.y0 - 1e-3
                || tb.y1 > cell_rect.y1 + 1e-3
        });
        if overflows && !tile_primitives.is_empty() && cell_rect.width() > 0.0 && cell_rect.height() > 0.0 {
            let region = self.bez_to_clip_region(&cell_bez, cell_rect);
            tile_primitives = vec![VectorPrimitive::Clip {
                region,
                children: tile_primitives,
            }];
        }

        PaintStyle::Tiling(Box::new(TilingPaint {
            primitives: tile_primitives,
            tile_size: [tw, th],
            step: [step_x, step_y],
            offset: [ox, oy],
            grid_to_page: grid_to_page.as_coeffs(),
        }))
    }

    fn bez_to_clip_region(&mut self, clip_bez: &BezPath, clip_rect: Rect) -> ClipRegion {
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
        parent: ParentContext,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        // Font outlines are defined in Y-Up, whereas Typst's local text frame
        // is Y-Down. Flip Y locally so composed with `root_ts`, glyphs end up Y-Up.
        let text_ts = ts.pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
        let font_size_pt = text.size.to_pt();
        let text_affine = affine_from_typst(text_ts);
        let stroke_scale = affine_stroke_scale(text_affine);

        let fill_is_solid = matches!(text.fill, Paint::Solid(_));
        let stroke_is_solid = text
            .stroke
            .as_ref()
            .is_none_or(|s| matches!(s.paint, Paint::Solid(_)));

        let scale_ts = Transform::scale(Ratio::new(font_size_pt), Ratio::new(font_size_pt));
        let linear_affine =
            strip_translation(affine_from_typst(text_ts.pre_concat(scale_ts)));
        let linear_q = quantize_linear(linear_affine);
        let run_key = self.font_run_key(&text.font);

        if fill_is_solid && stroke_is_solid {
            let fill = match &text.fill {
                Paint::Solid(c) => Some(Rgba::from_color(c)),
                _ => unreachable!(),
            };
            let stroke = text
                .stroke
                .as_ref()
                .map(|s| convert_solid_stroke(s, stroke_scale));

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
                            self.extract_shape(shape, glyph_ts, parent, bbox, primitives);
                        }
                        GlyphFrameItem::Image(_, image, size) => {
                            self.extract_image(image, size, glyph_ts, bbox, primitives);
                        }
                    }
                }

                x += glyph.x_advance.at(text.size);
                y += glyph.y_advance.at(text.size);
            }
            return;
        }

        // Gradient- or tiling-painted text: materialize the run's glyph outlines
        // as translated subpaths so Wolfram's native gradient/texture fills span
        // the whole text run cleanly (without `GeometricTransformation` limitations).
        self.active_glyph_indices.clear();
        let geom_stroke = text
            .stroke
            .as_ref()
            .map(|s| convert_stroke_geometry(s, stroke_scale));

        let mut run_subpaths: Vec<Subpath> = Vec::new();
        let mut run_bbox = BoundsTracker::empty();
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
                    let glyph_bbox = self.glyph_origin_bbox(key, &scaled, geom_stroke.as_ref());
                    let shifted = glyph_bbox + pos.to_vec2();
                    bbox.include_rect(shifted);
                    run_bbox.include_rect(shifted);
                    translate_subpaths_into(&scaled.subpaths, [pos.x, pos.y], &mut run_subpaths);
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
                        self.extract_shape(shape, glyph_ts, parent, bbox, primitives);
                    }
                    GlyphFrameItem::Image(_, image, size) => {
                        self.extract_image(image, size, glyph_ts, bbox, primitives);
                    }
                }
            }

            x += glyph.x_advance.at(text.size);
            y += glyph.y_advance.at(text.size);
        }

        if run_subpaths.is_empty() {
            return;
        }

        // Self container (only used if the user explicitly set `relative: "self"` on text).
        let self_rect = run_bbox.rect.unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
        let rw = self_rect.width().max(1e-6);
        let rh = self_rect.height().max(1e-6);
        let self_container_to_page =
            Affine::translate((self_rect.x0, self_rect.y1)) * Affine::scale_non_uniform(rw, -rh);
        let self_container_size = Size::new(Abs::pt(rw), Abs::pt(rh));
        let self_tiling_to_page = affine_from_typst(ts);

        let fill_paint = self.convert_typst_paint(
            &text.fill,
            self_container_to_page,
            self_container_size,
            self_tiling_to_page,
            parent,
            true,
        );

        let (solid_stroke, painted_stroke) = match text.stroke.as_ref() {
            None => (None, None),
            Some(s) if matches!(s.paint, Paint::Solid(_)) => {
                (Some(convert_solid_stroke(s, stroke_scale)), None)
            }
            Some(s) => {
                let paint = self.convert_typst_paint(
                    &s.paint,
                    self_container_to_page,
                    self_container_size,
                    self_tiling_to_page,
                    parent,
                    true,
                );
                let style = geom_stroke.clone().unwrap();
                (None, Some((style, paint)))
            }
        };

        if let Some((stroke_style, stroke_paint)) = painted_stroke {
            let stroke_bez = subpaths_to_bez_path(&run_subpaths);
            primitives.push(VectorPrimitive::Paths {
                paths: vec![run_subpaths],
                fill: Some(fill_paint),
                stroke: None,
                winding: WindingRule::EvenOdd,
            });
            push_painted_stroke(
                primitives,
                &stroke_bez,
                &stroke_style,
                stroke_paint,
                &mut self.warnings,
            );
        } else {
            primitives.push(VectorPrimitive::Paths {
                paths: vec![run_subpaths],
                fill: Some(fill_paint),
                stroke: solid_stroke,
                winding: WindingRule::EvenOdd,
            });
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
                (None, Some((convert_stroke_geometry(s, stroke_scale), paint)))
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
                } else if let Some((stroke_style, stroke_paint)) = painted_stroke {
                    if stroke_style.thickness > 0.0 {
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

    fn extract_usvg_paint(
        &mut self,
        paint: &usvg::Paint,
        paint_opacity: f32,
        group_opacity: f64,
        path_affine: Affine,
    ) -> PaintStyle {
        let opacity = (f64::from(paint_opacity) * group_opacity).clamp(0.0, 1.0);
        match paint {
            usvg::Paint::Color(c) => PaintStyle::Solid(Rgba {
                r: f64::from(c.red) / 255.0,
                g: f64::from(c.green) / 255.0,
                b: f64::from(c.blue) / 255.0,
                a: opacity,
            }),
            usvg::Paint::LinearGradient(g) => {
                let stops = usvg_stops_to_ir(g.stops(), opacity);
                if stops.len() <= 1 {
                    return PaintStyle::Solid(stops.first().map_or(
                        Rgba { r: 0.0, g: 0.0, b: 0.0, a: opacity },
                        |s| s.color,
                    ));
                }
                let local_to_page = path_affine * affine_from_usvg(g.transform());
                let x1 = f64::from(g.x1());
                let y1 = f64::from(g.y1());
                let dx = f64::from(g.x2()) - x1;
                let dy = f64::from(g.y2()) - y1;
                let len2 = dx * dx + dy * dy;
                if len2 <= 1e-18 {
                    return PaintStyle::Solid(stops[0].color);
                }
                let g_local = [dx / len2, dy / len2];
                match linear_paint_from_local(
                    local_to_page,
                    [x1, y1],
                    g_local,
                    stops.clone(),
                    usvg_spread(g.spread_method()),
                ) {
                    Some(gp) => PaintStyle::Gradient(gp),
                    None => PaintStyle::Solid(stops[stops.len() / 2].color),
                }
            }
            usvg::Paint::RadialGradient(g) => {
                let stops = usvg_stops_to_ir(g.stops(), opacity);
                if stops.len() <= 1 {
                    return PaintStyle::Solid(stops.first().map_or(
                        Rgba { r: 0.0, g: 0.0, b: 0.0, a: opacity },
                        |s| s.color,
                    ));
                }
                let local_to_page = path_affine * affine_from_usvg(g.transform());
                PaintStyle::Gradient(GradientPaint::Radial {
                    local_to_page: local_to_page.as_coeffs(),
                    center: [f64::from(g.cx()), f64::from(g.cy())],
                    radius: f64::from(g.r().get()),
                    focal_center: [f64::from(g.fx()), f64::from(g.fy())],
                    focal_radius: f64::from(g.fr().get()),
                    stops,
                    spread: usvg_spread(g.spread_method()),
                })
            }
            usvg::Paint::Pattern(pat) => {
                if self.tiling_depth >= MAX_TILING_DEPTH {
                    self.warnings.tiling_depth = true;
                    return PaintStyle::Solid(Rgba {
                        r: 0.5,
                        g: 0.5,
                        b: 0.5,
                        a: opacity,
                    });
                }
                let rect = pat.rect();
                let rw = f64::from(rect.width()).max(1e-6);
                let rh = f64::from(rect.height()).max(1e-6);
                let rx = f64::from(rect.x());
                let ry = f64::from(rect.y());
                let grid_to_page = path_affine * affine_from_usvg(pat.transform());
                let linear = strip_translation(grid_to_page);

                let saved_glyph_indices = std::mem::take(&mut self.active_glyph_indices);
                self.tiling_depth += 1;

                let mut tile_primitives = Vec::new();
                let mut tile_bbox = BoundsTracker::empty();
                self.extract_usvg_group(
                    pat.root(),
                    linear,
                    opacity,
                    &mut tile_bbox,
                    &mut tile_primitives,
                );

                self.tiling_depth -= 1;
                self.active_glyph_indices = saved_glyph_indices;

                let cell_bez = rect_bez_path(Size::new(Abs::pt(rw), Abs::pt(rh)), linear);
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
                    tile_primitives = vec![VectorPrimitive::Clip {
                        region,
                        children: tile_primitives,
                    }];
                }

                PaintStyle::Tiling(Box::new(TilingPaint {
                    primitives: tile_primitives,
                    tile_size: [rw, rh],
                    step: [rw, rh],
                    offset: [rx, ry],
                    grid_to_page: grid_to_page.as_coeffs(),
                }))
            }
        }
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
            if let Some(region) = usvg_clip_region(clip, canvas_to_page, &mut self.warnings) {
                let [[x0, x1], [y0, y1]] = region.bounds();
                let clip_rect = Rect::new(x0, y0, x1, y1);
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
                        region,
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
                    let fill = path.fill().map(|f| {
                        self.extract_usvg_paint(f.paint(), f.opacity().get(), opacity, path_affine)
                    });
                    let fill_rule = path
                        .fill()
                        .map_or(WindingRule::NonZero, |f| usvg_fill_rule(f.rule()));
                    let stroke_scale = affine_stroke_scale(path_affine);
                    let (solid_stroke, painted_stroke) = match path.stroke() {
                        None => (None, None),
                        Some(s) if matches!(s.paint(), usvg::Paint::Color(_)) => {
                            let color = match s.paint() {
                                usvg::Paint::Color(c) => Rgba {
                                    r: f64::from(c.red) / 255.0,
                                    g: f64::from(c.green) / 255.0,
                                    b: f64::from(c.blue) / 255.0,
                                    a: (f64::from(s.opacity().get()) * opacity).clamp(0.0, 1.0),
                                },
                                _ => unreachable!(),
                            };
                            (Some(usvg_stroke_with_color(s, stroke_scale, color)), None)
                        }
                        Some(s) => {
                            let paint = self.extract_usvg_paint(
                                s.paint(),
                                s.opacity().get(),
                                opacity,
                                path_affine,
                            );
                            let style = usvg_stroke_with_color(
                                s,
                                stroke_scale,
                                Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
                            );
                            (None, Some((style, paint)))
                        }
                    };
                    if fill.is_none() && solid_stroke.is_none() && painted_stroke.is_none() {
                        continue;
                    }
                    let bbox_stroke = solid_stroke
                        .as_ref()
                        .or_else(|| painted_stroke.as_ref().map(|(s, _)| s));
                    self.include_stroked_path(bbox, &bez, bbox_stroke);
                    let subpaths = bez_path_to_subpaths(&bez);
                    if fill.is_some() || solid_stroke.is_some() {
                        push_resolved_paths(
                            primitives,
                            subpaths,
                            fill,
                            solid_stroke,
                            fill_rule,
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

fn shape_self_container(shape: &Shape, item_affine: Affine, include_stroke: bool) -> (Affine, Size) {
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

fn translate_subpaths_into(subpaths: &[Subpath], delta: [f64; 2], out: &mut Vec<Subpath>) {
    let [dx, dy] = delta;
    for sp in subpaths {
        if sp.segments.is_empty() {
            continue;
        }
        let segs = sp
            .segments
            .iter()
            .map(|seg| match *seg {
                Segment::Line([x, y]) => Segment::Line([x + dx, y + dy]),
                Segment::Cubic([c1x, c1y], [c2x, c2y], [px, py]) => {
                    Segment::Cubic([c1x + dx, c1y + dy], [c2x + dx, c2y + dy], [px + dx, py + dy])
                }
            })
            .collect();
        out.push(Subpath {
            start: [sp.start[0] + dx, sp.start[1] + dy],
            segments: segs,
            closed: sp.closed,
        });
    }
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

fn usvg_clip_region(
    clip: &usvg::ClipPath,
    canvas_to_page: Affine,
    warnings: &mut DegradationWarnings,
) -> Option<ClipRegion> {
    let mut all_rect = true;
    let mut tracker = BoundsTracker::empty();
    let mut subpaths: Vec<Subpath> = Vec::new();
    let mut path_count = 0_usize;
    let clip_affine = canvas_to_page * affine_from_usvg(clip.transform());
    collect_usvg_clip_paths(
        clip.root(),
        clip_affine,
        &mut all_rect,
        &mut path_count,
        &mut tracker,
        &mut subpaths,
        warnings,
    );
    let r = tracker.rect?.abs();
    let bounds = [[r.x0, r.x1], [r.y0, r.y1]];
    if all_rect && path_count == 1 {
        return Some(ClipRegion::Rect(bounds));
    }
    if subpaths.is_empty() {
        return None;
    }
    let ctrl_bounds = subpaths_ctrl_bounds(&subpaths).unwrap_or(bounds);
    Some(ClipRegion::Path {
        subpaths,
        bounds,
        ctrl_bounds,
    })
}

fn collect_usvg_clip_paths(
    group: &usvg::Group,
    clip_affine: Affine,
    all_rect: &mut bool,
    path_count: &mut usize,
    tracker: &mut BoundsTracker,
    out_subpaths: &mut Vec<Subpath>,
    warnings: &mut DegradationWarnings,
) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => {
                collect_usvg_clip_paths(
                    g,
                    clip_affine,
                    all_rect,
                    path_count,
                    tracker,
                    out_subpaths,
                    warnings,
                );
            }
            usvg::Node::Path(path) => {
                *path_count += 1;
                let p_aff = clip_affine * affine_from_usvg(path.abs_transform());
                let bez = usvg_path_to_bez(path.data(), p_aff);
                if !is_axis_aligned_rect(&bez) {
                    *all_rect = false;
                }
                tracker.include_path(&bez, None);
                let raw = bez_path_to_subpaths(&bez);
                let rule = path
                    .fill()
                    .map_or(WindingRule::NonZero, |f| usvg_fill_rule(f.rule()));
                if rule == WindingRule::NonZero {
                    match winding::resolve_nonzero_subpaths(&raw) {
                        WindingResolution::Unchanged => out_subpaths.extend(raw),
                        WindingResolution::Resolved(resolved) => out_subpaths.extend(resolved),
                        WindingResolution::BudgetExceeded | WindingResolution::ExtractionFailed => {
                            warnings.winding = true;
                            out_subpaths.extend(raw);
                        }
                    }
                } else {
                    out_subpaths.extend(raw);
                }
            }
            usvg::Node::Text(text) => {
                *all_rect = false;
                collect_usvg_clip_paths(
                    text.flattened(),
                    clip_affine,
                    all_rect,
                    path_count,
                    tracker,
                    out_subpaths,
                    warnings,
                );
            }
            usvg::Node::Image(_) => {}
        }
    }
    if *path_count > 1 {
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

fn convert_stroke_geometry(stroke: &FixedStroke, scale: f64) -> StrokeStyle {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dash = stroke.dash.as_ref().map(|d| {
        let phase = d.phase.to_pt() * s;
        (
            d.array.iter().map(|len| len.to_pt() * s).collect(),
            phase,
        )
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

fn convert_solid_stroke(stroke: &FixedStroke, scale: f64) -> StrokeStyle {
    let mut style = convert_stroke_geometry(stroke, scale);
    if let Paint::Solid(ref color) = stroke.paint {
        style.color = Rgba::from_color(color);
    }
    style
}

fn usvg_fill_rule(rule: usvg::FillRule) -> WindingRule {
    match rule {
        usvg::FillRule::NonZero => WindingRule::NonZero,
        usvg::FillRule::EvenOdd => WindingRule::EvenOdd,
    }
}

fn usvg_spread(spread: usvg::SpreadMethod) -> GradientSpread {
    match spread {
        usvg::SpreadMethod::Pad => GradientSpread::Pad,
        usvg::SpreadMethod::Reflect => GradientSpread::Reflect,
        usvg::SpreadMethod::Repeat => GradientSpread::Repeat,
    }
}

fn usvg_stops_to_ir(stops: &[usvg::Stop], opacity: f64) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|s| {
            let c = s.color();
            GradientStop {
                offset: f64::from(s.offset().get()).clamp(0.0, 1.0),
                color: Rgba {
                    r: f64::from(c.red) / 255.0,
                    g: f64::from(c.green) / 255.0,
                    b: f64::from(c.blue) / 255.0,
                    a: (f64::from(s.opacity().get()) * opacity).clamp(0.0, 1.0),
                },
            }
        })
        .collect()
}

fn usvg_stroke_with_color(stroke: &usvg::Stroke, scale: f64, color: Rgba) -> StrokeStyle {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let dash = stroke.dasharray().map(|arr| {
        let phase = f64::from(stroke.dashoffset()) * s;
        (arr.iter().map(|v| f64::from(*v) * s).collect(), phase)
    });
    StrokeStyle {
        color,
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
    fn extracts_gradients_tilings_dash_phase_and_non_rect_clip_without_warnings() {
        // Use local stub generations (decoupled from host font store state).
        let doc = compile_doc("#set page(width: auto, height: auto, margin: 0pt, fill: none)\nHello");
        let _ = export_vector(&doc, None, 101);
        let c1 = cache_generation();

        let grad = compile_typst_with_gen(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#rect(width: 20pt, height: 10pt, fill: gradient.linear(red, blue))",
            102,
        );
        assert!(cache_generation() > c1);
        assert!(grad.warnings.is_empty(), "unexpected warnings: {:?}", grad.warnings);
        assert!(
            grad.pages[0].primitives.iter().any(|p| matches!(
                p,
                VectorPrimitive::Paths {
                    fill: Some(PaintStyle::Gradient(GradientPaint::Linear { .. })),
                    ..
                }
            )),
            "expected GradientPaint::Linear primitive"
        );

        let radial = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#circle(radius: 15pt, fill: gradient.radial(white, black))",
        );
        assert!(radial.warnings.is_empty(), "unexpected warnings: {:?}", radial.warnings);
        assert!(radial.pages[0].primitives.iter().any(|p| matches!(
            p,
            VectorPrimitive::Paths {
                fill: Some(PaintStyle::Gradient(GradientPaint::Radial { .. })),
                ..
            }
        )));

        let conic = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#circle(radius: 15pt, fill: gradient.conic(red, green, blue, red))",
        );
        assert!(conic.warnings.is_empty(), "unexpected warnings: {:?}", conic.warnings);
        assert!(conic.pages[0].primitives.iter().any(|p| matches!(
            p,
            VectorPrimitive::Paths {
                fill: Some(PaintStyle::Gradient(GradientPaint::Conic { .. })),
                ..
            }
        )));

        let tiling = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#let p = tiling(size: (10pt, 10pt), circle(radius: 3pt, fill: green))\n#rect(width: 40pt, height: 40pt, fill: p)",
        );
        assert!(tiling.warnings.is_empty(), "unexpected warnings: {:?}", tiling.warnings);
        assert!(tiling.pages[0].primitives.iter().any(|p| matches!(
            p,
            VectorPrimitive::Paths {
                fill: Some(PaintStyle::Tiling(_)),
                ..
            }
        )));

        let dashed = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#line(length: 40pt, stroke: (paint: black, thickness: 2pt, dash: (array: (6pt, 3pt), phase: 2pt)))",
        );
        assert!(dashed.warnings.is_empty(), "unexpected warnings: {:?}", dashed.warnings);
        assert!(dashed.pages[0].primitives.iter().any(|p| matches!(
            p,
            VectorPrimitive::Line { stroke, .. }
                if stroke.dash.as_ref().is_some_and(|(_, ph)| (*ph - 2.0).abs() < 1e-6)
        )));

        let non_rect_clip = compile_typst(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n#box(width: 30pt, height: 30pt, radius: 10pt, clip: true, rect(width: 30pt, height: 30pt, fill: red))",
        );
        assert!(
            non_rect_clip.warnings.is_empty(),
            "unexpected warnings: {:?}",
            non_rect_clip.warnings
        );
        assert!(non_rect_clip.pages[0].primitives.iter().any(|p| matches!(
            p,
            VectorPrimitive::Clip {
                region: ClipRegion::Path { .. },
                ..
            }
        )));
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
