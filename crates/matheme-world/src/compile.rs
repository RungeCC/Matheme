//! Wolfram-agnostic Typst compilation, export (PDF, SVG, HTML), and
//! simultaneous expression evaluation against the compiled document's scope and
//! introspector.
//!
//! Shared by both the LibraryLink WSTP bridge (`matheme`) and the standalone
//! CLI (`matheme-cli`).

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;
use serde::de::IntoDeserializer;
use typst::World;
use typst::comemo::Track;
use typst::diag::{SourceDiagnostic, SourceResult};
use typst::engine::{Route, Sink, Traced};
use typst::foundations::{Context, Dict, StyleChain, Target, TargetElem, Value};
use typst::introspection::Introspector;
use typst::routines::SpanMode;
use typst::syntax::{Span, SyntaxMode};
use typst_html::{HtmlDocument, HtmlOptions, html};
use typst_pdf::{PdfOptions, pdf};
use typst_svg::{SvgOptions, svg};

pub use typst::layout::{PageRange, PageRanges};
pub use typst::{Feature, Features};
pub use typst_layout::PagedDocument;
pub use typst_pdf::{PdfStandard, PdfStandards, Timestamp};

use crate::diagnostics::{self, Diagnostic, DiagnosticSeverity, DiagnosticSource};
use crate::world::{MathemeWorld, WorldDate};

/// Evict memoized Typst entries older than this many compilations so memory
/// stays bounded in long-running sessions.
pub const COMEMO_MAX_AGE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Pdf,
    Svg,
    Html,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        crate::deserialize_name(name).ok_or_else(|| format!("unknown output format: {name:?}"))
    }
}

impl OutputFormat {
    pub fn parse(name: &str) -> Option<Self> {
        name.parse().ok()
    }

    pub fn target(self) -> Target {
        match self {
            Self::Pdf | Self::Svg => Target::Paged,
            Self::Html => Target::Html,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Svg => "svg",
            Self::Html => "html",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluateSpec {
    Single(String),
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

#[derive(Debug, Clone)]
pub enum EvaluatedValues {
    Single(Value),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

#[derive(Debug, Clone)]
pub enum CompileOutput {
    Pdf {
        bytes: Vec<u8>,
        /// Number of pages actually written into `bytes` (after `Pages` filtering).
        pages: usize,
        /// Total number of pages in the compiled document (before `Pages` filtering).
        total_pages: usize,
    },
    Svg {
        pages: Vec<String>,
        total_pages: usize,
    },
    Html {
        html: String,
    },
}

pub struct CompileRequest {
    pub source: String,
    pub inputs: Dict,
    pub base_path: PathBuf,
    pub allow_downloads: bool,
    pub format: OutputFormat,
    pub pages: Option<PageRanges>,
    pub pdf_standards: PdfStandards,
    pub pdf_tagged: bool,
    pub pdf_timestamp: Option<Timestamp>,
    pub world_date: WorldDate,
    pub features: Vec<Feature>,
    pub evaluate: Option<EvaluateSpec>,
    pub extra_diagnostics: Vec<Diagnostic>,
}

impl CompileRequest {
    /// Extract the format-independent compilation fields shared by paged and
    /// HTML pipelines.
    pub fn into_paged_request(self) -> PagedCompileRequest {
        PagedCompileRequest {
            source: self.source,
            inputs: self.inputs,
            base_path: self.base_path,
            allow_downloads: self.allow_downloads,
            pages: self.pages,
            world_date: self.world_date,
            features: self.features,
            evaluate: self.evaluate,
            extra_diagnostics: self.extra_diagnostics,
        }
    }
}

pub struct CompileSuccess {
    pub output: CompileOutput,
    pub evaluated: Option<EvaluatedValues>,
    pub diagnostics: Vec<Diagnostic>,
    pub formatted: String,
}

pub struct PagedCompileRequest {
    pub source: String,
    pub inputs: Dict,
    pub base_path: PathBuf,
    pub allow_downloads: bool,
    pub pages: Option<PageRanges>,
    pub world_date: WorldDate,
    pub features: Vec<Feature>,
    pub evaluate: Option<EvaluateSpec>,
    pub extra_diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub struct PagedCompileSuccess<T> {
    pub output: T,
    /// Number of pages exported after applying `Pages` filtering.
    pub exported_pages: usize,
    /// Total number of pages in the compiled document before filtering.
    pub total_pages: usize,
    pub evaluated: Option<EvaluatedValues>,
    pub diagnostics: Vec<Diagnostic>,
    pub formatted: String,
}

#[derive(Debug)]
pub struct CompileFailure {
    pub summary: String,
    pub error_count: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub formatted: String,
}

impl CompileFailure {
    /// Uniform human-readable summary shared by the bridge and CLI.
    pub fn display_message(&self) -> String {
        format!(
            "Typst compilation failed with {} error(s): {}",
            self.error_count, self.summary
        )
    }
}

/// Resolve a relative `base_path` against the current working directory.
pub fn normalize_base_path(base_path: PathBuf) -> PathBuf {
    if base_path.is_absolute() {
        base_path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(base_path)
    }
}

/// Count how many 1-based pages in `1..=total_pages` match `pages` (or all of
/// them when `pages` is `None`).
pub fn count_exported_pages(total_pages: usize, pages: Option<&PageRanges>) -> usize {
    match pages {
        None => total_pages,
        Some(ranges) => (1..=total_pages)
            .filter(|&idx| NonZeroUsize::new(idx).is_some_and(|n| ranges.includes_page(n)))
            .count(),
    }
}

/// Parse a CLI-style page range string such as `"1,3-5,-2,7-"`.
pub fn parse_page_str(spec: &str) -> Result<PageRanges, String> {
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("empty page range component".into());
        }
        let range = if let Some((start, end)) = part.split_once('-') {
            let start = start.trim();
            let end = end.trim();
            let lo = if start.is_empty() {
                None
            } else {
                Some(
                    start
                        .parse::<NonZeroUsize>()
                        .map_err(|_| format!("invalid page number: {start:?}"))?,
                )
            };
            let hi = if end.is_empty() {
                None
            } else {
                Some(
                    end.parse::<NonZeroUsize>()
                        .map_err(|_| format!("invalid page number: {end:?}"))?,
                )
            };
            if lo.is_none() && hi.is_none() {
                return Err("page range '-' cannot have both ends empty".into());
            }
            if lo.zip(hi).is_some_and(|(start, end)| start > end) {
                return Err(format!("page range starts after its end: {part:?}"));
            }
            lo..=hi
        } else {
            let page = part
                .parse::<NonZeroUsize>()
                .map_err(|_| format!("invalid page number: {part:?}"))?;
            Some(page)..=Some(page)
        };
        ranges.push(range);
    }
    Ok(PageRanges::new(ranges))
}

pub fn parse_pdf_standard(name: &str) -> Result<PdfStandard, String> {
    crate::deserialize_name(name).ok_or_else(|| format!("unknown PDFStandard: {name:?}"))
}

pub fn validate_pdf_standards(standards: &[PdfStandard]) -> Result<PdfStandards, String> {
    PdfStandards::new(standards)
        .map_err(|err| format!("invalid PDFStandard combination: {}", err.message()))
}

pub fn pdf_standards_require_tags(standards: &[PdfStandard]) -> bool {
    standards.iter().any(|standard| {
        matches!(
            standard,
            PdfStandard::A_1a | PdfStandard::A_2a | PdfStandard::A_3a | PdfStandard::Ua_1
        )
    })
}

#[derive(Deserialize)]
#[serde(remote = "Feature", rename_all = "kebab-case")]
enum FeatureName {
    Html,
    Bundle,
    #[serde(alias = "a11yextras")]
    A11yExtras,
}

pub fn parse_feature(name: &str) -> Result<Feature, String> {
    deserialize_feature(name.to_ascii_lowercase().as_str().into_deserializer())
        .map_err(|_: serde::de::value::Error| format!("unknown Typst feature: {name:?}"))
}

pub fn deserialize_feature<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Feature, D::Error> {
    FeatureName::deserialize(deserializer)
}

pub fn parse_features(names: &[String]) -> Result<Vec<Feature>, String> {
    names.iter().map(|name| parse_feature(name)).collect()
}

/// Build Typst's [`Features`] bitset, automatically enabling [`Feature::Html`]
/// when targeting HTML output.
fn resolve_features(mut requested: Vec<Feature>, format: OutputFormat) -> Features {
    if format == OutputFormat::Html && !requested.contains(&Feature::Html) {
        requested.push(Feature::Html);
    }
    requested.into_iter().collect()
}

fn maybe_offline_hint(diagnostics: &mut Vec<Diagnostic>, download_blocked: bool) {
    if download_blocked {
        diagnostics.push(diagnostics::hint(
            DiagnosticSource::Package,
            "Package downloads are disabled; enable downloads to fetch missing packages.",
        ));
    }
}

trait CompiledDoc: Sized {
    fn compile_world(world: &MathemeWorld) -> typst::diag::Warned<SourceResult<Self>>;
    fn dyn_introspector(&self) -> &dyn Introspector;
}

impl CompiledDoc for PagedDocument {
    fn compile_world(world: &MathemeWorld) -> typst::diag::Warned<SourceResult<Self>> {
        typst::compile::<PagedDocument>(world)
    }

    fn dyn_introspector(&self) -> &dyn Introspector {
        self.introspector().as_ref()
    }
}

impl CompiledDoc for HtmlDocument {
    fn compile_world(world: &MathemeWorld) -> typst::diag::Warned<SourceResult<Self>> {
        typst::compile::<HtmlDocument>(world)
    }

    fn dyn_introspector(&self) -> &dyn Introspector {
        self.introspector().as_ref()
    }
}

struct CompiledPipelineResult<T> {
    output: T,
    evaluated: Option<EvaluatedValues>,
    diagnostics: Vec<Diagnostic>,
    formatted: String,
}

/// Unified compilation, export, evaluation, diagnostic formatting, and cache
/// eviction pipeline shared by Paged (PDF/SVG/Graphics) and HTML targets.
fn compile_document<D, T, F>(
    request: PagedCompileRequest,
    target: Target,
    features: Features,
    export: F,
) -> Result<CompiledPipelineResult<T>, CompileFailure>
where
    D: CompiledDoc,
    F: FnOnce(&D, Option<&PageRanges>) -> Result<(T, Vec<Diagnostic>), Vec<SourceDiagnostic>>,
{
    let mut diagnostics = request.extra_diagnostics;
    let base_path = normalize_base_path(request.base_path);

    let download_scope = crate::enter_call_downloads(request.allow_downloads);
    let world = MathemeWorld::with_options(
        request.source,
        request.inputs,
        base_path,
        features,
        request.world_date,
    );

    let mut typst_diags: Vec<SourceDiagnostic> = Vec::new();
    let mut output: Option<T> = None;
    let mut evaluated: Option<EvaluatedValues> = None;

    let warned = D::compile_world(&world);
    record_typst_diags(&world, &warned.warnings, &mut typst_diags, &mut diagnostics);

    let failed = match warned.output {
        Ok(document) => match export(&document, request.pages.as_ref()) {
            Ok((exported, export_diags)) => {
                diagnostics.extend(export_diags);
                output = Some(exported);
                if let Some(ref spec) = request.evaluate {
                    match evaluate_spec(
                        &world,
                        document.dyn_introspector(),
                        target,
                        spec,
                        &mut typst_diags,
                        &mut diagnostics,
                    ) {
                        Ok(values) => {
                            evaluated = Some(values);
                            false
                        }
                        Err(()) => true,
                    }
                } else {
                    false
                }
            }
            Err(errs) => {
                record_typst_diags(&world, &errs, &mut typst_diags, &mut diagnostics);
                true
            }
        },
        Err(errs) => {
            record_typst_diags(&world, &errs, &mut typst_diags, &mut diagnostics);
            true
        }
    };

    if failed {
        return Err(finish_compile_failure(
            &world,
            &typst_diags,
            diagnostics,
            download_scope.download_blocked(),
        ));
    }

    let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);
    crate::evict_caches(COMEMO_MAX_AGE);

    Ok(CompiledPipelineResult {
        output: output.expect("successful compile always produces output"),
        evaluated,
        diagnostics,
        formatted,
    })
}

/// Compile a Typst source document to a [`PagedDocument`], run a caller-supplied
/// exporter on it, and optionally evaluate Typst code expressions against the
/// compiled document's scope and introspector.
pub fn compile_paged_with<T, F>(
    request: PagedCompileRequest,
    export: F,
) -> Result<PagedCompileSuccess<T>, CompileFailure>
where
    F: FnOnce(
        &PagedDocument,
        Option<&PageRanges>,
    ) -> Result<(T, Vec<Diagnostic>), Vec<SourceDiagnostic>>,
{
    let features = resolve_features(request.features.clone(), OutputFormat::Pdf);
    let result = compile_document::<PagedDocument, (T, usize, usize), _>(
        request,
        Target::Paged,
        features,
        |document, pages| {
            let total_pages = document.pages().len();
            let exported_pages = count_exported_pages(total_pages, pages);
            let (exported, export_diags) = export(document, pages)?;
            Ok(((exported, exported_pages, total_pages), export_diags))
        },
    )?;

    let (output, exported_pages, total_pages) = result.output;
    Ok(PagedCompileSuccess {
        output,
        exported_pages,
        total_pages,
        evaluated: result.evaluated,
        diagnostics: result.diagnostics,
        formatted: result.formatted,
    })
}

fn finish_compile_failure(
    world: &MathemeWorld,
    typst_diags: &[SourceDiagnostic],
    mut diagnostics: Vec<Diagnostic>,
    download_blocked: bool,
) -> CompileFailure {
    maybe_offline_hint(&mut diagnostics, download_blocked);
    let formatted = diagnostics::format(world, typst_diags, &diagnostics);
    crate::evict_caches(COMEMO_MAX_AGE);

    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .collect();
    let summary = errors
        .first()
        .map(|diagnostic| diagnostic.message.clone())
        .unwrap_or_else(|| "compilation failed".into());
    CompileFailure {
        summary,
        error_count: errors.len(),
        diagnostics,
        formatted,
    }
}

/// Compile a Typst source document, export it to the requested format, and
/// optionally evaluate Typst code expressions against the compiled document.
pub fn compile(request: CompileRequest) -> Result<CompileSuccess, CompileFailure> {
    let format = request.format;
    match format {
        OutputFormat::Pdf | OutputFormat::Svg => {
            let pdf_standards = request.pdf_standards.clone();
            let pdf_tagged = request.pdf_tagged;
            let pdf_timestamp = request.pdf_timestamp;
            let paged_req = request.into_paged_request();

            let success = compile_paged_with(paged_req, |document, pages| match format {
                OutputFormat::Pdf => {
                    let total_pages = document.pages().len();
                    let exported_pages = count_exported_pages(total_pages, pages);
                    if exported_pages == 0 {
                        return Err(vec![
                            typst::diag::error!(
                                Span::detached(),
                                "page selection matches no pages"
                            )
                            .into(),
                        ]);
                    }
                    let opts = PdfOptions {
                        standards: pdf_standards,
                        tagged: pdf_tagged,
                        timestamp: pdf_timestamp,
                        page_ranges: pages.cloned(),
                        ..PdfOptions::default()
                    };
                    let bytes = pdf(document, &opts)
                        .map_err(|errs| errs.into_iter().collect::<Vec<_>>())?;
                    Ok((
                        CompileOutput::Pdf {
                            bytes,
                            pages: exported_pages,
                            total_pages,
                        },
                        Vec::new(),
                    ))
                }
                OutputFormat::Svg => {
                    let svgs = export_svg(document, pages);
                    let total_pages = document.pages().len();
                    Ok((
                        CompileOutput::Svg {
                            pages: svgs,
                            total_pages,
                        },
                        Vec::new(),
                    ))
                }
                OutputFormat::Html => unreachable!(),
            })?;

            Ok(CompileSuccess {
                output: success.output,
                evaluated: success.evaluated,
                diagnostics: success.diagnostics,
                formatted: success.formatted,
            })
        }
        OutputFormat::Html => {
            if request.pages.is_some() {
                let msg = "\"Pages\" filtering is not supported for HTML output.".to_string();
                return Err(CompileFailure {
                    summary: msg.clone(),
                    error_count: 1,
                    diagnostics: vec![diagnostics::error(DiagnosticSource::Host, msg.clone())],
                    formatted: format!("error: {msg}\n"),
                });
            }
            let features = resolve_features(request.features.clone(), OutputFormat::Html);
            let paged_req = request.into_paged_request();
            let result = compile_document::<HtmlDocument, CompileOutput, _>(
                paged_req,
                OutputFormat::Html.target(),
                features,
                |document, _pages| {
                    let html_text = html(document, &HtmlOptions::default())
                        .map_err(|errs| errs.into_iter().collect::<Vec<_>>())?;
                    Ok((CompileOutput::Html { html: html_text }, Vec::new()))
                },
            )?;
            Ok(CompileSuccess {
                output: result.output,
                evaluated: result.evaluated,
                diagnostics: result.diagnostics,
                formatted: result.formatted,
            })
        }
    }
}

fn record_typst_diags(
    world: &MathemeWorld,
    items: &[SourceDiagnostic],
    typst_diags: &mut Vec<SourceDiagnostic>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    typst_diags.extend(items.iter().cloned());
    diagnostics.extend(items.iter().map(|d| diagnostics::from_typst(d, world)));
}

fn export_svg(document: &PagedDocument, pages: Option<&PageRanges>) -> Vec<String> {
    let opts = SvgOptions::default();
    document
        .pages()
        .iter()
        .enumerate()
        .filter(|(idx, _)| {
            pages.is_none_or(|ranges| {
                NonZeroUsize::new(idx + 1).is_some_and(|n| ranges.includes_page(n))
            })
        })
        .map(|(_, page)| svg(page, &opts))
        .collect()
}

fn eval_one(
    world: &MathemeWorld,
    module_scope: typst::foundations::Scope,
    introspector: &dyn Introspector,
    context: &Context,
    source: (Span, &str),
    typst_diags: &mut Vec<SourceDiagnostic>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Value, ()> {
    let (span, code) = source;
    let mut sink = Sink::new();
    let res: SourceResult<Value> = typst_eval::eval_string(
        (world as &dyn World).track(),
        world.library(),
        sink.track_mut(),
        introspector.track(),
        context.track(),
        code,
        SpanMode::Uniform(span),
        SyntaxMode::Code,
        module_scope,
    );
    let warnings = sink.warnings();
    record_typst_diags(world, &warnings, typst_diags, diagnostics);

    match res {
        Ok(value) => Ok(value),
        Err(errs) => {
            record_typst_diags(world, &errs, typst_diags, diagnostics);
            Err(())
        }
    }
}

fn evaluate_spec(
    world: &MathemeWorld,
    introspector: &dyn Introspector,
    target: Target,
    spec: &EvaluateSpec,
    typst_diags: &mut Vec<SourceDiagnostic>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<EvaluatedValues, ()> {
    match spec {
        EvaluateSpec::List(codes) if codes.is_empty() => {
            return Ok(EvaluatedValues::List(Vec::new()));
        }
        EvaluateSpec::Map(pairs) if pairs.is_empty() => {
            return Ok(EvaluatedValues::Map(Vec::new()));
        }
        _ => {}
    }

    let main_source = match world.source(world.main()) {
        Ok(source) => source,
        Err(err) => {
            diagnostics.push(diagnostics::error(
                DiagnosticSource::Host,
                format!("failed to load main source for evaluation: {err}"),
            ));
            return Err(());
        }
    };

    // `typst_eval::eval` is comemo-memoized against the same `world` and
    // `main_source` just compiled by `typst::compile`, so this hits comemo's
    // cache before `evict_caches` runs. Warnings from `main_source` were
    // already recorded during `typst::compile`, so we do not re-emit them.
    let mut module_sink = Sink::new();
    let module = match typst_eval::eval(
        (world as &dyn World).track(),
        world.library(),
        Traced::default().track(),
        module_sink.track_mut(),
        Route::default().track(),
        &main_source,
    ) {
        Ok(module) => module,
        Err(errs) => {
            record_typst_diags(world, &errs, typst_diags, diagnostics);
            return Err(());
        }
    };

    let base = StyleChain::new(&world.library().styles);
    let target_style = TargetElem::target.set(target).wrap();
    let styles = base.chain(&target_style);
    let context = Context::new(None, Some(styles));
    let mut owned_scope = Some(module.scope().clone());
    // Give code expressions the same project-relative path base as the main source.
    let main_span = main_source.root().span();

    match spec {
        EvaluateSpec::Single(code) => eval_one(
            world,
            owned_scope.take().unwrap(),
            introspector,
            &context,
            (main_span, code),
            typst_diags,
            diagnostics,
        )
        .map(EvaluatedValues::Single),
        EvaluateSpec::List(codes) => {
            let mut out = Vec::with_capacity(codes.len());
            let last = codes.len().saturating_sub(1);
            for (idx, code) in codes.iter().enumerate() {
                let scope = if idx == last {
                    owned_scope.take().unwrap()
                } else {
                    owned_scope.as_ref().unwrap().clone()
                };
                out.push(eval_one(
                    world,
                    scope,
                    introspector,
                    &context,
                    (main_span, code),
                    typst_diags,
                    diagnostics,
                )?);
            }
            Ok(EvaluatedValues::List(out))
        }
        EvaluateSpec::Map(pairs) => {
            let mut out = Vec::with_capacity(pairs.len());
            let last = pairs.len().saturating_sub(1);
            for (idx, (key, code)) in pairs.iter().enumerate() {
                let scope = if idx == last {
                    owned_scope.take().unwrap()
                } else {
                    owned_scope.as_ref().unwrap().clone()
                };
                let val = eval_one(
                    world,
                    scope,
                    introspector,
                    &context,
                    (main_span, code),
                    typst_diags,
                    diagnostics,
                )?;
                out.push((key.clone(), val));
            }
            Ok(EvaluatedValues::Map(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_ranges() {
        let ranges = parse_page_str("1, 3-5, -2, 7-").expect("valid page ranges");
        assert!(ranges.includes_page(NonZeroUsize::new(1).unwrap()));
        assert!(ranges.includes_page(NonZeroUsize::new(4).unwrap()));
        assert!(!ranges.includes_page(NonZeroUsize::new(6).unwrap()));
        assert!(ranges.includes_page(NonZeroUsize::new(9).unwrap()));
        assert!(parse_page_str("-").is_err());
        assert!(parse_page_str("0").is_err());
    }

    #[test]
    fn parses_features_and_auto_enables_html() {
        let parsed = parse_features(&["a11y-extras".into()]).unwrap();
        let features = resolve_features(parsed, OutputFormat::Html);
        assert!(features.is_enabled(Feature::Html));
        assert!(features.is_enabled(Feature::A11yExtras));
        assert!(parse_features(&["bogus".into()]).is_err());
    }

    #[test]
    fn offline_hint_only_fires_when_download_was_actually_blocked() {
        let mut diags = vec![diagnostics::error(
            DiagnosticSource::Typst,
            "cannot find variable package",
        )];
        maybe_offline_hint(&mut diags, false);
        assert_eq!(
            diags.len(),
            1,
            "unrelated error mentioning 'package' must not trigger offline hint"
        );

        maybe_offline_hint(&mut diags, true);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[1].severity, DiagnosticSeverity::Hint);
        assert_eq!(diags[1].source, DiagnosticSource::Package);
    }

    #[test]
    fn compile_paged_with_propagates_custom_exporter_diagnostics() {
        let req = PagedCompileRequest {
            source: "= Hello".into(),
            inputs: Dict::new(),
            base_path: PathBuf::from("."),
            allow_downloads: false,
            pages: None,
            world_date: WorldDate::None,
            features: Vec::new(),
            evaluate: None,
            extra_diagnostics: Vec::new(),
        };
        let success = compile_paged_with(req, |doc, _pages| {
            let pages = doc.pages().len();
            Ok((
                pages,
                vec![diagnostics::warning(
                    DiagnosticSource::Graphics,
                    "custom exporter warning",
                )],
            ))
        })
        .expect("custom exporter compile should succeed");

        assert_eq!(success.exported_pages, 1);
        assert_eq!(success.total_pages, 1);
        assert_eq!(success.output, 1);
        assert!(
            success
                .diagnostics
                .iter()
                .any(|d| d.message.contains("custom exporter warning")
                    && d.source == DiagnosticSource::Graphics)
        );
        assert!(
            success
                .formatted
                .contains("warning: custom exporter warning"),
            "non-Typst warnings must format with 'warning:' prefix, got: {}",
            success.formatted
        );
    }

    #[test]
    fn html_format_rejects_pages_filter() {
        let req = CompileRequest {
            source: "= Hello".into(),
            inputs: Dict::new(),
            base_path: PathBuf::from("."),
            allow_downloads: false,
            format: OutputFormat::Html,
            pages: Some(parse_page_str("1").unwrap()),
            pdf_standards: PdfStandards::default(),
            pdf_tagged: false,
            pdf_timestamp: None,
            world_date: WorldDate::None,
            features: Vec::new(),
            evaluate: None,
            extra_diagnostics: Vec::new(),
        };
        let err = compile(req).err().expect("HTML with Pages must fail");
        assert!(err.summary.contains("HTML"), "{}", err.summary);
        assert_eq!(err.error_count, 1);
    }

    #[test]
    fn multipage_filter_reports_exported_pages_distinct_from_total_pages() {
        let src = "Page 1\n#pagebreak()\nPage 2\n#pagebreak()\nPage 3".to_string();
        let req = CompileRequest {
            source: src.clone(),
            inputs: Dict::new(),
            base_path: PathBuf::from("."),
            allow_downloads: false,
            format: OutputFormat::Pdf,
            pages: Some(parse_page_str("2").unwrap()),
            pdf_standards: PdfStandards::default(),
            pdf_tagged: false,
            pdf_timestamp: None,
            world_date: WorldDate::None,
            features: Vec::new(),
            evaluate: None,
            extra_diagnostics: Vec::new(),
        };
        let ok = compile(req).expect("filtered PDF compile");
        match ok.output {
            CompileOutput::Pdf {
                pages, total_pages, ..
            } => {
                assert_eq!(pages, 1);
                assert_eq!(total_pages, 3);
            }
            _ => panic!("expected Pdf output"),
        }

        let svg_req = CompileRequest {
            source: src,
            inputs: Dict::new(),
            base_path: PathBuf::from("."),
            allow_downloads: false,
            format: OutputFormat::Svg,
            pages: Some(parse_page_str("1,3").unwrap()),
            pdf_standards: PdfStandards::default(),
            pdf_tagged: false,
            pdf_timestamp: None,
            world_date: WorldDate::None,
            features: Vec::new(),
            evaluate: None,
            extra_diagnostics: Vec::new(),
        };
        let svg_ok = compile(svg_req).expect("filtered SVG compile");
        match svg_ok.output {
            CompileOutput::Svg { pages, total_pages } => {
                assert_eq!(pages.len(), 2);
                assert_eq!(total_pages, 3);
            }
            _ => panic!("expected Svg output"),
        }
    }
}
