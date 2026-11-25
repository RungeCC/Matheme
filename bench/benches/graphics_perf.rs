//! Criterion benchmarks for `matheme-graphics` core performance hotspots:
//! - PF-01: Non-zero -> even-odd planar winding resolution (fast path, slow path, dense stress)
//! - PF-02: Two-level LRU glyph caches under >4900-key churn + hot working set
//! - PF-04: Zero-copy winding fast path and analytical/cached stroke bounding boxes
//! - PF-05: Warm repeated exports (cross-call LRU scaled glyph cache + hoisted run hashing)

use std::time::Duration;

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use matheme_bench::{
    compile_doc, doc_academic_multipage, doc_glyph_cache_churn, doc_stroked_shapes,
    doc_winding_dense_self_intersecting, doc_winding_fast_curves, doc_winding_simple_polys,
    doc_winding_slow_stars,
};
use matheme_graphics::{clear_caches, export_vector, pages_to_expr};
use matheme_world::{font_cache, font_generation};

fn bench_winding(c: &mut Criterion) {
    let _ = font_cache().len();
    let fgen = font_generation();

    let mut group = c.benchmark_group("winding");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));

    let fast_curves = compile_doc(&doc_winding_fast_curves());
    group.throughput(Throughput::Elements(200));
    group.bench_function("fast_path_200_cubic_curves", |b| {
        b.iter(|| black_box(export_vector(black_box(&fast_curves), None, fgen)));
    });

    let simple_polys = compile_doc(&doc_winding_simple_polys());
    group.throughput(Throughput::Elements(500));
    group.bench_function("fast_path_500_simple_polys", |b| {
        b.iter(|| black_box(export_vector(black_box(&simple_polys), None, fgen)));
    });

    let slow_stars = compile_doc(&doc_winding_slow_stars(16, 19));
    group.throughput(Throughput::Elements(16));
    group.bench_function("slow_path_16x19_stars", |b| {
        b.iter(|| black_box(export_vector(black_box(&slow_stars), None, fgen)));
    });

    for &verts in &[250usize, 1500usize] {
        let dense = compile_doc(&doc_winding_dense_self_intersecting(verts));
        group.throughput(Throughput::Elements(verts as u64));
        group.bench_with_input(
            BenchmarkId::new("dense_self_intersecting", verts),
            &dense,
            |b, doc| {
                b.iter(|| black_box(export_vector(black_box(doc), None, fgen)));
            },
        );
    }

    group.finish();
}

fn bench_stroked_shapes(c: &mut Criterion) {
    let _ = font_cache().len();
    let fgen = font_generation();
    let doc = compile_doc(&doc_stroked_shapes());

    let mut group = c.benchmark_group("stroke_bbox");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));
    group.throughput(Throughput::Elements(600));
    group.bench_function("600_lines_rects_circles", |b| {
        b.iter(|| black_box(export_vector(black_box(&doc), None, fgen)));
    });
    group.finish();
}

fn bench_glyph_caches(c: &mut Criterion) {
    let _ = font_cache().len();
    let fgen = font_generation();

    let mut group = c.benchmark_group("glyph_cache");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    // >4,900 unique GlyphKeys (> old 4096 Expr cache clear() cliff) + hot working set.
    let churn_5k = compile_doc(&doc_glyph_cache_churn(80));
    clear_caches();
    let _ = pages_to_expr(&export_vector(&churn_5k, None, fgen).pages);
    group.bench_function("churn_5k_unique_keys_extract_and_wl", |b| {
        b.iter(|| {
            let out = export_vector(black_box(&churn_5k), None, fgen);
            black_box(pages_to_expr(&out.pages))
        });
    });

    // >9,000 unique GlyphKeys (> 8192 LRU capacity) with hot working set retained by LRU.
    let churn_9k = compile_doc(&doc_glyph_cache_churn(150));
    clear_caches();
    let _ = pages_to_expr(&export_vector(&churn_9k, None, fgen).pages);
    group.bench_function("churn_9k_over_capacity_lru_steady", |b| {
        b.iter(|| {
            let out = export_vector(black_box(&churn_9k), None, fgen);
            black_box(pages_to_expr(&out.pages))
        });
    });

    // Repeated warm export of a 4-page academic math+prose document (tests cross-call L1 scaled cache).
    let academic = compile_doc(&doc_academic_multipage());
    clear_caches();
    let _ = pages_to_expr(&export_vector(&academic, None, fgen).pages);
    group.bench_function("warm_repeated_export_4p_academic", |b| {
        b.iter(|| {
            let out = export_vector(black_box(&academic), None, fgen);
            black_box(pages_to_expr(&out.pages))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_winding,
    bench_stroked_shapes,
    bench_glyph_caches
);
criterion_main!(benches);
