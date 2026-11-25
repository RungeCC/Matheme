//! Criterion benchmarks for `matheme-world` compilation / evaluation,
//! `matheme-wstp` symbol resolution, and end-to-end compile -> vector -> WL
//! pipelines (PF-03, PF-05).

use std::path::PathBuf;
use std::time::Duration;

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use matheme_bench::{compile_doc, doc_academic_multipage, doc_compile_evaluate};
use matheme_graphics::{export_vector, pages_to_expr};
use matheme_world::compile::{self, CompileRequest, EvaluateSpec, OutputFormat, PdfStandards};
use matheme_world::{WorldDate, font_cache, font_generation};
use matheme_wstp::resolve_unqualified_symbol;
use typst::foundations::Dict;

fn bench_compile_evaluate(c: &mut Criterion) {
    let _ = font_cache().len();
    let src = doc_compile_evaluate();
    let exprs: Vec<String> = (0..25)
        .map(|i| format!("fib({}) + table_data.at({}).sq", 10 + (i % 15), i))
        .collect();

    let mut group = c.benchmark_group("compile_evaluate");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    group.bench_function("svg_6p_no_evaluate", |b| {
        b.iter(|| {
            let req = CompileRequest {
                source: src.clone(),
                inputs: Dict::new(),
                base_path: PathBuf::from("."),
                allow_downloads: false,
                format: OutputFormat::Svg,
                pages: None,
                pdf_standards: PdfStandards::default(),
                pdf_tagged: false,
                pdf_timestamp: None,
                world_date: WorldDate::None,
                features: Vec::new(),
                evaluate: None,
                extra_diagnostics: Vec::new(),
            };
            black_box(compile::compile(req).expect("compile without evaluate"))
        });
    });

    group.bench_function("svg_6p_with_25_evaluate_exprs", |b| {
        b.iter(|| {
            let req = CompileRequest {
                source: src.clone(),
                inputs: Dict::new(),
                base_path: PathBuf::from("."),
                allow_downloads: false,
                format: OutputFormat::Svg,
                pages: None,
                pdf_standards: PdfStandards::default(),
                pdf_tagged: false,
                pdf_timestamp: None,
                world_date: WorldDate::None,
                features: Vec::new(),
                evaluate: Some(EvaluateSpec::List(exprs.clone())),
                extra_diagnostics: Vec::new(),
            };
            black_box(compile::compile(req).expect("compile with evaluate"))
        });
    });

    group.finish();
}

fn bench_symbol_resolution(c: &mut Criterion) {
    let names = [
        "ReturnPacket",
        "FilledCurve",
        "GeometricTransformation",
        "PlotRangeClipping",
        "Disk",
        "List",
        "Rule",
        "$InstallationDirectory",
        "myUserSymbol",
        "fooBarBaz",
        "x",
        "Show",
        "ImportByteArray",
        "Thickness",
        "UnknownHelper",
        "JoinForm",
    ];

    let mut group = c.benchmark_group("wstp_symbol_resolve");
    group.warm_up_time(Duration::from_millis(400));
    group.measurement_time(Duration::from_secs(2));
    group.throughput(Throughput::Elements((names.len() * 10_000) as u64));

    group.bench_function("160k_mixed_system_and_global", |b| {
        b.iter(|| {
            let mut acc = 0usize;
            for _ in 0..10_000 {
                for name in names {
                    acc += resolve_unqualified_symbol(black_box(name)).as_str().len();
                }
            }
            black_box(acc)
        });
    });

    group.finish();
}

fn bench_e2e_pipeline(c: &mut Criterion) {
    let _ = font_cache().len();
    let fgen = font_generation();
    let src = doc_academic_multipage();

    let mut group = c.benchmark_group("e2e_pipeline");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    group.bench_function("4p_academic_compile_extract_wl", |b| {
        b.iter(|| {
            let doc = compile_doc(black_box(&src));
            let vec_out = export_vector(&doc, None, fgen);
            black_box(pages_to_expr(&vec_out.pages))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_compile_evaluate,
    bench_symbol_resolution,
    bench_e2e_pipeline
);
criterion_main!(benches);
