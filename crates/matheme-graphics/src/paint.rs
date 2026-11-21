//! Host-agnostic paint IR (solid colors, gradients, tilings), clip regions, and
//! exact coordinate/stop mapping math shared between [`crate::extract`] and
//! [`crate::wl`].

use std::f64::consts::TAU;

use kurbo::{Affine, Point};
use typst::layout::{Ratio, Size};
use typst::visualize::{Color, Gradient, GradientStop as TypstGradientStop, RatioOrAngle};

use crate::extract::{Subpath, VectorPrimitive};

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
    ///
    /// For Typst conic gradients, angles are measured in screen/page aspect
    /// space (true circular angle in the container's physical coordinates),
    /// starting at `start_angle` (in radians, Typst CW convention from left
    /// when `start_angle == 0`, converted to `start_angle_rad` in Y-Up page
    /// angle space when `aspect_preserving` is true).
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

/// Expand a Typst [`Gradient`]'s stops into sRGB [`GradientStop`]s, using
/// Typst's own adaptive subdivision so Oklab / Oklch / HSL / HSV / linear-RGB /
/// CMYK gradients reproduce accurately on an sRGB-interpolating host.
pub fn expand_typst_gradient_stops(gradient: &Gradient) -> Vec<GradientStop> {
    let raw = gradient.stops_ref();
    if raw.is_empty() {
        return vec![GradientStop {
            offset: 0.0,
            color: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }];
    }
    if raw.len() == 1 {
        let (c, t) = &raw[0];
        return vec![GradientStop {
            offset: t.get().clamp(0.0, 1.0),
            color: Rgba::from_color(c),
        }];
    }

    let mut out = Vec::with_capacity(raw.len() * 2);
    for window in raw.windows(2) {
        let (start_c, start_t) = &window[0];
        let (end_c, end_t) = &window[1];
        out.push(GradientStop {
            offset: start_t.get().clamp(0.0, 1.0),
            color: Rgba::from_color(start_c),
        });
        if end_t > start_t && gradient.anti_alias() {
            let start = TypstGradientStop::new(start_c.clone(), *start_t);
            let end = TypstGradientStop::new(end_c.clone(), *end_t);
            for (mid_c, mid_t) in gradient.generate_intermediate_stops_for_rgb_interpolation(&start, &end)
            {
                out.push(GradientStop {
                    offset: mid_t.get().clamp(0.0, 1.0),
                    color: Rgba::from_color(&mid_c),
                });
            }
        }
    }
    if let Some((last_c, last_t)) = raw.last() {
        out.push(GradientStop {
            offset: last_t.get().clamp(0.0, 1.0),
            color: Rgba::from_color(last_c),
        });
    }
    out
}

/// Build a [`GradientPaint`] from a Typst [`Gradient`], given the affine
/// `container_to_page` mapping the container's normalized `[0..1]^2` (Y-Down)
/// space into page Y-Up coordinates, and the container's physical `container_size`.
pub fn typst_gradient_to_paint(
    gradient: &Gradient,
    container_to_page: Affine,
    container_size: Size,
) -> PaintStyle {
    let stops = expand_typst_gradient_stops(gradient);
    if stops.len() <= 1 {
        let c = stops
            .first()
            .map_or_else(|| Rgba::from_color(&gradient.sample(RatioOrAngle::Ratio(Ratio::new(0.5)))), |s| s.color);
        return PaintStyle::Solid(c);
    }

    let w = container_size.x.to_pt().abs().max(1e-6);
    let h = container_size.y.to_pt().abs().max(1e-6);
    let aspect = Ratio::new(w / h);

    match gradient {
        Gradient::Linear(linear) => {
            let angle = Gradient::correct_aspect_ratio(linear.angle, aspect);
            let (sin, cos) = (angle.sin(), angle.cos());
            let factor = (sin.abs() + cos.abs()).max(1e-12);
            let (x1, y1) = match angle.quadrant() {
                typst::layout::Quadrant::First => (0.0, 0.0),
                typst::layout::Quadrant::Second => (1.0, 0.0),
                typst::layout::Quadrant::Third => (1.0, 1.0),
                typst::layout::Quadrant::Fourth => (0.0, 1.0),
            };
            let g_uv = [cos / factor, sin / factor];
            match linear_paint_from_local(container_to_page, [x1, y1], g_uv, stops, GradientSpread::Pad) {
                Some(gp) => PaintStyle::Gradient(gp),
                None => PaintStyle::Solid(Rgba::from_color(
                    &gradient.sample(RatioOrAngle::Ratio(Ratio::new(0.5))),
                )),
            }
        }
        Gradient::Radial(radial) => PaintStyle::Gradient(GradientPaint::Radial {
            local_to_page: container_to_page.as_coeffs(),
            center: [radial.center.x.get(), radial.center.y.get()],
            radius: radial.radius.get(),
            focal_center: [radial.focal_center.x.get(), radial.focal_center.y.get()],
            focal_radius: radial.focal_radius.get(),
            stops,
            spread: GradientSpread::Pad,
        }),
        Gradient::Conic(conic) => PaintStyle::Gradient(GradientPaint::Conic {
            local_to_page: container_to_page.as_coeffs(),
            container_size: [w, h],
            center: [conic.center.x.get(), conic.center.y.get()],
            angle: conic.angle.to_rad(),
            stops,
        }),
    }
}

/// Map a local linear gradient `s(u) = (u - local_origin) · g_local` through an
/// affine transform `local_to_page` into page coordinates:
/// `s(p) = (p - origin_page) · g_page`, where `g_page = (A_lin^{-1})^T * g_local`.
pub fn linear_paint_from_local(
    local_to_page: Affine,
    local_origin: [f64; 2],
    g_local: [f64; 2],
    stops: Vec<GradientStop>,
    spread: GradientSpread,
) -> Option<GradientPaint> {
    let [sx, ky, kx, sy, _, _] = local_to_page.as_coeffs();
    let det = sx * sy - kx * ky;
    if !det.is_finite() || det.abs() <= 1e-18 {
        return None;
    }
    let inv_det = 1.0 / det;
    // A_lin = [[sx, kx], [ky, sy]]
    // A_lin^{-1} = inv_det * [[sy, -kx], [-ky, sx]]
    // (A_lin^{-1})^T = inv_det * [[sy, -ky], [-kx, sx]]
    let gx = inv_det * (sy * g_local[0] - ky * g_local[1]);
    let gy = inv_det * (-kx * g_local[0] + sx * g_local[1]);
    if !gx.is_finite() || !gy.is_finite() {
        return None;
    }
    let origin = local_to_page * Point::new(local_origin[0], local_origin[1]);
    Some(GradientPaint::Linear {
        origin: [origin.x, origin.y],
        grad: [gx, gy],
        stops,
        spread,
    })
}

/// Compute the control-point bounding box `[[x_min, x_max], [y_min, y_max]]` of
/// one or more [`Subpath`]s.
///
/// Wolfram Language computes both `Texture` UV bounds and
/// `Linear/Radial/ConicGradientFilling` bounding boxes from `FilledCurve`'s
/// control points (not the tighter geometric Bezier hull), so using this box
/// keeps gradient and texture alignment exact to the pixel.
pub fn subpaths_ctrl_bounds<'a>(subpaths: impl IntoIterator<Item = &'a Subpath>) -> Option<[[f64; 2]; 2]> {
    let mut x0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y0 = f64::INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    let mut include = |p: [f64; 2]| {
        if p[0].is_finite() && p[1].is_finite() {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
    };

    for sp in subpaths {
        if sp.segments.is_empty() {
            continue;
        }
        include(sp.start);
        for seg in &sp.segments {
            match *seg {
                crate::extract::Segment::Line(p) => include(p),
                crate::extract::Segment::Cubic(c1, c2, p) => {
                    include(c1);
                    include(c2);
                    include(p);
                }
            }
        }
    }

    if x0.is_finite() && y0.is_finite() {
        Some([[x0, x1], [y0, y1]])
    } else {
        None
    }
}

/// Fold an arbitrary parameter `s` into `[0.0, 1.0]` according to `spread`.
pub fn apply_spread(s: f64, spread: GradientSpread) -> f64 {
    match spread {
        GradientSpread::Pad => s.clamp(0.0, 1.0),
        GradientSpread::Repeat => s.rem_euclid(1.0),
        GradientSpread::Reflect => {
            let m = s.rem_euclid(2.0);
            if m <= 1.0 { m } else { 2.0 - m }
        }
    }
}

/// Sample a sorted slice of [`GradientStop`]s at parameter `s` under `spread`.
pub fn sample_stops(stops: &[GradientStop], s: f64, spread: GradientSpread) -> Rgba {
    debug_assert!(!stops.is_empty());
    let t = apply_spread(s, spread);
    if stops.len() == 1 || t <= stops[0].offset {
        return stops[0].color;
    }
    let last = stops[stops.len() - 1];
    if t >= last.offset {
        return last.color;
    }
    // Binary search for the first stop with offset >= t.
    let idx = stops.partition_point(|st| st.offset < t);
    if idx == 0 {
        return stops[0].color;
    }
    let a = stops[idx - 1];
    let b = stops[idx];
    let span = b.offset - a.offset;
    if span <= 1e-12 {
        return b.color;
    }
    a.color.lerp(b.color, (t - a.offset) / span)
}

/// Result of projecting a linear gradient onto Wolfram `LinearGradientFilling`'s
/// `[0, 1]` centered-square stop domain.
pub struct ProjectedLinearStops {
    pub stops: Vec<GradientStop>,
    pub spread: GradientSpread,
}

/// Project stops from `s ∈ [0, 1]` via `t = t_start + s * span` (`span > 0`)
/// into Wolfram `LinearGradientFilling`'s strict `[0, 1]` stop domain.
///
/// When `[t_start, t_start + span]` lies inside `[0, 1]`, stops map directly
/// and Wolfram's native padding (`"Fixed"`, `"Periodic"`, `"Reflected"`) handles
/// regions outside `[t_start, t_end]`. When `[t_start, t_end]` extends past
/// `0` or `1`, stops are clipped/unrolled analytically onto `[0, 1]` so Wolfram
/// never rescales out-of-range offsets.
pub fn project_linear_stops_to_unit(
    stops: &[GradientStop],
    t_start: f64,
    span: f64,
    spread: GradientSpread,
) -> ProjectedLinearStops {
    const EPS: f64 = 1e-6;
    let t_end = t_start + span;
    if t_start >= -EPS && t_end <= 1.0 + EPS {
        let mapped = stops
            .iter()
            .map(|st| GradientStop {
                offset: (t_start + st.offset * span).clamp(0.0, 1.0),
                color: st.color,
            })
            .collect();
        return ProjectedLinearStops {
            stops: mapped,
            spread,
        };
    }

    let s_min = -t_start / span;
    let s_max = (1.0 - t_start) / span;
    let mut out = Vec::with_capacity(stops.len() + 2);
    out.push(GradientStop {
        offset: 0.0,
        color: sample_stops(stops, s_min, spread),
    });

    match spread {
        GradientSpread::Pad => {
            for st in stops {
                if st.offset > s_min + EPS && st.offset < s_max - EPS {
                    out.push(GradientStop {
                        offset: (t_start + st.offset * span).clamp(0.0, 1.0),
                        color: st.color,
                    });
                }
            }
        }
        GradientSpread::Repeat | GradientSpread::Reflect => {
            let k_min = s_min.floor() as i64;
            let k_max = s_max.ceil() as i64;
            let max_cycles = 64_i64;
            let k_end = k_max.min(k_min.saturating_add(max_cycles));
            for k in k_min..=k_end {
                let reflected = spread == GradientSpread::Reflect && k.rem_euclid(2) == 1;
                if reflected {
                    for st in stops.iter().rev() {
                        let s = (k as f64) + (1.0 - st.offset);
                        if s > s_min + EPS && s < s_max - EPS {
                            out.push(GradientStop {
                                offset: (t_start + s * span).clamp(0.0, 1.0),
                                color: st.color,
                            });
                        }
                    }
                } else {
                    for st in stops {
                        let s = (k as f64) + st.offset;
                        if s > s_min + EPS && s < s_max - EPS {
                            out.push(GradientStop {
                                offset: (t_start + s * span).clamp(0.0, 1.0),
                                color: st.color,
                            });
                        }
                    }
                }
            }
        }
    }

    out.push(GradientStop {
        offset: 1.0,
        color: sample_stops(stops, s_max, spread),
    });

    ProjectedLinearStops {
        stops: out,
        spread: GradientSpread::Pad,
    }
}

/// Convert conic gradient stops `s ∈ [0, 1]` starting at `start_angle_wl`
/// (radians, CCW from `+x` in Y-Up) advancing in `clockwise_wl` direction into
/// monotonically non-decreasing `(theta_rad, color)` stops on `[0, 2π]` for
/// Wolfram's `ConicGradientFilling`.
pub fn map_conic_stops_to_wl(
    stops: &[GradientStop],
    start_angle_wl: f64,
    clockwise_wl: bool,
) -> Vec<(f64, Rgba)> {
    if stops.is_empty() {
        return Vec::new();
    }
    if stops.len() == 1 {
        let c = stops[0].color;
        return vec![(0.0, c), (TAU, c)];
    }

    // Build full coverage over s in [0, 1] by padding endpoints if stops don't reach 0 / 1.
    let mut full: Vec<GradientStop> = Vec::with_capacity(stops.len() + 2);
    if stops[0].offset > 1e-9 {
        full.push(GradientStop {
            offset: 0.0,
            color: stops[0].color,
        });
    }
    full.extend_from_slice(stops);
    if full.last().unwrap().offset < 1.0 - 1e-9 {
        full.push(GradientStop {
            offset: 1.0,
            color: full.last().unwrap().color,
        });
    }

    // Each interval [s0, s1] maps linearly to a CCW interval [th_lo, th_hi] with colors (c_lo, c_hi).
    struct ArcPiece {
        lo: f64,
        hi: f64,
        c_lo: Rgba,
        c_hi: Rgba,
    }

    let mut pieces: Vec<ArcPiece> = Vec::with_capacity(full.len() + 2);
    let start = start_angle_wl.rem_euclid(TAU);

    let push_wrapped = |pieces: &mut Vec<ArcPiece>, lo: f64, hi: f64, c_lo: Rgba, c_hi: Rgba| {
        if hi - lo <= 1e-12 {
            return;
        }
        // Shift into ranges intersecting [0, TAU].
        let shift = (-lo / TAU).ceil() as i64 - 1;
        for k in shift..=shift + 2 {
            let off = (k as f64) * TAU;
            let a = lo + off;
            let b = hi + off;
            if b <= 1e-12 || a >= TAU - 1e-12 {
                continue;
            }
            let ca = a.max(0.0);
            let cb = b.min(TAU);
            if cb - ca <= 1e-12 {
                continue;
            }
            let span = b - a;
            let t0 = ((ca - a) / span).clamp(0.0, 1.0);
            let t1 = ((cb - a) / span).clamp(0.0, 1.0);
            pieces.push(ArcPiece {
                lo: ca,
                hi: cb,
                c_lo: c_lo.lerp(c_hi, t0),
                c_hi: c_lo.lerp(c_hi, t1),
            });
        }
    };

    for w in full.windows(2) {
        let a = w[0];
        let b = w[1];
        if b.offset - a.offset <= 1e-12 {
            continue;
        }
        if clockwise_wl {
            // s increases => theta decreases: [start - b*TAU, start - a*TAU], color goes b -> a.
            let lo = start - b.offset * TAU;
            let hi = start - a.offset * TAU;
            push_wrapped(&mut pieces, lo, hi, b.color, a.color);
        } else {
            let lo = start + a.offset * TAU;
            let hi = start + b.offset * TAU;
            push_wrapped(&mut pieces, lo, hi, a.color, b.color);
        }
    }

    pieces.sort_by(|x, y| x.lo.total_cmp(&y.lo));

    let mut out: Vec<(f64, Rgba)> = Vec::with_capacity(pieces.len() + 2);
    for piece in pieces {
        match out.last() {
            Some(&(prev_th, prev_c))
                if (prev_th - piece.lo).abs() <= 1e-6 && rgba_approx_eq(prev_c, piece.c_lo) => {}
            _ => out.push((piece.lo, piece.c_lo)),
        }
        out.push((piece.hi, piece.c_hi));
    }
    out
}

fn rgba_approx_eq(a: Rgba, b: Rgba) -> bool {
    (a.r - b.r).abs() <= 1e-5
        && (a.g - b.g).abs() <= 1e-5
        && (a.b - b.b).abs() <= 1e-5
        && (a.a - b.a).abs() <= 1e-5
}

/// Evaluate the exact 2-circle radial gradient parameter `t` at local point `z`,
/// for focal circle `(q, fr)` and end circle `(p, cr)`.
///
/// Matches SVG 2 / Typst / PDF Type 3 radial shading semantics.
pub fn eval_radial_2circle(
    z: [f64; 2],
    p: [f64; 2],
    cr: f64,
    q: [f64; 2],
    fr: f64,
    spread: GradientSpread,
) -> Option<f64> {
    let dx = p[0] - q[0];
    let dy = p[1] - q[1];
    let dr = cr - fr;
    let vx = z[0] - q[0];
    let vy = z[1] - q[1];

    // Degenerate concentric case (fastest exact formula).
    if dx.abs() <= 1e-12 && dy.abs() <= 1e-12 {
        if dr.abs() <= 1e-12 {
            return Some(0.0);
        }
        let dist = vx.hypot(vy);
        return Some((dist - fr) / dr);
    }

    let a = dx * dx + dy * dy - dr * dr;
    let b = vx * dx + vy * dy + fr * dr;
    let c = vx * vx + vy * vy - fr * fr;

    if a.abs() <= 1e-12 {
        if b.abs() <= 1e-12 {
            return None;
        }
        let t = c / (2.0 * b);
        if fr + t * dr >= -1e-9 {
            return Some(t);
        }
        return None;
    }

    let disc = b * b - a * c;
    if disc < 0.0 {
        // Point lies outside the cone formed by the two circles.
        return None;
    }
    let sqrt_d = disc.sqrt();
    // Try larger root first (standard PDF/SVG convention for overlapping circles).
    let t1 = (b + sqrt_d) / a;
    let t2 = (b - sqrt_d) / a;
    let (hi, lo) = if t1 >= t2 { (t1, t2) } else { (t2, t1) };
    if fr + hi * dr >= -1e-9 {
        return Some(hi);
    }
    if fr + lo * dr >= -1e-9 {
        return Some(lo);
    }
    if spread == GradientSpread::Pad && vx.hypot(vy) <= fr + 1e-9 {
        return Some(0.0);
    }
    None
}

/// Rasterize an arbitrary [`GradientPaint`] (such as an off-center focal radial
/// gradient, a rotated/skewed radial gradient, or a skewed conic gradient) over
/// `ctrl_bounds` into an RGBA byte buffer `(width, height, top_down_rgba_bytes)`
/// suitable for Wolfram's `Texture[Image[..., "Byte", ColorSpace -> "RGB"]]`.
///
/// Smooth gradients are band-limited and GPU-bilinear-interpolated by Wolfram's
/// `Texture`, so `max_dim = 96` produces seamless high-DPI rendering in ~0.2 ms.
pub fn rasterize_gradient_texture(
    gradient: &GradientPaint,
    ctrl_bounds: [[f64; 2]; 2],
    max_dim: usize,
) -> Option<(usize, usize, Vec<[u8; 4]>)> {
    let [[x0, x1], [y0, y1]] = ctrl_bounds;
    let cw = (x1 - x0).abs();
    let ch = (y1 - y0).abs();
    if cw <= 1e-9 || ch <= 1e-9 {
        return None;
    }

    let (w, h) = if cw >= ch {
        let h = ((ch / cw) * max_dim as f64).round().clamp(16.0, max_dim as f64) as usize;
        (max_dim, h)
    } else {
        let w = ((cw / ch) * max_dim as f64).round().clamp(16.0, max_dim as f64) as usize;
        (w, max_dim)
    };

    let mut pixels = vec![[0u8; 4]; w * h];
    match gradient {
        GradientPaint::Radial {
            local_to_page,
            center,
            radius,
            focal_center,
            focal_radius,
            stops,
            spread,
        } => {
            let aff = Affine::new(*local_to_page);
            let det = aff.determinant();
            if !det.is_finite() || det.abs() <= 1e-18 {
                return None;
            }
            let page_to_local = aff.inverse();
            for iy in 0..h {
                // Image row 0 is top (y1), row h-1 is bottom (y0).
                let py = y1 - ((iy as f64 + 0.5) / h as f64) * ch;
                for ix in 0..w {
                    let px = x0 + ((ix as f64 + 0.5) / w as f64) * cw;
                    let loc = page_to_local * Point::new(px, py);
                    let rgba = match eval_radial_2circle(
                        [loc.x, loc.y],
                        *center,
                        *radius,
                        *focal_center,
                        *focal_radius,
                        *spread,
                    ) {
                        Some(t) => sample_stops(stops, t, *spread),
                        None => Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        },
                    };
                    pixels[iy * w + ix] = rgba.to_u8_rgba();
                }
            }
        }
        GradientPaint::Conic {
            local_to_page,
            container_size,
            center,
            angle,
            stops,
        } => {
            let aff = Affine::new(*local_to_page);
            let det = aff.determinant();
            if !det.is_finite() || det.abs() <= 1e-18 {
                return None;
            }
            let page_to_local = aff.inverse();
            let [cw_loc, ch_loc] = *container_size;
            for iy in 0..h {
                let py = y1 - ((iy as f64 + 0.5) / h as f64) * ch;
                for ix in 0..w {
                    let px = x0 + ((ix as f64 + 0.5) / w as f64) * cw;
                    let loc = page_to_local * Point::new(px, py);
                    let dx = (loc.x - center[0]) * cw_loc;
                    let dy = (loc.y - center[1]) * ch_loc;
                    let sample_angle = dy.atan2(dx);
                    let t = (std::f64::consts::PI + sample_angle - angle).rem_euclid(TAU) / TAU;
                    pixels[iy * w + ix] = sample_stops(stops, t, GradientSpread::Pad).to_u8_rgba();
                }
            }
        }
        GradientPaint::Linear {
            origin,
            grad,
            stops,
            spread,
        } => {
            for iy in 0..h {
                let py = y1 - ((iy as f64 + 0.5) / h as f64) * ch;
                for ix in 0..w {
                    let px = x0 + ((ix as f64 + 0.5) / w as f64) * cw;
                    let s = (px - origin[0]) * grad[0] + (py - origin[1]) * grad[1];
                    pixels[iy * w + ix] = sample_stops(stops, s, *spread).to_u8_rgba();
                }
            }
        }
    }

    Some((w, h, pixels))
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

    // Account for tile content extending from [0..tile_size] and offset [ox, oy].
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn red() -> Rgba {
        Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }
    }
    fn green() -> Rgba {
        Rgba { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }
    }
    fn blue() -> Rgba {
        Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }
    }
    fn yellow() -> Rgba {
        Rgba { r: 1.0, g: 1.0, b: 0.0, a: 1.0 }
    }

    #[test]
    fn linear_projection_clips_out_of_unit_stops_accurately() {
        let stops = vec![
            GradientStop { offset: 0.0, color: red() },
            GradientStop { offset: 1.0, color: blue() },
        ];
        // Gradient spans [-0.5, 1.5] in WL space -> visible [0, 1] covers s in [0.25, 0.75].
        let proj = project_linear_stops_to_unit(&stops, -0.5, 2.0, GradientSpread::Pad);
        assert_eq!(proj.stops.len(), 2);
        assert!((proj.stops[0].offset - 0.0).abs() < 1e-9);
        assert!((proj.stops[0].color.r - 0.75).abs() < 1e-6);
        assert!((proj.stops[0].color.b - 0.25).abs() < 1e-6);
        assert!((proj.stops[1].offset - 1.0).abs() < 1e-9);
        assert!((proj.stops[1].color.r - 0.25).abs() < 1e-6);
        assert!((proj.stops[1].color.b - 0.75).abs() < 1e-6);
    }

    #[test]
    fn conic_cw_from_left_maps_cardinal_directions_for_wl_ccw() {
        let stops = vec![
            GradientStop { offset: 0.0, color: red() },
            GradientStop { offset: 0.25, color: green() },
            GradientStop { offset: 0.5, color: blue() },
            GradientStop { offset: 0.75, color: yellow() },
            GradientStop { offset: 1.0, color: red() },
        ];
        let wl = map_conic_stops_to_wl(&stops, PI, true);
        assert_eq!(wl.len(), 5);
        assert!((wl[0].0 - 0.0).abs() < 1e-6 && rgba_approx_eq(wl[0].1, blue()));
        assert!((wl[1].0 - FRAC_PI_2).abs() < 1e-6 && rgba_approx_eq(wl[1].1, green()));
        assert!((wl[2].0 - PI).abs() < 1e-6 && rgba_approx_eq(wl[2].1, red()));
        assert!((wl[3].0 - 3.0 * FRAC_PI_2).abs() < 1e-6 && rgba_approx_eq(wl[3].1, yellow()));
        assert!((wl[4].0 - TAU).abs() < 1e-6 && rgba_approx_eq(wl[4].1, blue()));
    }

    #[test]
    fn radial_2circle_matches_concentric_and_off_center_focal() {
        // Concentric: center (0.5, 0.5), r = 0.5, focal = center, fr = 0.
        let t_mid = eval_radial_2circle([0.75, 0.5], [0.5, 0.5], 0.5, [0.5, 0.5], 0.0, GradientSpread::Pad).unwrap();
        assert!((t_mid - 0.5).abs() < 1e-9);
        // Off-center focal at (0.25, 0.5), end circle center (0.5, 0.5), r = 0.5:
        // at z = focal, t == 0; at z = (1.0, 0.5) (on end circle), t == 1.
        let t0 = eval_radial_2circle([0.25, 0.5], [0.5, 0.5], 0.5, [0.25, 0.5], 0.0, GradientSpread::Pad).unwrap();
        let t1 = eval_radial_2circle([1.0, 0.5], [0.5, 0.5], 0.5, [0.25, 0.5], 0.0, GradientSpread::Pad).unwrap();
        assert!(t0.abs() < 1e-9, "t0={t0}");
        assert!((t1 - 1.0).abs() < 1e-9, "t1={t1}");
    }
}
