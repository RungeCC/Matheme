//! Geometry helpers: affine conversions, path conversions, rectangle detection,
//! bounds tracking, and exact/analytical stroke bounding boxes.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use kurbo::{
    Affine, BezPath, Cap, CubicBez, Join, PathEl, Point, QuadBez, Rect, Shape as _, Stroke,
    StrokeOpts,
};
use typst::layout::{Size, Transform};
use typst::visualize::{Curve, CurveItem};
use usvg::tiny_skia_path;

use crate::ir::{LineCapStyle, LineJoinStyle, Segment, StrokeStyle, Subpath};

/// Incremental bounding-box accumulator ignoring non-finite coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BoundsTracker {
    pub(crate) rect: Option<Rect>,
}

impl BoundsTracker {
    pub(crate) fn empty() -> Self {
        Self { rect: None }
    }

    pub(crate) fn from_page(width: f64, height: f64) -> Self {
        Self {
            rect: Some(Rect::new(0.0, 0.0, width, height)),
        }
    }

    pub(crate) fn include_point(&mut self, p: Point) {
        if p.x.is_finite() && p.y.is_finite() {
            self.rect = Some(match self.rect {
                Some(r) => r.union_pt(p),
                None => Rect::from_points(p, p),
            });
        }
    }

    pub(crate) fn include_rect(&mut self, r: Rect) {
        let r = r.abs();
        if r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite() {
            self.rect = Some(match self.rect {
                Some(cur) => cur.union(r),
                None => r,
            });
        }
    }

    pub(crate) fn include_path(&mut self, path: &BezPath, stroke: Option<&StrokeStyle>) {
        if path.elements().is_empty() {
            return;
        }
        let bbox = match stroke.filter(|s| s.thickness > 0.0) {
            Some(style) => stroke_bounding_box(path, style),
            None => path.bounding_box(),
        };
        self.include_rect(bbox);
    }

    pub(crate) fn into_bounds(self) -> [[f64; 2]; 2] {
        match self.rect {
            Some(r) => [[r.x0, r.x1], [r.y0, r.y1]],
            None => [[0.0, 1.0], [0.0, 1.0]],
        }
    }
}

/// Right-handed oriented rectangle `origin + s * dir + t * (-dir[1], dir[0])`
/// for `s in [0, width]`, `t in [0, height]`, where `dir = [cos θ, sin θ]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientedRect {
    pub origin: [f64; 2],
    pub width: f64,
    pub height: f64,
    pub dir: [f64; 2],
}

/// Compute the control-point bounding box `[[x_min, x_max], [y_min, y_max]]` of
/// one or more [`Subpath`]s.
///
/// Wolfram Language computes both `Texture` UV bounds and
/// `Linear/Radial/ConicGradientFilling` bounding boxes from `FilledCurve`'s
/// control points (not the tighter geometric Bezier hull), so using this box
/// keeps gradient and texture alignment exact to the pixel.
pub fn subpaths_ctrl_bounds<'a>(
    subpaths: impl IntoIterator<Item = &'a Subpath>,
) -> Option<[[f64; 2]; 2]> {
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
                Segment::Line(p) => include(p),
                Segment::Cubic(c1, c2, p) => {
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

/// Extract the 4 distinct vertices of a single closed polygonal subpath if it
/// consists purely of line segments and has exactly 4 non-degenerate corners.
fn single_closed_quad(subpaths: &[Subpath]) -> Option<[[f64; 2]; 4]> {
    if subpaths.len() != 1 {
        return None;
    }
    let sp = &subpaths[0];
    if !sp.closed || sp.segments.len() < 3 || sp.segments.len() > 5 {
        return None;
    }
    if !sp.start[0].is_finite() || !sp.start[1].is_finite() {
        return None;
    }
    let mut verts = Vec::with_capacity(5);
    verts.push(sp.start);
    for seg in &sp.segments {
        let Segment::Line(p) = *seg else {
            return None;
        };
        if !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        let prev = *verts.last().unwrap();
        if (p[0] - prev[0]).hypot(p[1] - prev[1]) > 1e-6 {
            verts.push(p);
        }
    }
    if verts.len() >= 2 {
        let first = verts[0];
        let last = *verts.last().unwrap();
        if (last[0] - first[0]).hypot(last[1] - first[1]) <= 1e-6 {
            verts.pop();
        }
    }
    if verts.len() != 4 {
        return None;
    }
    Some([verts[0], verts[1], verts[2], verts[3]])
}

/// If `subpaths` is a single closed axis-aligned rectangle (including 90°/180°/270°
/// rotations), return its `[[x_min, x_max], [y_min, y_max]]` bounds.
pub fn subpaths_axis_aligned_rect(subpaths: &[Subpath]) -> Option<[[f64; 2]; 2]> {
    let verts = single_closed_quad(subpaths)?;
    let mut x0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y0 = f64::INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    for &[x, y] in &verts {
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let w = x1 - x0;
    let h = y1 - y0;
    if w <= 1e-6 || h <= 1e-6 {
        return None;
    }
    let tol = 1e-5 * w.max(h);
    let mut prev_horiz: Option<bool> = None;
    for i in 0..4 {
        let a = verts[i];
        let b = verts[(i + 1) % 4];
        let dx = (b[0] - a[0]).abs();
        let dy = (b[1] - a[1]).abs();
        let is_horiz = if dy <= tol && dx > tol {
            true
        } else if dx <= tol && dy > tol {
            false
        } else {
            return None;
        };
        if prev_horiz == Some(is_horiz) {
            return None;
        }
        prev_horiz = Some(is_horiz);
    }
    Some([[x0, x1], [y0, y1]])
}

/// If `subpaths` is a single closed (possibly rotated) rectangle, return its
/// right-handed [`OrientedRect`] parameterization.
pub fn subpaths_oriented_rect(subpaths: &[Subpath]) -> Option<OrientedRect> {
    let [v0, v1, v2, v3] = single_closed_quad(subpaths)?;
    let mut u = [v1[0] - v0[0], v1[1] - v0[1]];
    let mut v = [v3[0] - v0[0], v3[1] - v0[1]];
    let mut w = u[0].hypot(u[1]);
    let mut h = v[0].hypot(v[1]);
    if w <= 1e-6 || h <= 1e-6 {
        return None;
    }
    let tol = 1e-5 * w.max(h);
    // Parallelogram closure: v2 == v0 + u + v.
    let err_x = v2[0] - (v0[0] + u[0] + v[0]);
    let err_y = v2[1] - (v0[1] + u[1] + v[1]);
    if err_x.hypot(err_y) > tol {
        return None;
    }
    // Right-angle check: u ⊥ v.
    let dot = u[0] * v[0] + u[1] * v[1];
    if dot.abs() > 1e-5 * w * h {
        return None;
    }
    // Ensure right-handed frame (det(u, v) > 0).
    let det = u[0] * v[1] - u[1] * v[0];
    if det < 0.0 {
        std::mem::swap(&mut u, &mut v);
        std::mem::swap(&mut w, &mut h);
    }
    Some(OrientedRect {
        origin: v0,
        width: w,
        height: h,
        dir: [u[0] / w, u[1] / w],
    })
}

pub(crate) fn affine_from_typst(ts: Transform) -> Affine {
    Affine::new([
        ts.sx.get(),
        ts.ky.get(),
        ts.kx.get(),
        ts.sy.get(),
        ts.tx.to_pt(),
        ts.ty.to_pt(),
    ])
}

pub(crate) fn affine_from_usvg(ts: tiny_skia_path::Transform) -> Affine {
    Affine::new([
        f64::from(ts.sx),
        f64::from(ts.ky),
        f64::from(ts.kx),
        f64::from(ts.sy),
        f64::from(ts.tx),
        f64::from(ts.ty),
    ])
}

pub(crate) fn strip_translation(affine: Affine) -> Affine {
    let [sx, ky, kx, sy, _, _] = affine.as_coeffs();
    Affine::new([sx, ky, kx, sy, 0.0, 0.0])
}

pub(crate) fn affine_stroke_scale(affine: Affine) -> f64 {
    let [sx, ky, kx, sy, _, _] = affine.as_coeffs();
    0.5 * (sx.hypot(ky) + kx.hypot(sy))
}

pub(crate) fn include_transformed_box(w: f64, h: f64, affine: Affine, bbox: &mut BoundsTracker) {
    for pt in [
        Point::new(0.0, 0.0),
        Point::new(w, 0.0),
        Point::new(w, h),
        Point::new(0.0, h),
    ] {
        bbox.include_point(affine * pt);
    }
}

pub(crate) fn kurbo_stroke_from_style(style: &StrokeStyle) -> Stroke {
    let cap = match style.cap {
        LineCapStyle::Butt => Cap::Butt,
        LineCapStyle::Round => Cap::Round,
        LineCapStyle::Square => Cap::Square,
    };
    Stroke {
        width: style.thickness,
        join: match style.join {
            LineJoinStyle::Miter => Join::Miter,
            LineJoinStyle::Round => Join::Round,
            LineJoinStyle::Bevel => Join::Bevel,
        },
        miter_limit: style.miter_limit.max(1.0),
        start_cap: cap,
        end_cap: cap,
        ..Stroke::default()
    }
}

pub(crate) fn stroke_hash(s: &StrokeStyle) -> u64 {
    let mut h = DefaultHasher::new();
    s.thickness.to_bits().hash(&mut h);
    (s.cap as u8).hash(&mut h);
    (s.join as u8).hash(&mut h);
    s.miter_limit.to_bits().hash(&mut h);
    h.finish()
}

pub(crate) fn bez_path_hash(path: &BezPath) -> u64 {
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
pub(crate) fn line_stroke_bbox(start: Point, end: Point, style: &StrokeStyle) -> Rect {
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
pub(crate) fn axis_aligned_rect_stroke_bbox(rect: Rect, style: &StrokeStyle) -> Rect {
    let hw = 0.5 * style.thickness;
    rect.abs().inflate(hw, hw)
}

pub(crate) fn stroke_bounding_box(path: &BezPath, style: &StrokeStyle) -> Rect {
    if let [PathEl::MoveTo(start), PathEl::LineTo(end)] = path.elements() {
        return line_stroke_bbox(*start, *end, style);
    }

    let kstroke = kurbo_stroke_from_style(style);
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

pub(crate) fn rect_bez_path(size: Size, affine: Affine) -> BezPath {
    let mut bez = Rect::new(0.0, 0.0, size.x.to_pt(), size.y.to_pt()).to_path(0.1);
    bez.apply_affine(affine);
    bez
}

pub(crate) fn curve_to_bez_path(curve: &Curve, affine: Affine) -> BezPath {
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

pub(crate) fn usvg_path_to_bez(path: &tiny_skia_path::Path, affine: Affine) -> BezPath {
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
pub(crate) fn transform_subpaths_and_bbox(
    subpaths: &[Subpath],
    linear: Affine,
) -> Option<(Vec<Subpath>, Rect)> {
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

pub(crate) fn translate_subpaths_into(
    subpaths: &[Subpath],
    delta: [f64; 2],
    out: &mut Vec<Subpath>,
) {
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

pub(crate) fn subpaths_to_bez_path(subpaths: &[Subpath]) -> BezPath {
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

pub(crate) fn bez_path_to_subpaths(path: &BezPath) -> Vec<Subpath> {
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

pub(crate) fn is_axis_aligned_rect(path: &BezPath) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rgba;

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
        assert!(
            (b.x0 - 0.0).abs() < 1e-6 && (b.x1 - 40.0).abs() < 1e-6,
            "butt cap must not expand x: {b:?}"
        );
        assert!(
            (b.y0 - 7.0).abs() < 1e-6 && (b.y1 - 13.0).abs() < 1e-6,
            "butt cap y: {b:?}"
        );

        let square = StrokeStyle { cap: LineCapStyle::Square, ..butt.clone() };
        let sq = line_stroke_bbox(Point::new(0.0, 10.0), Point::new(40.0, 10.0), &square);
        assert!(
            (sq.x0 - (-3.0)).abs() < 1e-6 && (sq.x1 - 43.0).abs() < 1e-6,
            "square cap x: {sq:?}"
        );

        let round = StrokeStyle { cap: LineCapStyle::Round, ..butt };
        let rd = line_stroke_bbox(Point::new(0.0, 10.0), Point::new(40.0, 10.0), &round);
        assert!(
            (rd.x0 - (-3.0)).abs() < 1e-6 && (rd.x1 - 43.0).abs() < 1e-6,
            "round cap x: {rd:?}"
        );
    }

    #[test]
    fn detects_axis_aligned_and_oriented_rectangles() {
        let rect = vec![Subpath {
            start: [10.0, 20.0],
            segments: vec![
                Segment::Line([110.0, 20.0]),
                Segment::Line([110.0, 60.0]),
                Segment::Line([10.0, 60.0]),
                Segment::Line([10.0, 20.0]),
            ],
            closed: true,
        }];
        assert_eq!(subpaths_axis_aligned_rect(&rect), Some([[10.0, 110.0], [20.0, 60.0]]));

        let ang = 30.0_f64.to_radians();
        let (c, s) = (ang.cos(), ang.sin());
        let p0 = [5.0, 8.0];
        let p1 = [p0[0] + 50.0 * c, p0[1] + 50.0 * s];
        let p2 = [p1[0] - 20.0 * s, p1[1] + 20.0 * c];
        let p3 = [p0[0] - 20.0 * s, p0[1] + 20.0 * c];
        let rot = vec![Subpath {
            start: p0,
            segments: vec![Segment::Line(p1), Segment::Line(p2), Segment::Line(p3)],
            closed: true,
        }];
        assert_eq!(subpaths_axis_aligned_rect(&rot), None);
        let orect = subpaths_oriented_rect(&rot).expect("rotated rect");
        assert!((orect.width - 50.0).abs() < 1e-6);
        assert!((orect.height - 20.0).abs() < 1e-6);
        assert!((orect.dir[0] - c).abs() < 1e-6 && (orect.dir[1] - s).abs() < 1e-6);

        let holed = vec![rect[0].clone(), rect[0].clone()];
        assert_eq!(subpaths_axis_aligned_rect(&holed), None);
        assert_eq!(subpaths_oriented_rect(&holed), None);

        let curved = vec![Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Cubic([10.0, 0.0], [20.0, 10.0], [20.0, 20.0]),
                Segment::Line([0.0, 20.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        }];
        assert_eq!(subpaths_axis_aligned_rect(&curved), None);
        assert_eq!(subpaths_oriented_rect(&curved), None);
    }
}
