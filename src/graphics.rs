//! Conversion of [`matheme_world::graphics::VectorPage`] into native Wolfram
//! Language `System\`Graphics` [`Expr`]s.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use matheme_wl::{Expr, Symbol};
use matheme_world::graphics::{
    GlyphInstances, GlyphKey, LineCapStyle, LineJoinStyle, Rgba, Segment, StrokeStyle, Subpath,
    VectorPage, VectorPrimitive,
};

#[derive(Clone)]
struct CachedGlyphExpr {
    all_closed: bool,
    filled: Expr,
    joined: Expr,
}

static GLYPH_EXPR_CACHE: LazyLock<RwLock<HashMap<GlyphKey, CachedGlyphExpr>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn clear_glyph_expr_cache() {
    if let Ok(mut guard) = GLYPH_EXPR_CACHE.write() {
        guard.clear();
    }
}

pub fn page_to_expr(page: &VectorPage) -> Expr {
    let [[x0, x1], [y0, y1]] = page.plot_range;
    let plot_width = (x1 - x0).abs().max(1e-6);
    let img_w = (x1 - x0).ceil().max(1.0);
    let img_h = (y1 - y0).ceil().max(1.0);

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
            Expr::normal(
                Symbol::RULE,
                [
                    Expr::symbol(Symbol::system("ImageSize")),
                    Expr::list([img_w, img_h]),
                ],
            ),
            Expr::normal(
                Symbol::RULE,
                [
                    Expr::symbol(Symbol::system("PlotRange")),
                    Expr::list([Expr::list([x0, x1]), Expr::list([y0, y1])]),
                ],
            ),
            Expr::normal(
                Symbol::RULE,
                [
                    Expr::symbol(Symbol::system("AspectRatio")),
                    Expr::symbol(Symbol::AUTOMATIC),
                ],
            ),
        ],
    )
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
            data, min, max, ..
        } => {
            let imported = Expr::normal(
                Symbol::system("ImportByteArray"),
                [Expr::bytes(data.clone())],
            );
            let width = (max[0] - min[0]).max(1e-3);
            let height = (max[1] - min[1]).max(1e-3);
            Some(Expr::normal(
                Symbol::system("Inset"),
                [
                    imported,
                    point_expr(*min),
                    Expr::list([0.0, 0.0]),
                    Expr::list([width, height]),
                ],
            ))
        }
    }
}

fn cached_glyph_expr(instance: &GlyphInstances) -> CachedGlyphExpr {
    if let Ok(guard) = GLYPH_EXPR_CACHE.read() {
        if let Some(cached) = guard.get(&instance.key) {
            return cached.clone();
        }
    }
    let all_closed = instance.subpaths.iter().all(|s| s.closed);
    let (all_segs, all_pts, closed_flags) = encode_subpaths(&instance.subpaths);
    let segs_expr = Expr::list(all_segs);
    let pts_expr = Expr::list(all_pts);
    let filled = Expr::normal(
        Symbol::system("FilledCurve"),
        [segs_expr.clone(), pts_expr.clone()],
    );
    let joined = Expr::normal(
        Symbol::system("JoinedCurve"),
        [
            segs_expr,
            pts_expr,
            Expr::normal(
                Symbol::RULE,
                [
                    Expr::symbol(Symbol::system("CurveClosed")),
                    Expr::list(closed_flags),
                ],
            ),
        ],
    );
    let cached = CachedGlyphExpr {
        all_closed,
        filled,
        joined,
    };
    if let Ok(mut guard) = GLYPH_EXPR_CACHE.write() {
        guard.insert(instance.key.clone(), cached.clone());
    }
    cached
}

fn instanced_glyph_expr(curve_expr: Expr, positions: &[[f64; 2]]) -> Expr {
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

fn glyphs_to_expr(
    glyphs: &[GlyphInstances],
    fill: Option<Rgba>,
    stroke: Option<&StrokeStyle>,
    plot_width: f64,
) -> Option<Expr> {
    let valid: Vec<(&GlyphInstances, CachedGlyphExpr)> = glyphs
        .iter()
        .filter(|g| !g.positions.is_empty() && g.subpaths.iter().any(|s| !s.segments.is_empty()))
        .map(|g| (g, cached_glyph_expr(g)))
        .collect();
    if valid.is_empty() {
        return None;
    }

    let all_closed = valid.iter().all(|(_, c)| c.all_closed);

    match (fill, stroke) {
        (Some(fill_color), None) => {
            let mut group = Vec::with_capacity(valid.len() + 2);
            group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::symbol(Symbol::NONE)],
            ));
            group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            for (g, c) in valid {
                group.push(instanced_glyph_expr(c.filled, &g.positions));
            }
            Some(Expr::list(group))
        }
        (Some(fill_color), Some(stroke_style)) if all_closed => {
            let mut group = Vec::with_capacity(valid.len() + 2);
            group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::list(stroke_directives(stroke_style, plot_width))],
            ));
            for (g, c) in valid {
                group.push(instanced_glyph_expr(c.filled, &g.positions));
            }
            Some(Expr::list(group))
        }
        (Some(fill_color), Some(stroke_style)) => {
            let mut fill_group = Vec::with_capacity(valid.len() + 2);
            fill_group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::symbol(Symbol::NONE)],
            ));
            fill_group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            let mut stroke_group = stroke_directives(stroke_style, plot_width);
            for (g, c) in valid {
                fill_group.push(instanced_glyph_expr(c.filled, &g.positions));
                stroke_group.push(instanced_glyph_expr(c.joined, &g.positions));
            }
            Some(Expr::list([Expr::list(fill_group), Expr::list(stroke_group)]))
        }
        (None, Some(stroke_style)) => {
            let mut group = stroke_directives(stroke_style, plot_width);
            for (g, c) in valid {
                group.push(instanced_glyph_expr(c.joined, &g.positions));
            }
            Some(Expr::list(group))
        }
        (None, None) => None,
    }
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

    let all_closed = valid_paths
        .iter()
        .all(|sp| sp.iter().all(|s| s.closed));

    match (fill, stroke) {
        (Some(fill_color), None) => {
            let mut group = Vec::with_capacity(valid_paths.len() + 2);
            group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::symbol(Symbol::NONE)],
            ));
            group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            for subpaths in valid_paths {
                group.push(filled_curve_expr(subpaths));
            }
            Some(Expr::list(group))
        }
        (Some(fill_color), Some(stroke_style)) if all_closed => {
            let mut group = Vec::with_capacity(valid_paths.len() + 2);
            group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::list(stroke_directives(stroke_style, plot_width))],
            ));
            for subpaths in valid_paths {
                group.push(filled_curve_expr(subpaths));
            }
            Some(Expr::list(group))
        }
        (Some(fill_color), Some(stroke_style)) => {
            let mut fill_group = Vec::with_capacity(valid_paths.len() + 2);
            fill_group.push(Expr::normal(
                Symbol::system("EdgeForm"),
                [Expr::symbol(Symbol::NONE)],
            ));
            fill_group.push(Expr::normal(
                Symbol::system("FaceForm"),
                [rgba_expr(fill_color)],
            ));
            for subpaths in &valid_paths {
                fill_group.push(filled_curve_expr(subpaths));
            }

            let mut stroke_group = stroke_directives(stroke_style, plot_width);
            for subpaths in &valid_paths {
                stroke_group.push(joined_curve_expr(subpaths));
            }

            Some(Expr::list([Expr::list(fill_group), Expr::list(stroke_group)]))
        }
        (None, Some(stroke_style)) => {
            let mut group = stroke_directives(stroke_style, plot_width);
            for subpaths in valid_paths {
                group.push(joined_curve_expr(subpaths));
            }
            Some(Expr::list(group))
        }
        (None, None) => None,
    }
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

fn filled_curve_expr(subpaths: &[Subpath]) -> Expr {
    let (all_segs, all_pts, _) = encode_subpaths(subpaths);
    Expr::normal(
        Symbol::system("FilledCurve"),
        [Expr::list(all_segs), Expr::list(all_pts)],
    )
}

fn joined_curve_expr(subpaths: &[Subpath]) -> Expr {
    let (all_segs, all_pts, closed_flags) = encode_subpaths(subpaths);
    Expr::normal(
        Symbol::system("JoinedCurve"),
        [
            Expr::list(all_segs),
            Expr::list(all_pts),
            Expr::normal(
                Symbol::RULE,
                [
                    Expr::symbol(Symbol::system("CurveClosed")),
                    Expr::list(closed_flags),
                ],
            ),
        ],
    )
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
    use matheme_world::graphics::WindingRule;

    #[test]
    fn encodes_filled_and_joined_curves_into_graphics_expr() {
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
            width: 10.0,
            height: 10.0,
            background: None,
            plot_range: [[0.0, 10.0], [0.0, 10.0]],
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
    }
}
