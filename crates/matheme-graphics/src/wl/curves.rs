//! Curve, glyph-instance, color, and stroke directive encoding for Wolfram `Graphics`.

use std::sync::{LazyLock, Mutex};

use matheme_wl::{Expr, fundamental, graphics, list, normal};

use super::WlEncoder;
use crate::extract::{GLYPH_CACHE_CAPACITY, cache_generation};
use crate::ir::{
    GlyphInstances, GlyphKey, LineCapStyle, LineJoinStyle, Rgba, Segment, StrokeStyle, Subpath,
};
use crate::lru::{LruCache, lock_unpoisoned};

#[derive(Clone)]
pub(super) struct CachedGlyphExpr {
    filled: Expr,
    joined: Option<Expr>,
}

pub(super) struct GlyphExprCache {
    pub(super) generation: u64,
    pub(super) entries: LruCache<GlyphKey, CachedGlyphExpr>,
}

impl GlyphExprCache {
    fn new() -> Self {
        Self {
            generation: 0,
            entries: LruCache::new(GLYPH_CACHE_CAPACITY),
        }
    }
}

pub(super) static GLYPH_EXPR_CACHE: LazyLock<Mutex<GlyphExprCache>> =
    LazyLock::new(|| Mutex::new(GlyphExprCache::new()));

pub(crate) fn clear_expr_cache() {
    let mut guard = lock_unpoisoned(&GLYPH_EXPR_CACHE);
    guard.entries.clear();
    guard.generation = cache_generation();
}

impl WlEncoder {
    pub(super) fn build_curve_exprs(
        &self,
        subpaths: &[Subpath],
        need_joined: bool,
    ) -> (Expr, Option<Expr>) {
        let (all_segs, all_pts, closed_flags) = self.encode_subpaths(subpaths);
        let segs_expr = Expr::list(all_segs);
        let pts_expr = Expr::list(all_pts);
        let filled = normal!(&graphics::FILLED_CURVE, segs_expr.clone(), pts_expr.clone());
        let joined =
            need_joined.then(|| joined_curve_from_encoded(segs_expr, pts_expr, closed_flags));
        (filled, joined)
    }

    pub(super) fn build_joined_curve_expr(&self, subpaths: &[Subpath]) -> Expr {
        let (all_segs, all_pts, closed_flags) = self.encode_subpaths(subpaths);
        joined_curve_from_encoded(Expr::list(all_segs), Expr::list(all_pts), closed_flags)
    }

    pub(super) fn cached_glyph_expr(
        &self,
        instance: &GlyphInstances,
        need_joined: bool,
    ) -> (Expr, Option<Expr>) {
        let gen_now = cache_generation();
        let mut existing_filled: Option<Expr> = None;

        {
            let mut guard = lock_unpoisoned(&GLYPH_EXPR_CACHE);
            if guard.generation != gen_now {
                guard.entries.clear();
                guard.generation = gen_now;
            } else if let Some(cached) = guard.entries.get(&instance.key) {
                if !need_joined || cached.joined.is_some() {
                    return (cached.filled.clone(), cached.joined.clone());
                }
                existing_filled = Some(cached.filled.clone());
            }
        }

        let (filled, joined) = match existing_filled {
            Some(filled) => (
                filled,
                Some(self.build_joined_curve_expr(&instance.subpaths)),
            ),
            None => self.build_curve_exprs(&instance.subpaths, need_joined),
        };

        let mut guard = lock_unpoisoned(&GLYPH_EXPR_CACHE);
        if guard.generation != gen_now {
            guard.entries.clear();
            guard.generation = gen_now;
        }

        if let Some(existing) = guard.entries.get_mut(&instance.key) {
            if need_joined && existing.joined.is_none() {
                existing.joined = joined.clone();
            }
            return (existing.filled.clone(), existing.joined.clone());
        }

        guard.entries.insert(
            instance.key,
            CachedGlyphExpr {
                filled: filled.clone(),
                joined: joined.clone(),
            },
        );

        (filled, joined)
    }

    pub(super) fn glyphs_to_expr(
        &self,
        glyphs: &[GlyphInstances],
        fill: Option<Rgba>,
        stroke: Option<&StrokeStyle>,
        plot_width: f64,
    ) -> Option<Expr> {
        let valid_inputs: Vec<&GlyphInstances> = glyphs
            .iter()
            .filter(|g| {
                !g.positions.is_empty() && g.subpaths.iter().any(|s| !s.segments.is_empty())
            })
            .collect();
        if valid_inputs.is_empty() {
            return None;
        }

        let all_closed = valid_inputs
            .iter()
            .all(|g| g.subpaths.iter().all(|s| s.closed));
        let need_joined = stroke.is_some() && (fill.is_none() || !all_closed);

        let prepared: Vec<(&GlyphInstances, Expr, Option<Expr>)> = valid_inputs
            .into_iter()
            .map(|g| {
                let (filled, joined) = self.cached_glyph_expr(g, need_joined);
                (g, filled, joined)
            })
            .collect();

        self.styled_group(
            &prepared,
            fill,
            stroke,
            all_closed,
            plot_width,
            |(g, filled, _)| instanced_expr(filled.clone(), &g.positions),
            |(g, _, joined)| {
                instanced_expr(joined.clone().expect("joined curve built"), &g.positions)
            },
        )
    }

    fn encode_subpaths(&self, subpaths: &[Subpath]) -> (Vec<Expr>, Vec<Expr>, Vec<i64>) {
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
                        seg_descs.push(if is_first {
                            self.seg_line_first.clone()
                        } else {
                            self.seg_line_cont.clone()
                        });
                        pts.push(point_expr(p));
                    }
                    Segment::Cubic(c1, c2, p) => {
                        seg_descs.push(if is_first {
                            self.seg_cubic_first.clone()
                        } else {
                            self.seg_cubic_cont.clone()
                        });
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
}

fn joined_curve_from_encoded(segs_expr: Expr, pts_expr: Expr, closed_flags: Vec<i64>) -> Expr {
    normal!(
        &graphics::JOINED_CURVE,
        segs_expr,
        pts_expr,
        &graphics::CURVE_CLOSED => Expr::list(closed_flags),
    )
}

pub(super) fn instanced_expr(prim_expr: Expr, positions: &[[f64; 2]]) -> Expr {
    // Wolfram's `GeometricTransformation[prim, {{x0, y0}, {x1, y1}}]` parses a
    // 2×2 numeric list as an affine matrix instead of two translations, so wrap
    // length-2 translation lists as `{{{x0, y0}}, {{x1, y1}}}`.
    let translations = match positions {
        [single] => point_expr(*single),
        [p0, p1] => list![list![point_expr(*p0)], list![point_expr(*p1)]],
        many => Expr::list(many.iter().copied().map(point_expr)),
    };
    normal!(&graphics::GEOMETRIC_TRANSFORMATION, prim_expr, translations)
}

pub(super) fn point_expr(p: [f64; 2]) -> Expr {
    list![p[0], p[1]]
}

pub(super) fn rgba_expr(c: Rgba) -> Expr {
    if (c.a - 1.0).abs() <= 1e-6 {
        normal!(&fundamental::RGB_COLOR, c.r, c.g, c.b)
    } else {
        normal!(&fundamental::RGB_COLOR, c.r, c.g, c.b, c.a)
    }
}

pub(super) fn stroke_directives(s: &StrokeStyle, plot_width: f64) -> Vec<Expr> {
    let mut out = Vec::with_capacity(5);
    out.push(rgba_expr(s.color));
    out.push(normal!(&graphics::THICKNESS, s.thickness / plot_width));
    let cap_name = match s.cap {
        LineCapStyle::Butt => "Butt",
        LineCapStyle::Round => "Round",
        LineCapStyle::Square => "Square",
    };
    out.push(normal!(&graphics::CAP_FORM, cap_name));

    let join_expr = match s.join {
        LineJoinStyle::Miter => normal!(&graphics::JOIN_FORM, list!["Miter", s.miter_limit]),
        LineJoinStyle::Round => normal!(&graphics::JOIN_FORM, "Round"),
        LineJoinStyle::Bevel => normal!(&graphics::JOIN_FORM, "Bevel"),
    };
    out.push(join_expr);

    if let Some((ref pattern, phase)) = s.dash {
        if !pattern.is_empty() {
            let lengths = Expr::list(pattern.iter().map(|len| *len / plot_width));
            if phase.abs() <= 1e-9 && s.cap == LineCapStyle::Butt {
                out.push(normal!(&graphics::DASHING, lengths));
            } else {
                out.push(normal!(
                    &graphics::DASHING,
                    lengths,
                    phase / plot_width,
                    cap_name,
                ));
            }
        }
    }
    out
}
