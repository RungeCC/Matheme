//! Gradient stop expansion, coordinate projection, two-circle radial evaluation,
//! and RGBA texture rasterization for non-native gradient configurations.

use std::f64::consts::TAU;

use kurbo::{Affine, Point};
use typst::layout::{Ratio, Size};
use typst::visualize::{Gradient, GradientStop as TypstGradientStop, RatioOrAngle};

use crate::ir::{GradientPaint, GradientSpread, GradientStop, PaintStyle, Rgba};

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
            for (mid_c, mid_t) in
                gradient.generate_intermediate_stops_for_rgb_interpolation(&start, &end)
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
        let c = stops.first().map_or_else(
            || Rgba::from_color(&gradient.sample(RatioOrAngle::Ratio(Ratio::new(0.5)))),
            |s| s.color,
        );
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
            match linear_paint_from_local(
                container_to_page,
                [x1, y1],
                g_uv,
                stops,
                GradientSpread::Pad,
            ) {
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
        return None;
    }
    let sqrt_d = disc.sqrt();
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
/// `ctrl_bounds` into a flat top-down RGBA byte buffer
/// `(width, height, rgba_bytes)` of length `width * height * 4`, suitable for
/// zero-copy WXF `NumericArray` transfer into Wolfram's
/// `Texture[Image[NumericArray[...], "Byte", ColorSpace -> "RGB"]]`.
pub fn rasterize_gradient_texture(
    gradient: &GradientPaint,
    ctrl_bounds: [[f64; 2]; 2],
    max_dim: usize,
) -> Option<(usize, usize, Vec<u8>)> {
    let [[x0, x1], [y0, y1]] = ctrl_bounds;
    let cw = (x1 - x0).abs();
    let ch = (y1 - y0).abs();
    if cw <= 1e-9 || ch <= 1e-9 {
        return None;
    }

    let (w, h) = if cw >= ch {
        let h = ((ch / cw) * max_dim as f64)
            .round()
            .clamp(16.0, max_dim as f64) as usize;
        (max_dim, h)
    } else {
        let w = ((cw / ch) * max_dim as f64)
            .round()
            .clamp(16.0, max_dim as f64) as usize;
        (w, max_dim)
    };

    let mut pixels = vec![0u8; w * h * 4];
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
                    let off = (iy * w + ix) * 4;
                    pixels[off..off + 4].copy_from_slice(&rgba.to_u8_rgba());
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
                    let off = (iy * w + ix) * 4;
                    pixels[off..off + 4]
                        .copy_from_slice(&sample_stops(stops, t, GradientSpread::Pad).to_u8_rgba());
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
                    let off = (iy * w + ix) * 4;
                    pixels[off..off + 4]
                        .copy_from_slice(&sample_stops(stops, s, *spread).to_u8_rgba());
                }
            }
        }
    }

    Some((w, h, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn red() -> Rgba {
        Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }
    fn green() -> Rgba {
        Rgba {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        }
    }
    fn blue() -> Rgba {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }
    }
    fn yellow() -> Rgba {
        Rgba {
            r: 1.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        }
    }

    #[test]
    fn linear_projection_clips_out_of_unit_stops_accurately() {
        let stops = vec![
            GradientStop {
                offset: 0.0,
                color: red(),
            },
            GradientStop {
                offset: 1.0,
                color: blue(),
            },
        ];
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
            GradientStop {
                offset: 0.0,
                color: red(),
            },
            GradientStop {
                offset: 0.25,
                color: green(),
            },
            GradientStop {
                offset: 0.5,
                color: blue(),
            },
            GradientStop {
                offset: 0.75,
                color: yellow(),
            },
            GradientStop {
                offset: 1.0,
                color: red(),
            },
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
        let t_mid = eval_radial_2circle(
            [0.75, 0.5],
            [0.5, 0.5],
            0.5,
            [0.5, 0.5],
            0.0,
            GradientSpread::Pad,
        )
        .unwrap();
        assert!((t_mid - 0.5).abs() < 1e-9);
        let t0 = eval_radial_2circle(
            [0.25, 0.5],
            [0.5, 0.5],
            0.5,
            [0.25, 0.5],
            0.0,
            GradientSpread::Pad,
        )
        .unwrap();
        let t1 = eval_radial_2circle(
            [1.0, 0.5],
            [0.5, 0.5],
            0.5,
            [0.25, 0.5],
            0.0,
            GradientSpread::Pad,
        )
        .unwrap();
        assert!(t0.abs() < 1e-9, "t0={t0}");
        assert!((t1 - 1.0).abs() < 1e-9, "t1={t1}");
    }
}
