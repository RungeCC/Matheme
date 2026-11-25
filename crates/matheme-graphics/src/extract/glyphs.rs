//! Glyph outline extraction, generation-tracked bounded LRU caches, and
//! solid/painted text-run extraction.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use kurbo::{Affine, BezPath, Point, Rect};
use ttf_parser::{GlyphId, OutlineBuilder};
use typst::layout::{Abs, Ratio, Size, Transform};
use typst::text::color::{GlyphFrameItem, glyph_frame, should_outline};
use typst::text::{Font, FontInstance, TextItem};
use typst::visualize::Paint;

use super::{
    Extractor, ParentContext, convert_solid_stroke, convert_stroke_geometry, push_painted_stroke,
};
use crate::geometry::{
    BoundsTracker, affine_from_typst, affine_stroke_scale, bez_path_to_subpaths, strip_translation,
    stroke_bounding_box, stroke_hash, subpaths_to_bez_path, transform_subpaths_and_bbox,
    translate_subpaths_into,
};
use crate::ir::{
    GlyphInstances, GlyphKey, Rgba, StrokeStyle, Subpath, UnitGlyphKey, VectorPrimitive,
    WindingRule,
};
use crate::lru::{LruCache, lock_unpoisoned};
use crate::winding::{self, WindingResolution};

/// Shared upper bound across all glyph cache tiers (unit outlines, scaled
/// outlines, and host-encoded expressions) using O(1) LRU eviction.
pub(crate) const GLYPH_CACHE_CAPACITY: usize = 8192;

/// Scale factor used to quantize linear transform components (`1e-6` precision)
/// so floating-point noise never pollutes the glyph key space.
const LINEAR_QUANT_SCALE: f64 = 1_000_000.0;

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

/// Scaled glyph outline cached across exports in a bounded LRU.
#[derive(Debug, Clone)]
pub(super) struct ScaledGlyph {
    pub(super) subpaths: Arc<Vec<Subpath>>,
    pub(super) fill_bbox: Rect,
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

pub(super) fn sync_font_generation(font_gen: u64) {
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

fn outline_glyph_unit(font: &FontInstance, key: UnitGlyphKey) -> CachedUnitGlyph {
    {
        let mut guard = lock_unpoisoned(&UNIT_GLYPH_CACHE);
        if guard.font_gen == key.font_gen
            && let Some(cached) = guard.entries.get(&key)
        {
            return cached.clone();
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
        if font
            .ttf()
            .outline_glyph(GlyphId(key.glyph_id), &mut builder)
            .is_none()
        {
            return CachedUnitGlyph {
                subpaths: None,
                winding_degraded: false,
            };
        }
        let mut path = builder.0;
        path.apply_affine(Affine::scale(1.0 / upem));
        let raw_subpaths = bez_path_to_subpaths(&path);
        if raw_subpaths.is_empty() {
            return CachedUnitGlyph {
                subpaths: None,
                winding_degraded: false,
            };
        }
        let (final_subpaths, winding_degraded) =
            match winding::resolve_nonzero_subpaths(&raw_subpaths) {
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

impl Extractor {
    fn font_run_key(&mut self, font: &FontInstance) -> FontRunKey {
        let base = font.font();
        let ptr_key = (
            base.data().as_ptr() as usize,
            base.data().len(),
            base.index(),
        );
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

    fn extract_glyph_frame(
        &mut self,
        text: &TextItem,
        glyph_id: u16,
        origin_ts: Transform,
        parent: ParentContext,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
        let Some(gframe) = glyph_frame(&text.font, glyph_id) else {
            return;
        };
        let scale = Ratio::new(text.size.to_pt() / text.font.units_per_em());
        let item_pos = gframe.item.pos();
        let glyph_ts = origin_ts
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

    pub(super) fn extract_text(
        &mut self,
        text: &TextItem,
        ts: Transform,
        parent: ParentContext,
        bbox: &mut BoundsTracker,
        primitives: &mut Vec<VectorPrimitive>,
    ) {
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
        let linear_affine = strip_translation(affine_from_typst(text_ts.pre_concat(scale_ts)));
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
                    let key = GlyphKey::from_quantized(run_key.unit_key(id.0), linear_q);
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
                } else {
                    let origin_ts = text_ts.pre_concat(Transform::translate(x_offset, y_offset));
                    self.extract_glyph_frame(text, id.0, origin_ts, parent, bbox, primitives);
                }

                x += glyph.x_advance.at(text.size);
                y += glyph.y_advance.at(text.size);
            }
            return;
        }

        // Gradient- or tiling-painted text: materialize the run's glyph outlines
        // as translated subpaths so native gradient/texture fills span the whole run.
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
                let key = GlyphKey::from_quantized(run_key.unit_key(id.0), linear_q);
                if let Some(scaled) = self.scaled_glyph(&text.font, key, linear_affine) {
                    let pos = text_affine * Point::new(x_offset.to_pt(), y_offset.to_pt());
                    let glyph_bbox = self.glyph_origin_bbox(key, &scaled, geom_stroke.as_ref());
                    let shifted = glyph_bbox + pos.to_vec2();
                    bbox.include_rect(shifted);
                    run_bbox.include_rect(shifted);
                    translate_subpaths_into(&scaled.subpaths, [pos.x, pos.y], &mut run_subpaths);
                }
            } else {
                let origin_ts = text_ts.pre_concat(Transform::translate(x_offset, y_offset));
                self.extract_glyph_frame(text, id.0, origin_ts, parent, bbox, primitives);
            }

            x += glyph.x_advance.at(text.size);
            y += glyph.y_advance.at(text.size);
        }

        if run_subpaths.is_empty() {
            return;
        }

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
}
