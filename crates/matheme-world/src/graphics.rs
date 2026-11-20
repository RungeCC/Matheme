//! Host-agnostic vector graphics extraction from a compiled Typst [`PagedDocument`].
//!
//! Instead of serializing to SVG/PDF and round-tripping through an external
//! importer (`PDFTools.dll` / `resvg.dll` / `pdfium.dll`), this module walks
//! each [`Page`]'s [`Frame`] tree directly, outlines glyphs via `ttf-parser`,
//! transforms all coordinates into a Y-Up Cartesian plane (`(0, 0)` at the
//! bottom-left of the page, units in points), and produces a compact
//! [`VectorPage`] IR ready for direct conversion into host `Graphics` primitives.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use ttf_parser::{GlyphId, OutlineBuilder};
use typst::foundations::Smart;
use typst::layout::{Abs, Axes, Frame, FrameItem, GroupItem, PageRanges, Ratio, Size, Transform};
use typst::text::color::{glyph_frame, should_outline, GlyphFrameItem};
use typst::text::{FontInstance, TextItem};
use typst::visualize::{
    Color, Curve, CurveItem, FillRule, FixedStroke, Geometry, Image, LineCap, LineJoin, Paint,
    RatioOrAngle, Shape,
};
use typst_layout::{Page, PagedDocument};
use typst_svg::WebImage;

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

    pub fn from_paint(paint: &Paint) -> Self {
        match paint {
            Paint::Solid(color) => Self::from_color(color),
            Paint::Gradient(gradient) => {
                Self::from_color(&gradient.sample(RatioOrAngle::Ratio(Ratio::new(0.5))))
            }
            Paint::Tiling(_) => Self {
                r: 0.5,
                g: 0.5,
                b: 0.5,
                a: 1.0,
            },
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

impl Subpath {
    fn transform(&self, ts: &Affine2D, bbox: &mut BBox) -> Self {
        let start = ts.apply(self.start);
        bbox.include(start);
        let segments = self
            .segments
            .iter()
            .map(|seg| match *seg {
                Segment::Line(p) => {
                    let tp = ts.apply(p);
                    bbox.include(tp);
                    Segment::Line(tp)
                }
                Segment::Cubic(c1, c2, p) => {
                    let tc1 = ts.apply(c1);
                    let tc2 = ts.apply(c2);
                    let tp = ts.apply(p);
                    bbox.include(tc1);
                    bbox.include(tc2);
                    bbox.include(tp);
                    Segment::Cubic(tc1, tc2, tp)
                }
            })
            .collect();
        Self {
            start,
            segments,
            closed: self.closed,
        }
    }
}

/// A host-neutral vector/raster drawing primitive on a [`VectorPage`].
#[derive(Debug, Clone, PartialEq)]
pub enum VectorPrimitive {
    /// One or more compound paths (e.g. all outlined glyphs of a `TextItem`,
    /// or a `Shape`) sharing the same fill and stroke.
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
    /// An embedded image placed inside the axis-aligned box `[min, max]`.
    Image {
        format: &'static str,
        data: Vec<u8>,
        min: [f64; 2],
        max: [f64; 2],
    },
}

/// One exported page in Y-Up point coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorPage {
    pub width: f64,
    pub height: f64,
    pub background: Option<Rgba>,
    /// Union of the page box `[[0, width], [0, height]]` and all primitive points.
    pub plot_range: [[f64; 2]; 2],
    pub primitives: Vec<VectorPrimitive>,
}

/// Export selected pages of a [`PagedDocument`] into [`VectorPage`]s.
pub fn export_graphics(document: &PagedDocument, pages: Option<&PageRanges>) -> Vec<VectorPage> {
    let mut extractor = Extractor::default();
    document
        .pages()
        .iter()
        .enumerate()
        .filter(|(idx, _)| {
            pages.is_none_or(|ranges| {
                NonZeroUsize::new(idx + 1).is_some_and(|n| ranges.includes_page(n))
            })
        })
        .map(|(_, page)| extractor.extract_page(page))
        .collect()
}

/// 2D affine matrix in point coordinates:
/// `x' = sx * x + kx * y + tx`
/// `y' = ky * x + sy * y + ty`
#[derive(Debug, Clone, Copy)]
struct Affine2D {
    sx: f64,
    ky: f64,
    kx: f64,
    sy: f64,
    tx: f64,
    ty: f64,
}

impl Affine2D {
    fn from_typst(ts: Transform) -> Self {
        Self {
            sx: ts.sx.get(),
            ky: ts.ky.get(),
            kx: ts.kx.get(),
            sy: ts.sy.get(),
            tx: ts.tx.to_pt(),
            ty: ts.ty.to_pt(),
        }
    }

    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.sx * p[0] + self.kx * p[1] + self.tx,
            self.ky * p[0] + self.sy * p[1] + self.ty,
        ]
    }

    /// Average scale factor for stroke widths under this transform.
    fn stroke_scale(&self) -> f64 {
        let sx = (self.sx * self.sx + self.ky * self.ky).sqrt();
        let sy = (self.kx * self.kx + self.sy * self.sy).sqrt();
        0.5 * (sx + sy)
    }
}

#[derive(Debug, Clone, Copy)]
struct BBox {
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
}

impl BBox {
    fn from_page(width: f64, height: f64) -> Self {
        Self {
            x_min: 0.0,
            x_max: width,
            y_min: 0.0,
            y_max: height,
        }
    }

    fn include(&mut self, p: [f64; 2]) {
        if p[0].is_finite() && p[1].is_finite() {
            self.x_min = self.x_min.min(p[0]);
            self.x_max = self.x_max.max(p[0]);
            self.y_min = self.y_min.min(p[1]);
            self.y_max = self.y_max.max(p[1]);
        }
    }
}

#[derive(Default)]
struct Extractor {
    /// Cached glyph outlines in normalized `1 / units_per_em` coordinates.
    glyph_cache: HashMap<(FontInstance, u16), Option<Vec<Subpath>>>,
}

impl Extractor {
    fn extract_page(&mut self, page: &Page) -> VectorPage {
        let size = page.frame.size().max(Size::splat(Abs::pt(1.0)));
        let width = size.x.to_pt();
        let height = size.y.to_pt();

        let background = match &page.fill {
            Smart::Auto => None,
            Smart::Custom(fill) => fill.as_ref().map(Rgba::from_paint),
        };

        // Flip from Typst's top-left Y-Down frame to WL's bottom-left Y-Up frame:
        // (x, y) -> (x, height - y).
        let root_ts = Transform::translate(Abs::zero(), size.y)
            .pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));

        let mut bbox = BBox::from_page(width, height);
        let mut primitives = Vec::new();

        if let Some(bg) = background {
            let rect = rect_subpath(size, &Affine2D::from_typst(root_ts), &mut bbox);
            primitives.push(VectorPrimitive::Paths {
                paths: vec![vec![rect]],
                fill: Some(bg),
                stroke: None,
                winding: WindingRule::NonZero,
            });
        }

        self.extract_frame(&page.frame, root_ts, &mut bbox, &mut primitives);

        VectorPage {
            width,
            height,
            background,
            plot_range: [[bbox.x_min, bbox.x_max], [bbox.y_min, bbox.y_max]],
            primitives,
        }
    }

    fn extract_frame(
        &mut self,
        frame: &Frame,
        ts: Transform,
        bbox: &mut BBox,
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
        bbox: &mut BBox,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        let group_ts = ts.pre_concat(group.transform);
        self.extract_frame(&group.frame, group_ts, bbox, primitives);
    }

    fn extract_text(
        &mut self,
        text: &TextItem,
        ts: Transform,
        bbox: &mut BBox,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        // Font glyph outlines are defined in a Y-Up coordinate system, whereas
        // Typst's local text frame is Y-Down. Flip Y locally so that composed
        // with `root_ts` (which also flips Y), the glyph outlines end up Y-Up.
        let text_ts = ts.pre_concat(Transform::scale(Ratio::one(), -Ratio::one()));
        let font_size_pt = text.size.to_pt();
        let text_affine = Affine2D::from_typst(text_ts);

        let fill = Some(Rgba::from_paint(&text.fill));
        let stroke = text
            .stroke
            .as_ref()
            .map(|s| convert_stroke(s, text_affine.stroke_scale()));

        let mut glyph_paths: Vec<Vec<Subpath>> = Vec::with_capacity(text.glyphs.len());
        let mut x = Abs::zero();
        let mut y = Abs::zero();

        for glyph in &text.glyphs {
            let id = GlyphId(glyph.id);
            let x_offset = x + glyph.x_offset.at(text.size);
            let y_offset = y + glyph.y_offset.at(text.size);

            if should_outline(&text.font, id) {
                if let Some(unit_subpaths) = self.outline_glyph_unit(&text.font, id) {
                    let glyph_ts = text_ts
                        .pre_concat(Transform::translate(x_offset, y_offset))
                        .pre_concat(Transform::scale(
                            Ratio::new(font_size_pt),
                            Ratio::new(font_size_pt),
                        ));
                    let affine = Affine2D::from_typst(glyph_ts);
                    let transformed: Vec<Subpath> = unit_subpaths
                        .iter()
                        .map(|sp| sp.transform(&affine, bbox))
                        .collect();
                    if !transformed.is_empty() {
                        glyph_paths.push(transformed);
                    }
                }
            } else if let Some(gframe) = glyph_frame(&text.font, id.0) {
                if !glyph_paths.is_empty() {
                    primitives.push(VectorPrimitive::Paths {
                        paths: std::mem::take(&mut glyph_paths),
                        fill,
                        stroke: stroke.clone(),
                        winding: WindingRule::NonZero,
                    });
                }
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

        if !glyph_paths.is_empty() {
            primitives.push(VectorPrimitive::Paths {
                paths: glyph_paths,
                fill,
                stroke,
                winding: WindingRule::NonZero,
            });
        }
    }

    fn outline_glyph_unit(&mut self, font: &FontInstance, id: GlyphId) -> Option<&[Subpath]> {
        let key = (font.clone(), id.0);
        let entry = self.glyph_cache.entry(key).or_insert_with(|| {
            let upem = font.units_per_em();
            if upem <= 0.0 {
                return None;
            }
            let mut builder = PathBuilder::new(1.0 / upem);
            font.ttf().outline_glyph(id, &mut builder)?;
            let subpaths = builder.finish();
            (!subpaths.is_empty()).then_some(subpaths)
        });
        entry.as_deref()
    }

    fn extract_shape(
        &mut self,
        shape: &Shape,
        ts: Transform,
        bbox: &mut BBox,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        let fill = shape.fill.as_ref().map(Rgba::from_paint);
        let affine = Affine2D::from_typst(ts);
        let stroke = shape
            .stroke
            .as_ref()
            .map(|s| convert_stroke(s, affine.stroke_scale()));

        if fill.is_none() && stroke.is_none() {
            return;
        }

        match &shape.geometry {
            Geometry::Line(end) => {
                if let Some(stroke) = stroke {
                    let start = affine.apply([0.0, 0.0]);
                    let end = affine.apply([end.x.to_pt(), end.y.to_pt()]);
                    bbox.include(start);
                    bbox.include(end);
                    primitives.push(VectorPrimitive::Line { start, end, stroke });
                }
            }
            Geometry::Rect(size) => {
                let subpath = rect_subpath(*size, &affine, bbox);
                primitives.push(VectorPrimitive::Paths {
                    paths: vec![vec![subpath]],
                    fill,
                    stroke,
                    winding: shape.fill_rule.into(),
                });
            }
            Geometry::Curve(curve) => {
                let subpaths = convert_curve(curve, &affine, bbox);
                if !subpaths.is_empty() {
                    primitives.push(VectorPrimitive::Paths {
                        paths: vec![subpaths],
                        fill,
                        stroke,
                        winding: shape.fill_rule.into(),
                    });
                }
            }
        }
    }

    fn extract_image(
        &mut self,
        image: &Image,
        size: &Axes<Abs>,
        ts: Transform,
        bbox: &mut BBox,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        let web = WebImage::new(image);
        let affine = Affine2D::from_typst(ts);
        let corners = [
            affine.apply([0.0, 0.0]),
            affine.apply([size.x.to_pt(), 0.0]),
            affine.apply([size.x.to_pt(), size.y.to_pt()]),
            affine.apply([0.0, size.y.to_pt()]),
        ];
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for c in corners {
            bbox.include(c);
            x_min = x_min.min(c[0]);
            x_max = x_max.max(c[0]);
            y_min = y_min.min(c[1]);
            y_max = y_max.max(c[1]);
        }
        primitives.push(VectorPrimitive::Image {
            format: web.format.extension(),
            data: web.data.to_vec(),
            min: [x_min, y_min],
            max: [x_max, y_max],
        });
    }
}

fn convert_stroke(stroke: &FixedStroke, scale: f64) -> StrokeStyle {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    StrokeStyle {
        color: Rgba::from_paint(&stroke.paint),
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
        dash: stroke.dash.as_ref().map(|d| {
            (
                d.array.iter().map(|len| len.to_pt() * s).collect(),
                d.phase.to_pt() * s,
            )
        }),
    }
}

fn rect_subpath(size: Size, affine: &Affine2D, bbox: &mut BBox) -> Subpath {
    let w = size.x.to_pt();
    let h = size.y.to_pt();
    let p0 = affine.apply([0.0, 0.0]);
    let p1 = affine.apply([w, 0.0]);
    let p2 = affine.apply([w, h]);
    let p3 = affine.apply([0.0, h]);
    for p in [p0, p1, p2, p3] {
        bbox.include(p);
    }
    Subpath {
        start: p0,
        segments: vec![
            Segment::Line(p1),
            Segment::Line(p2),
            Segment::Line(p3),
            Segment::Line(p0),
        ],
        closed: true,
    }
}

fn convert_curve(curve: &Curve, affine: &Affine2D, bbox: &mut BBox) -> Vec<Subpath> {
    let mut builder = PathBuilder::new(1.0);
    for item in &curve.0 {
        match *item {
            CurveItem::Move(p) => builder.move_pt(affine.apply([p.x.to_pt(), p.y.to_pt()])),
            CurveItem::Line(p) => builder.line_pt(affine.apply([p.x.to_pt(), p.y.to_pt()])),
            CurveItem::Cubic(p1, p2, p3) => builder.cubic_pt(
                affine.apply([p1.x.to_pt(), p1.y.to_pt()]),
                affine.apply([p2.x.to_pt(), p2.y.to_pt()]),
                affine.apply([p3.x.to_pt(), p3.y.to_pt()]),
            ),
            CurveItem::Close => builder.close_subpath(),
        }
    }
    let subpaths = builder.finish();
    for sp in &subpaths {
        bbox.include(sp.start);
        for seg in &sp.segments {
            match *seg {
                Segment::Line(p) => bbox.include(p),
                Segment::Cubic(c1, c2, p) => {
                    bbox.include(c1);
                    bbox.include(c2);
                    bbox.include(p);
                }
            }
        }
    }
    subpaths
}

struct PathBuilder {
    scale: f64,
    subpaths: Vec<Subpath>,
    current_start: Option<[f64; 2]>,
    current_pos: [f64; 2],
    current_segments: Vec<Segment>,
}

impl PathBuilder {
    fn new(scale: f64) -> Self {
        Self {
            scale,
            subpaths: Vec::new(),
            current_start: None,
            current_pos: [0.0, 0.0],
            current_segments: Vec::new(),
        }
    }

    fn flush_open(&mut self) {
        if let Some(start) = self.current_start.take() {
            if !self.current_segments.is_empty() {
                self.subpaths.push(Subpath {
                    start,
                    segments: std::mem::take(&mut self.current_segments),
                    closed: false,
                });
            }
        }
    }

    fn move_pt(&mut self, p: [f64; 2]) {
        self.flush_open();
        self.current_start = Some(p);
        self.current_pos = p;
    }

    fn line_pt(&mut self, p: [f64; 2]) {
        if self.current_start.is_none() {
            self.current_start = Some(self.current_pos);
        }
        if (p[0] - self.current_pos[0]).abs() > 1e-9 || (p[1] - self.current_pos[1]).abs() > 1e-9 {
            self.current_segments.push(Segment::Line(p));
            self.current_pos = p;
        }
    }

    fn quad_pt(&mut self, q1: [f64; 2], p3: [f64; 2]) {
        if self.current_start.is_none() {
            self.current_start = Some(self.current_pos);
        }
        let p0 = self.current_pos;
        // Exact degree elevation from quadratic (p0, q1, p3) to cubic (p0, c1, c2, p3).
        let c1 = [
            p0[0] + (2.0 / 3.0) * (q1[0] - p0[0]),
            p0[1] + (2.0 / 3.0) * (q1[1] - p0[1]),
        ];
        let c2 = [
            p3[0] + (2.0 / 3.0) * (q1[0] - p3[0]),
            p3[1] + (2.0 / 3.0) * (q1[1] - p3[1]),
        ];
        self.current_segments.push(Segment::Cubic(c1, c2, p3));
        self.current_pos = p3;
    }

    fn cubic_pt(&mut self, c1: [f64; 2], c2: [f64; 2], p3: [f64; 2]) {
        if self.current_start.is_none() {
            self.current_start = Some(self.current_pos);
        }
        self.current_segments.push(Segment::Cubic(c1, c2, p3));
        self.current_pos = p3;
    }

    fn close_subpath(&mut self) {
        if let Some(start) = self.current_start.take() {
            if !self.current_segments.is_empty() {
                if (self.current_pos[0] - start[0]).abs() > 1e-9
                    || (self.current_pos[1] - start[1]).abs() > 1e-9
                {
                    self.current_segments.push(Segment::Line(start));
                }
                self.subpaths.push(Subpath {
                    start,
                    segments: std::mem::take(&mut self.current_segments),
                    closed: true,
                });
            }
            self.current_pos = start;
        }
    }

    fn finish(mut self) -> Vec<Subpath> {
        self.flush_open();
        self.subpaths
    }
}

impl OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.move_pt([f64::from(x) * self.scale, f64::from(y) * self.scale]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.line_pt([f64::from(x) * self.scale, f64::from(y) * self.scale]);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.quad_pt(
            [f64::from(x1) * self.scale, f64::from(y1) * self.scale],
            [f64::from(x) * self.scale, f64::from(y) * self.scale],
        );
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cubic_pt(
            [f64::from(x1) * self.scale, f64::from(y1) * self.scale],
            [f64::from(x2) * self.scale, f64::from(y2) * self.scale],
            [f64::from(x) * self.scale, f64::from(y) * self.scale],
        );
    }

    fn close(&mut self) {
        self.close_subpath();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::MathemeWorld;
    use std::path::PathBuf;
    use typst::foundations::Dict;

    #[test]
    fn extracts_glyph_outlines_and_expands_descender_bbox() {
        let src = "#set page(width: auto, height: auto, margin: 0pt, fill: none)\nxxyyzz";
        let world = MathemeWorld::new(src.into(), Dict::new(), PathBuf::from("."));
        let doc = typst::compile::<PagedDocument>(&world)
            .output
            .expect("compile xxyyzz");
        let pages = export_graphics(&doc, None);
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert!(page.width > 20.0);
        assert!(page.height > 5.0);
        // 'y' descender extends below y = 0 in a 0pt-margin page, so plot_range.y_min < 0.
        assert!(
            page.plot_range[1][0] < -1.0,
            "expected negative y_min for 'y' descender, got {:?}",
            page.plot_range
        );
        let glyph_count: usize = page
            .primitives
            .iter()
            .map(|p| match p {
                VectorPrimitive::Paths { paths, .. } => paths.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(glyph_count, 6, "expected 6 outlined glyphs for 'xxyyzz'");
    }

    #[test]
    fn extracts_unicode_math_curves_not_tofu() {
        let src = "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$ x^y + integral_0^infinity e^(-x^2) dif x $";
        let world = MathemeWorld::new(src.into(), Dict::new(), PathBuf::from("."));
        let doc = typst::compile::<PagedDocument>(&world)
            .output
            .expect("compile math");
        let pages = export_graphics(&doc, None);
        assert_eq!(pages.len(), 1);
        let cubic_count: usize = pages[0]
            .primitives
            .iter()
            .flat_map(|p| match p {
                VectorPrimitive::Paths { paths, .. } => paths.as_slice(),
                _ => &[],
            })
            .flatten()
            .flat_map(|sp| &sp.segments)
            .filter(|seg| matches!(seg, Segment::Cubic(..)))
            .count();
        assert!(
            cubic_count > 50,
            "expected curved glyph outlines for unicode-math, got {cubic_count} cubic segments"
        );
    }
}
