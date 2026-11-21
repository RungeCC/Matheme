//! Automated performance & complexity regression guard tests.
//!
//! Run with: `cargo test -p matheme-bench`

use std::path::PathBuf;
use std::time::Instant;

use matheme_bench::{
    compile_doc, doc_academic_multipage, doc_glyph_cache_churn, doc_stroked_shapes,
    doc_winding_dense_self_intersecting, doc_winding_slow_stars,
};
use matheme_graphics::{
    cache_generation, clear_caches, export_vector, pages_to_expr, DegradationWarning,
};
use matheme_world::compile::{
    self, parse_page_str, CompileOutput, CompileRequest, EvaluateSpec, OutputFormat, PdfStandards,
};
use matheme_world::{font_generation, WorldDate};
use matheme_wstp::resolve_unqualified_symbol;
use typst::foundations::Dict;

#[test]
fn winding_budget_prevents_quadratic_stall_and_emits_warning() {
    let doc = compile_doc(&doc_winding_dense_self_intersecting(2500));
    let fgen = font_generation();

    let start = Instant::now();
    let out = export_vector(&doc, None, fgen);
    let elapsed = start.elapsed();

    assert!(
        out.warnings
            .contains(&DegradationWarning::NonZeroWindingApproximated),
        "2500-vertex self-intersecting path must trigger NonZeroWindingApproximated warning"
    );
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "budget-capped winding resolution took too long: {elapsed:?}"
    );
}

#[test]
fn winding_slow_stars_resolve_cleanly_without_warning() {
    let doc = compile_doc(&doc_winding_slow_stars(8, 13));
    let fgen = font_generation();

    let out = export_vector(&doc, None, fgen);
    assert!(
        !out.warnings
            .contains(&DegradationWarning::NonZeroWindingApproximated),
        "ordinary 13-point stars must resolve within budget without degradation: {:?}",
        out.warnings
    );
    assert_eq!(out.pages.len(), 1);
    assert_eq!(out.pages[0].primitives.len(), 8);
}

#[test]
fn lru_cache_handles_5k_glyph_churn_without_generation_reset_and_accelerates_warm_calls() {
    let doc = compile_doc(&doc_glyph_cache_churn(80));
    let fgen = font_generation();
    clear_caches();
    let gen_before = cache_generation();

    let cold_start = Instant::now();
    let out1 = export_vector(&doc, None, fgen);
    let _expr1 = pages_to_expr(&out1.pages);
    let cold = cold_start.elapsed();

    let warm_start = Instant::now();
    let out2 = export_vector(&doc, None, fgen);
    let _expr2 = pages_to_expr(&out2.pages);
    let warm = warm_start.elapsed();

    assert_eq!(
        cache_generation(),
        gen_before,
        "steady-state LRU eviction must not advance cache_generation"
    );
    assert!(
        warm < cold,
        "warm LRU-cached 5k-glyph export ({warm:?}) should be faster than cold ({cold:?})"
    );
}

#[test]
fn stroked_shapes_export_produces_finite_bounds_quickly() {
    let doc = compile_doc(&doc_stroked_shapes());
    let fgen = font_generation();

    let out = export_vector(&doc, None, fgen);
    assert_eq!(out.pages.len(), 1);
    assert_eq!(out.pages[0].primitives.len(), 600);
    let [[x0, x1], [y0, y1]] = out.pages[0].bounds;
    assert!(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite());
    assert!(x1 > x0 && y1 > y0);
}

#[test]
fn pages_filtering_consistent_across_vector_svg_and_pdf() {
    let src = doc_academic_multipage();
    let pages = parse_page_str("2,4").unwrap();

    let doc = compile_doc(&src);
    let vec_out = export_vector(&doc, Some(&pages), font_generation());
    assert_eq!(vec_out.pages.len(), 2);

    let pdf_ok = compile::compile(CompileRequest {
        source: src.clone(),
        inputs: Dict::new(),
        base_path: PathBuf::from("."),
        allow_downloads: false,
        format: OutputFormat::Pdf,
        pages: Some(pages.clone()),
        pdf_standards: PdfStandards::default(),
        pdf_tagged: false,
        pdf_timestamp: None,
        world_date: WorldDate::None,
        features: Vec::new(),
        evaluate: Some(EvaluateSpec::Single("1 + 2".into())),
        extra_diagnostics: Vec::new(),
    })
    .expect("filtered PDF compile");

    match pdf_ok.output {
        CompileOutput::Pdf {
            pages: exported,
            total_pages,
            ..
        } => {
            assert_eq!(exported, 2);
            assert_eq!(total_pages, 4);
        }
        _ => panic!("expected Pdf output"),
    }

    let svg_ok = compile::compile(CompileRequest {
        source: src,
        inputs: Dict::new(),
        base_path: PathBuf::from("."),
        allow_downloads: false,
        format: OutputFormat::Svg,
        pages: Some(pages),
        pdf_standards: PdfStandards::default(),
        pdf_tagged: false,
        pdf_timestamp: None,
        world_date: WorldDate::None,
        features: Vec::new(),
        evaluate: None,
        extra_diagnostics: Vec::new(),
    })
    .expect("filtered SVG compile");

    match svg_ok.output {
        CompileOutput::Svg {
            pages: svgs,
            total_pages,
        } => {
            assert_eq!(svgs.len(), 2);
            assert_eq!(total_pages, 4);
        }
        _ => panic!("expected Svg output"),
    }
}

#[test]
fn wstp_unqualified_symbols_resolve_uniformly_to_system_context() {
    assert_eq!(resolve_unqualified_symbol("Show").as_str(), "System`Show");
    assert_eq!(
        resolve_unqualified_symbol("ImportByteArray").as_str(),
        "System`ImportByteArray"
    );
    assert_eq!(
        resolve_unqualified_symbol("Integrate").as_str(),
        "System`Integrate"
    );
    assert_eq!(
        resolve_unqualified_symbol("FilledCurve").as_str(),
        "System`FilledCurve"
    );
}
