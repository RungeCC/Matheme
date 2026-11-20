//! Conversion of [`matheme_world::graphics::VectorPage`] into native Wolfram
//! Language `System\`Graphics` [`Expr`]s.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use matheme_wl::{Expr, Symbol};
use matheme_world::graphics::{
    cache_generation, EmbeddedImageFormat, GlyphInstances, GlyphKey, LineCapStyle, LineJoinStyle,
    Rgba, Segment, StrokeStyle, Subpath, VectorPage, VectorPrimitive,
};

/// Upper bound on cached `FilledCurve` / `JoinedCurve` `Expr` entries in the
/// LibraryLink bridge.
const MAX_EXPR_CACHE_ENTRIES: usize = 4096;

#[derive(Clone)]
struct CachedGlyphExpr {
    all_closed: bool,
    filled: Expr,
    joined: Option<Expr>,
}

#[derive(Default)]
struct GlyphExprCache {
    generation: u64,
    entries: HashMap<GlyphKey, CachedGlyphExpr>,
}

static GLYPH_EXPR_CACHE: LazyLock<RwLock<GlyphExprCache>> =
    LazyLock::new(|| RwLock::new(GlyphExprCache::default()));

/// Convert one [`VectorPage`] into a `System\`Graphics` [`Expr`] with explicit
/// numeric `PlotRange`, zero padding, and exact pt `ImageSize`.
pub fn page_to_expr(page: &VectorPage) -> Expr {
    let [[x0, x1], [y0, y1]] = page.bounds;
    let plot_width = (x1 - x0).abs().max(1e-6);
    let img_w = (x1 - x0).max(1.0);
    let img_h = (y1 - y0).max(1.0);

    let mut elements = Vec::with_capacity(page.primitives.len());
    for primitive in &page.primitives {
        if let Some(expr) = primitive_to_expr(primitive, plot_width) {
            elements.push(expr);
        }
    }

    Expr::normal(
        Symbol::system("Graphics"),
        [
            Expr::list(elements),
            rule_sym_expr("ImageSize", Expr::list([img_w, img_h])),
            rule_sym_expr(
                "PlotRange",
                Expr::list([Expr::list([x0, x1]), Expr::list([y0, y1])]),
            ),
            rule_sym_sym("PlotRangePadding", "None"),
            rule_sym_sym("ImagePadding", "None"),
            rule_sym_sym("AspectRatio", "Automatic"),
        ],
    )
}

fn rule_sym_expr(lhs: &str, rhs: Expr) -> Expr {
    Expr::normal(Symbol::RULE, [Expr::symbol(Symbol::system(lhs)), rhs])
}

fn rule_sym_sym(lhs: &str, rhs: &str) -> Expr {
    rule_sym_expr(lhs, Expr::symbol(Symbol::system(rhs)))
}

fn primitive_to_expr(primitive: &VectorPrimitive, plot_width: f64) -> Option<Expr> {
    match primitive {
        VectorPrimitive::Glyphs {
            glyphs,
            fill,
            stroke,
        } => glyphs_to_expr(glyphs, *fill, stroke.as_ref(), plot_width),
        VectorPrimitive::Paths {
            paths,
            fill,
            stroke,
            ..
        } => paths_to_expr(paths, *fill, stroke.as_ref(), plot_width),
        VectorPrimitive::Line { start, end, stroke } => {
            let mut group = stroke_directives(stroke, plot_width);
            group.push(Expr::normal(
                Symbol::system("Line"),
                [Expr::list([point_expr(*start), point_expr(*end)])],
            ));
            Some(Expr::list(group))
        }
        VectorPrimitive::Image {
            format,
            data,
            size,
            transform,
        } => Some(image_to_expr(*format, data.as_ref(), *size, *transform)),
        VectorPrimitive::Clip { bounds, children } => clip_to_expr(*bounds, children),
    }
}

fn image_to_expr(
    format: EmbeddedImageFormat,
    data: &[u8],
    size: [f64; 2],
    transform: [f64; 6],
) -> Expr {
    let imported = Expr::normal(
        Symbol::system("ImportByteArray"),
        [Expr::bytes(data), Expr::string(format.wl_format())],
    );
    let stretched = Expr::normal(
        Symbol::system("Show"),
        [imported, rule_sym_sym("AspectRatio", "Full")],
    );
    let [sx, ky, kx, sy, tx, ty] = transform;
    let origin = Expr::list([
        Expr::symbol(Symbol::system("Left")),
        Expr::symbol(Symbol::system("Bottom")),
    ]);

    if kx.abs() < 1e-9 && ky.abs() < 1e-9 && sx > 0.0 && sy > 0.0 {
        Expr::normal(
            Symbol::system("Inset"),
            [
                stretched,
                point_expr([tx, ty]),
                origin,
                Expr::list([size[0] * sx, size[1] * sy]),
            ],
        )
    } else {
        let inset = Expr::normal(
            Symbol::system("Inset"),
            [
                stretched,
                point_expr([0.0, 0.0]),
                origin,
                Expr::list([size[0], size[1]]),
            ],
        );
        // Column-major `[sx, ky, kx, sy]` corresponds to WL 2×2 row-major matrix
        // `{{sx, kx}, {ky, sy}}` with translation vector `{tx, ty}`.
        let matrix = Expr::list([Expr::list([sx, kx]), Expr::list([ky, sy])]);
        Expr::normal(
            Symbol::system("GeometricTransformation"),
            [inset, Expr::list([matrix, point_expr([tx, ty])])],
        )
    }
}

fn clip_to_expr(bounds: [[f64; 2]; 2], children: &[VectorPrimitive]) -> Option<Expr> {
    let [[x0, x1], [y0, y1]] = bounds;
    let cw = (x1 - x0).max(1e-6);
    let ch = (y1 - y0).max(1e-6);

    let mut elements = Vec::with_capacity(children.len());
    for child in children {
        // Strokes inside clipped subgraphics normalize relative to `cw` so
        // `Thickness[stroke_pt / cw]` preserves exact typographic point widths.
        if let Some(expr) = primitive_to_expr(child, cw) {
            elements.push(expr);
        }
    }
    if elements.is_empty() {
        return None;
    }

    let inner = Expr::normal(
        Symbol::system("Graphics"),
        [
            Expr::list(elements),
            rule_sym_expr(
                "PlotRange",
                Expr::list([Expr::list([x0, x1]), Expr::list([y0, y1])]),
            ),
            rule_sym_sym("PlotRangePadding", "None"),
            rule_sym_sym("ImagePadding", "None"),
            rule_sym_expr("PlotRangeClipping", Expr::boolean(true)),
            rule_sym_sym("AspectRatio", "Full"),
        ],
    );
    let origin = Expr::list([
        Expr::symbol(Symbol::system("Left")),
        Expr::symbol(Symbol::system("Bottom")),
    ]);
    Some(Expr::normal(
        Symbol::system("Inset"),
        [inner, point_expr([x0, y0]), origin, Expr::list([cw, ch])],
    ))
}

fn build_curve_exprs(subpaths: &[Subpath], need_joined: bool) -> (bool, Expr, Option<Expr>) {
    let all_closed = subpaths.iter().all(|s| s.closed);
    let (all_segs, all_pts, closed_flags) = encode_subpaths(subpaths);
    let segs_expr = Expr::list(all_segs);
    let pts_expr = Expr::list(all_pts);
    let filled = Expr::normal(
        Symbol::system("FilledCurve"),
        [segs_expr.clone(), pts_expr.clone()],
    );
    let joined = need_joined.then(|| joined_curve_from_encoded(segs_expr, pts_expr, closed_flags));
    (all_closed, filled, joined)
}

fn joined_curve_from_encoded(segs_expr: Expr, pts_expr: Expr, closed_flags: Vec<i64>) -> Expr {
    Expr::normal(
        Symbol::system("JoinedCurve"),
        [
            segs_expr,
            pts_expr,
            rule_sym_expr("CurveClosed", Expr::list(closed_flags)),
        ],
    )
}

fn cached_glyph_expr(
    instance: &GlyphInstances,
    need_joined: bool,
) -> (bool, Expr, Option<Expr>) {
    let gen_now = cache_generation();

    if let Ok(guard) = GLYPH_EXPR_CACHE.read() {
        if guard.generation == gen_now {
            if let Some(cached) = guard.entries.get(&instance.key) {
                if !need_joined || cached.joined.is_some() {
                    return (
                        cached.all_closed,
                        cached.filled.clone(),
                        cached.joined.clone(),
                    );
                }
            }
        }
    }

    let (all_closed, filled, joined) = build_curve_exprs(&instance.subpaths, need_joined);

    if let Ok(mut guard) = GLYPH_EXPR_CACHE.write() {
        if guard.generation != gen_now {
            guard.entries.clear();
            guard.generation = gen_now;
        } else if guard.entries.len() >= MAX_EXPR_CACHE_ENTRIES
            && !guard.entries.contains_key(&instance.key)
        {
            guard.entries.clear();
        }

        if let Some(existing) = guard.entries.get_mut(&instance.key) {
            if need_joined && existing.joined.is_none() {
                existing.joined = joined.clone();
            }
            return (
                existing.all_closed,
                existing.filled.clone(),
                existing.joined.clone(),
            );
        }

        guard.entries.insert(
            instance.key,
            CachedGlyphExpr {
                all_closed,
                filled: filled.clone(),
                joined: joined.clone(),
            },
        );
    }

    (all_closed, filled, joined)
}

fn instanced_glyph_expr(curve_expr: Expr, positions: &[[f64; 2]]) -> Expr {
    // Wolfram's `GeometricTransformation[prim, {{x0, y0}, {x1, y1}}]` parses a
    // 2×2 numeric list as an affine matrix instead of two translations, so wrap
    // length-2 translation lists as `{{{x0, y0}}, {{x1, y1}}}`.
    let translations = match positions {
        [single] => point_expr(*single),
        [p0, p1] => Expr::list([
            Expr::list([point_expr(*p0)]),
            Expr::list([point_expr(*p1)]),
        ]),
        many => Expr::list(many.iter().copied().map(point_expr)),
    };
    Expr::normal(
        Symbol::system("GeometricTransformation"),
        [curve_expr, translations],
    )
}

/// Shared `(fill, stroke, all_closed)` primitive styling assembler used by both
/// [`glyphs_to_expr`] and [`paths_to_expr`].
fn styled_group<I>(
    items: &[I],
    fill: Option<Rgba>,
    stroke: Option<&StrokeStyle>,
    all_closed: bool,
    plot_width: f64,
    mut make_filled: impl FnMut(&I) -> Expr,
    mut make_joined: impl FnMut(&I) -> Expr,
) -> Option<Expr> {
    match (fill, stroke) {
        (Some(fill_color), None) => {
            let mut group = Vec::with_capacity(items.len() + 2);
            group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::symbol(Symbol::NONE)],
            ));
            group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            for item in items {
                group.push(make_filled(item));
            }
            Some(Expr::list(group))
        }
        (Some(fill_color), Some(stroke_style)) if all_closed => {
            let mut group = Vec::with_capacity(items.len() + 2);
            group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::list(stroke_directives(stroke_style, plot_width))],
            ));
            for item in items {
                group.push(make_filled(item));
            }
            Some(Expr::list(group))
        }
        (Some(fill_color), Some(stroke_style)) => {
            let mut fill_group = Vec::with_capacity(items.len() + 2);
            fill_group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::symbol(Symbol::NONE)],
            ));
            fill_group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            let mut stroke_group = stroke_directives(stroke_style, plot_width);
            for item in items {
                fill_group.push(make_filled(item));
                stroke_group.push(make_joined(item));
            }
            Some(Expr::list([Expr::list(fill_group), Expr::list(stroke_group)]))
        }
        (None, Some(stroke_style)) => {
            let mut group = stroke_directives(stroke_style, plot_width);
            for item in items {
                group.push(make_joined(item));
            }
            Some(Expr::list(group))
        }
        (None, None) => None,
    }
}

fn glyphs_to_expr(
    glyphs: &[GlyphInstances],
    fill: Option<Rgba>,
    stroke: Option<&StrokeStyle>,
    plot_width: f64,
) -> Option<Expr> {
    let valid_inputs: Vec<&GlyphInstances> = glyphs
        .iter()
        .filter(|g| !g.positions.is_empty() && g.subpaths.iter().any(|s| !s.segments.is_empty()))
        .collect();
    if valid_inputs.is_empty() {
        return None;
    }

    let prelim: Vec<(&GlyphInstances, bool, Expr, Option<Expr>)> = valid_inputs
        .into_iter()
        .map(|g| {
            let (all_closed, filled, joined) = cached_glyph_expr(g, fill.is_none());
            (g, all_closed, filled, joined)
        })
        .collect();
    let all_closed = prelim.iter().all(|(_, c, _, _)| *c);
    let need_joined = stroke.is_some() && (fill.is_none() || !all_closed);

    let prepared: Vec<(&GlyphInstances, Expr, Option<Expr>)> = prelim
        .into_iter()
        .map(|(g, _, filled, joined)| {
            let joined = if need_joined && joined.is_none() {
                cached_glyph_expr(g, true).2
            } else {
                joined
            };
            (g, filled, joined)
        })
        .collect();

    styled_group(
        &prepared,
        fill,
        stroke,
        all_closed,
        plot_width,
        |(g, filled, _)| instanced_glyph_expr(filled.clone(), &g.positions),
        |(g, _, joined)| {
            instanced_glyph_expr(joined.clone().expect("joined curve built"), &g.positions)
        },
    )
}

fn paths_to_expr(
    paths: &[Vec<Subpath>],
    fill: Option<Rgba>,
    stroke: Option<&StrokeStyle>,
    plot_width: f64,
) -> Option<Expr> {
    let valid_paths: Vec<&[Subpath]> = paths
        .iter()
        .map(Vec::as_slice)
        .filter(|sp| sp.iter().any(|s| !s.segments.is_empty()))
        .collect();
    if valid_paths.is_empty() {
        return None;
    }

    let all_closed = valid_paths.iter().all(|sp| sp.iter().all(|s| s.closed));
    styled_group(
        &valid_paths,
        fill,
        stroke,
        all_closed,
        plot_width,
        |sp| build_curve_exprs(sp, false).1,
        |sp| {
            let (all_segs, all_pts, closed_flags) = encode_subpaths(sp);
            joined_curve_from_encoded(Expr::list(all_segs), Expr::list(all_pts), closed_flags)
        },
    )
}

fn encode_subpaths(subpaths: &[Subpath]) -> (Vec<Expr>, Vec<Expr>, Vec<i64>) {
    let mut all_segs = Vec::with_capacity(subpaths.len());
    let mut all_pts = Vec::with_capacity(subpaths.len());
    let mut closed_flags = Vec::with_capacity(subpaths.len());

    for sp in subpaths {
        if sp.segments.is_empty() {
            continue;
        }
        let mut seg_descs = Vec::with_capacity(sp.segments.len());
        let mut pts = Vec::with_capacity(sp.segments.len() * 3 + 1);
        pts.push(point_expr(sp.start));

        for (idx, seg) in sp.segments.iter().enumerate() {
            let is_first = idx == 0;
            match *seg {
                Segment::Line(p) => {
                    let count = if is_first { 2_i64 } else { 1_i64 };
                    seg_descs.push(Expr::list([0_i64, count, 0_i64]));
                    pts.push(point_expr(p));
                }
                Segment::Cubic(c1, c2, p) => {
                    let count = if is_first { 4_i64 } else { 3_i64 };
                    seg_descs.push(Expr::list([1_i64, count, 3_i64]));
                    pts.push(point_expr(c1));
                    pts.push(point_expr(c2));
                    pts.push(point_expr(p));
                }
            }
        }

        all_segs.push(Expr::list(seg_descs));
        all_pts.push(Expr::list(pts));
        closed_flags.push(if sp.closed { 1_i64 } else { 0_i64 });
    }

    (all_segs, all_pts, closed_flags)
}

fn point_expr(p: [f64; 2]) -> Expr {
    Expr::list([p[0], p[1]])
}

fn rgba_expr(c: Rgba) -> Expr {
    if (c.a - 1.0).abs() <= 1e-6 {
        Expr::normal(&Symbol::RGB_COLOR, [c.r, c.g, c.b])
    } else {
        Expr::normal(&Symbol::RGB_COLOR, [c.r, c.g, c.b, c.a])
    }
}

fn stroke_directives(s: &StrokeStyle, plot_width: f64) -> Vec<Expr> {
    let mut out = Vec::with_capacity(5);
    out.push(rgba_expr(s.color));
    out.push(Expr::normal(
        Symbol::system("Thickness"),
        [s.thickness / plot_width],
    ));
    let cap_name = match s.cap {
        LineCapStyle::Butt => "Butt",
        LineCapStyle::Round => "Round",
        LineCapStyle::Square => "Square",
    };
    out.push(Expr::normal(Symbol::system("CapForm"), [cap_name]));

    let join_expr = match s.join {
        LineJoinStyle::Miter => Expr::normal(
            Symbol::system("JoinForm"),
            [Expr::list([Expr::from("Miter"), Expr::from(s.miter_limit)])],
        ),
        LineJoinStyle::Round => Expr::normal(Symbol::system("JoinForm"), ["Round"]),
        LineJoinStyle::Bevel => Expr::normal(Symbol::system("JoinForm"), ["Bevel"]),
    };
    out.push(join_expr);

    if let Some((ref pattern, _phase)) = s.dash {
        if !pattern.is_empty() {
            out.push(Expr::normal(
                Symbol::system("Dashing"),
                [Expr::list(pattern.iter().map(|len| *len / plot_width))],
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use matheme_world::graphics::{clear_glyph_cache, UnitGlyphKey, WindingRule};
    use typst::foundations::Bytes;

    #[test]
    fn encodes_filled_and_joined_curves_with_exact_options() {
        let subpath = Subpath {
            start: [0.0, 0.0],
            segments: vec![
                Segment::Line([10.0, 0.0]),
                Segment::Cubic([10.0, 5.0], [5.0, 10.0], [0.0, 10.0]),
                Segment::Line([0.0, 0.0]),
            ],
            closed: true,
        };
        let page = VectorPage {
            size: [10.5, 10.25],
            bounds: [[0.0, 10.5], [0.0, 10.25]],
            primitives: vec![VectorPrimitive::Paths {
                paths: vec![vec![subpath]],
                fill: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }),
                stroke: None,
                winding: WindingRule::NonZero,
            }],
        };

        let expr = page_to_expr(&page);
        let normal = expr.as_normal().expect("Graphics[...]");
        assert!(normal.has_head(&Symbol::system("Graphics")));
        let rendered = expr.to_string();
        assert!(rendered.contains("System`FilledCurve"), "{rendered}");
        assert!(rendered.contains("System`PlotRange"), "{rendered}");
        assert!(rendered.contains("System`PlotRangePadding"), "{rendered}");
        assert!(rendered.contains("System`ImagePadding"), "{rendered}");
        // Fractional ImageSize preserved (no .ceil()).
        assert!(rendered.contains("10.5, 10.25"), "{rendered}");
    }

    #[test]
    fn encodes_clip_and_rotated_image_primitives() {
        let clipped_child = VectorPrimitive::Paths {
            paths: vec![vec![Subpath {
                start: [0.0, 0.0],
                segments: vec![
                    Segment::Line([40.0, 0.0]),
                    Segment::Line([40.0, 40.0]),
                    Segment::Line([0.0, 40.0]),
                    Segment::Line([0.0, 0.0]),
                ],
                closed: true,
            }]],
            fill: Some(Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }),
            stroke: None,
            winding: WindingRule::EvenOdd,
        };
        let clip_prim = VectorPrimitive::Clip {
            bounds: [[10.0, 30.0], [10.0, 25.0]],
            children: vec![clipped_child],
        };
        let img_prim = VectorPrimitive::Image {
            format: EmbeddedImageFormat::Png,
            data: Bytes::new([0x89, b'P', b'N', b'G']),
            size: [20.0, 10.0],
            transform: [0.0, 1.0, -1.0, 0.0, 35.0, 5.0],
        };
        let page = VectorPage {
            size: [40.0, 40.0],
            bounds: [[0.0, 40.0], [0.0, 40.0]],
            primitives: vec![clip_prim, img_prim],
        };
        let rendered = page_to_expr(&page).to_string();
        assert!(rendered.contains("System`Inset"), "{rendered}");
        assert!(rendered.contains("System`Left, System`Bottom"), "{rendered}");
        assert!(rendered.contains("System`PlotRangeClipping"), "{rendered}");
        assert!(rendered.contains("System`True"), "{rendered}");
        assert!(rendered.contains("System`ImportByteArray"), "{rendered}");
        assert!(rendered.contains("\"PNG\""), "{rendered}");
        assert!(rendered.contains("System`GeometricTransformation"), "{rendered}");
    }

    #[test]
    fn glyph_expr_cache_invalidates_on_world_cache_generation() {
        let key = GlyphKey {
            unit: UnitGlyphKey {
                font_gen: 1,
                data_ptr: 0x1234,
                data_len: 100,
                face_index: 0,
                variations_hash: 0,
                glyph_id: 42,
            },
            linear_q: [10_000_000, 0, 0, 10_000_000],
        };
        let inst = GlyphInstances {
            key,
            subpaths: std::sync::Arc::new(vec![Subpath {
                start: [0.0, 0.0],
                segments: vec![
                    Segment::Line([5.0, 0.0]),
                    Segment::Line([5.0, 5.0]),
                    Segment::Line([0.0, 0.0]),
                ],
                closed: true,
            }]),
            positions: vec![[0.0, 0.0]],
        };

        let _ = cached_glyph_expr(&inst, false);
        assert!(GLYPH_EXPR_CACHE.read().unwrap().entries.contains_key(&key));
        clear_glyph_cache();
        // After world cache clear, next lookup clears old generation entries and repopulates only current key.
        let (_, _, joined) = cached_glyph_expr(&inst, true);
        assert!(joined.is_some());
        assert_eq!(
            GLYPH_EXPR_CACHE.read().unwrap().generation,
            cache_generation()
        );
    }
}
