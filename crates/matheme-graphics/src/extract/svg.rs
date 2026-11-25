//! Embedded SVG extraction via `usvg` (groups, paths, gradients, patterns,
//! clip paths, flattened text, and nested images).

use std::sync::Arc;

use kurbo::{Affine, Rect};
use typst::foundations::Bytes;
use typst::layout::{Abs, Size};

use super::{
    DegradationWarnings, Extractor, MAX_TILING_DEPTH, push_painted_stroke, push_resolved_paths,
};
use crate::geometry::{
    BoundsTracker, affine_from_usvg, affine_stroke_scale, bez_path_to_subpaths,
    include_transformed_box, is_axis_aligned_rect, strip_translation, subpaths_ctrl_bounds,
    usvg_path_to_bez,
};
use crate::ir::{
    ClipRegion, EmbeddedImageFormat, GradientPaint, GradientSpread, GradientStop, ImagePayload,
    LineCapStyle, LineJoinStyle, PaintStyle, Rgba, StrokeStyle, Subpath, TilingPaint,
    VectorPrimitive, WindingRule,
};
use crate::paint::linear_paint_from_local;
use crate::winding::{self, WindingResolution};

impl Extractor {
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
                        Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: opacity,
                        },
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
                        Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: opacity,
                        },
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

                tile_primitives = self.clip_overflowing_tile(
                    tile_primitives,
                    tile_bbox,
                    Size::new(Abs::pt(rw), Abs::pt(rh)),
                    linear,
                );

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

    pub(super) fn extract_usvg_group(
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

        if let Some(clip) = group.clip_path()
            && let Some(region) = usvg_clip_region(clip, canvas_to_page, &mut self.warnings)
        {
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
            if !child_primitives.is_empty() && clip_rect.width() > 0.0 && clip_rect.height() > 0.0 {
                primitives.push(VectorPrimitive::Clip {
                    region,
                    children: child_primitives,
                });
            }
            return;
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
                                Rgba {
                                    r: 0.0,
                                    g: 0.0,
                                    b: 0.0,
                                    a: 1.0,
                                },
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
                    self.extract_usvg_group(
                        text.flattened(),
                        canvas_to_page,
                        opacity,
                        bbox,
                        primitives,
                    );
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
                                payload: ImagePayload::Encoded {
                                    format,
                                    data: Bytes::new(ArcBytes(Arc::clone(bytes))),
                                },
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
