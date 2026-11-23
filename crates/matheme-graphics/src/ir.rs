//! Host-agnostic vector graphics intermediate representation (IR).
//!
//! All coordinates are expressed in Typst points (`pt`) in a Y-Up Cartesian
//! coordinate system with `(0, 0)` at the page's bottom-left corner.

use std::sync::Arc;

use typst::foundations::Bytes;
use typst::visualize::{Color, FillRule};

/// RGBA color with components in `0.0..=1.0` (sRGB, straight/unpremultiplied alpha).
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

    pub fn with_opacity(self, opacity: f64) -> Self {
        Self {
            a: (self.a * opacity).clamp(0.0, 1.0),
            ..self
        }
    }

    /// Linearly interpolate two RGBA colors in sRGB space.
    pub fn lerp(self, other: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        let u = 1.0 - t;
        Self {
            r: self.r * u + other.r * t,
            g: self.g * u + other.g * t,
            b: self.b * u + other.b * t,
            a: self.a * u + other.a * t,
        }
    }

    /// Quantize to `[R, G, B, A]` bytes in `0..=255`.
    pub fn to_u8_rgba(self) -> [u8; 4] {
        let q = |v: f64| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
        [q(self.r), q(self.g), q(self.b), q(self.a)]
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

/// How a gradient behaves outside its `[0, 1]` parameter domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientSpread {
    Pad,
    Reflect,
    Repeat,
}

/// One color stop at normalized offset `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub offset: f64,
    pub color: Rgba,
}

/// Host-agnostic vector gradient definition in page Y-Up point coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum GradientPaint {
    /// Linear gradient where parameter `s(p) = (p - origin) · grad` in page Y-Up
    /// coordinates. Exact under arbitrary rotation, reflection, non-uniform
    /// scaling, and skew.
    Linear {
        origin: [f64; 2],
        grad: [f64; 2],
        stops: Vec<GradientStop>,
        spread: GradientSpread,
    },
    /// Two-circle radial gradient defined in local space mapped to page Y-Up
    /// coordinates by column-major affine `local_to_page`.
    Radial {
        local_to_page: [f64; 6],
        center: [f64; 2],
        radius: f64,
        focal_center: [f64; 2],
        focal_radius: f64,
        stops: Vec<GradientStop>,
        spread: GradientSpread,
    },
    /// Conic (sweep) gradient around `center` in local space mapped to page
    /// Y-Up coordinates by column-major affine `local_to_page`.
    Conic {
        /// Affine mapping local container `[0..1]^2` (Y-Down) to page Y-Up points.
        local_to_page: [f64; 6],
        /// Container physical `[width, height]` in local points before extra transform.
        container_size: [f64; 2],
        /// Center in local normalized `[0..1]^2` coordinates.
        center: [f64; 2],
        /// Typst conic start angle in radians (CW from left in container space).
        angle: f64,
        stops: Vec<GradientStop>,
    },
}

/// Repeating 2D vector tiling (Typst `tiling(...)` or SVG `<pattern>`).
#[derive(Debug, Clone, PartialEq)]
pub struct TilingPaint {
    /// Tile cell primitives extracted in the tile's linear-transformed page
    /// orientation with tile origin at `(0, 0)`.
    pub primitives: Vec<VectorPrimitive>,
    /// Tile cell clip dimensions `[w, h]` in local tile coordinates (before
    /// `grid_to_page`).
    pub tile_size: [f64; 2],
    /// Lattice step `[step_x, step_y]` (`size + spacing`) in local tile coordinates.
    pub step: [f64; 2],
    /// Phase offset `[ox, oy]` in local tile coordinates.
    pub offset: [f64; 2],
    /// Column-major affine mapping local tile lattice coordinates `(gx, gy)`
    /// into page Y-Up point coordinates.
    pub grid_to_page: [f64; 6],
}

/// Fill or stroke paint style.
#[derive(Debug, Clone, PartialEq)]
pub enum PaintStyle {
    Solid(Rgba),
    Gradient(GradientPaint),
    Tiling(Box<TilingPaint>),
}

impl PaintStyle {
    pub fn as_solid(&self) -> Option<Rgba> {
        match *self {
            Self::Solid(c) => Some(c),
            _ => None,
        }
    }
}

/// Clip region enclosing child primitives.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipRegion {
    /// Axis-aligned rectangle `[[x_min, x_max], [y_min, y_max]]`.
    Rect([[f64; 2]; 2]),
    /// Arbitrary closed curve clip path (already resolved to EvenOdd winding),
    /// paired with both its geometric bounding box (`bounds`) and its
    /// control-point bounding box (`ctrl_bounds`).
    Path {
        subpaths: Vec<Subpath>,
        bounds: [[f64; 2]; 2],
        ctrl_bounds: [[f64; 2]; 2],
    },
}

impl ClipRegion {
    pub fn bounds(&self) -> [[f64; 2]; 2] {
        match self {
            Self::Rect(b) => *b,
            Self::Path { bounds, .. } => *bounds,
        }
    }
}

/// Encoded container format for fallback embedded images (e.g. embedded SVG
/// `<image>` assets or rasterized/vector PDF fallbacks).
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

/// Decoded 8-bit top-down row-major pixel buffer (`channels` ∈ `{1, 2, 3, 4}`:
/// Grayscale, Grayscale+Alpha, RGB, RGBA).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterPixels {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Arc<[u8]>,
}

/// Payload carried by [`VectorPrimitive::Image`].
#[derive(Debug, Clone, PartialEq)]
pub enum ImagePayload {
    /// Already-decoded 8-bit pixels (Typst `ImageKind::Raster`, with EXIF
    /// orientation already applied), transferred as a packed WXF `NumericArray`.
    Raster(RasterPixels),
    /// Encoded container bytes (embedded `<image>` inside SVG or PDF fallback).
    Encoded {
        format: EmbeddedImageFormat,
        data: Bytes,
    },
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

/// Identifies a specific glyph under a specific quantized 2x2 linear
/// transformation, invariant to page translation `(tx, ty)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub unit: UnitGlyphKey,
    /// Quantized `[sx, ky, kx, sy]` scaled by `LINEAR_QUANT_SCALE`.
    pub linear_q: [i64; 4],
}

impl GlyphKey {
    pub(crate) fn from_quantized(unit: UnitGlyphKey, linear_q: [i64; 4]) -> Self {
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
        payload: ImagePayload,
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
