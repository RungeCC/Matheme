//! Wolfram-agnostic Typst compilation, export (PDF, SVG, HTML), and
//! simultaneous expression evaluation against the compiled document's scope and
//! introspector.
//!
//! Shared by both the LibraryLink WSTP bridge (`matheme`) and the standalone
//! CLI (`matheme-cli`).

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

use typst::comemo::Track;
use typst::diag::{SourceDiagnostic, SourceResult};
use typst::engine::{Route, Sink, Traced};
use typst::foundations::{Context, Dict, StyleChain, Target, TargetElem, Value};
use typst::introspection::Introspector;
use typst::routines::SpanMode;
use typst::syntax::{Span, SyntaxMode};
use typst::World;
use typst_html::{html, HtmlDocument, HtmlOptions};
use typst_layout::PagedDocument;
use typst_pdf::{pdf, PdfOptions};
use typst_svg::{svg, SvgOptions};

pub use typst::layout::{PageRange, PageRanges};
pub use typst::{Feature, Features};
pub use typst_pdf::{PdfStandard, PdfStandards, Timestamp};

use crate::diagnostics::{self, Diagnostic, DiagnosticSeverity};
use crate::graphics::{self, VectorPage};
use crate::world::{MathemeWorld, WorldDate};

/// Evict memoized Typst entries older than this many compilations so memory
/// stays bounded in long-running sessions.
pub const COMEMO_MAX_AGE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Pdf,
    Svg,
    Html,
    Graphics,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        if name.eq_ignore_ascii_case("pdf") {
            Ok(Self::Pdf)
        } else if name.eq_ignore_ascii_case("svg") {
            Ok(Self::Svg)
        } else if name.eq_ignore_ascii_case("html") {
            Ok(Self::Html)
        } else if name.eq_ignore_ascii_case("graphics") {
            Ok(Self::Graphics)
        } else {
            Err(format!("unknown output format: {name:?}"))
        }
    }
}

impl OutputFormat {
    pub fn parse(name: &str) -> Option<Self> {
        name.parse().ok()
    }

    pub fn target(self) -> Target {
        match self {
            Self::Pdf | Self::Svg | Self::Graphics => Target::Paged,
            Self::Html => Target::Html,
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
        pages: usize,
    },
    Svg {
        pages: Vec<String>,
        total_pages: usize,
    },
    Html {
        html: String,
    },
    Graphics {
        pages: Vec<VectorPage>,
        total_pages: usize,
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

pub struct CompileSuccess {
    pub output: CompileOutput,
    pub evaluated: Option<EvaluatedValues>,
    pub diagnostics: Vec<Diagnostic>,
    pub formatted: String,
}

pub struct CompileFailure {
    pub summary: String,
    pub error_count: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub formatted: String,
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

pub fn parse_pdf_standard(name: &str) -> Option<PdfStandard> {
    match name.to_ascii_lowercase().as_str() {
        "1.4" => Some(PdfStandard::V_1_4),
        "1.5" => Some(PdfStandard::V_1_5),
        "1.6" => Some(PdfStandard::V_1_6),
        "1.7" => Some(PdfStandard::V_1_7),
        "2.0" => Some(PdfStandard::V_2_0),
        "a-1b" => Some(PdfStandard::A_1b),
        "a-1a" => Some(PdfStandard::A_1a),
        "a-2b" => Some(PdfStandard::A_2b),
        "a-2u" => Some(PdfStandard::A_2u),
        "a-2a" => Some(PdfStandard::A_2a),
        "a-3b" => Some(PdfStandard::A_3b),
        "a-3u" => Some(PdfStandard::A_3u),
        "a-3a" => Some(PdfStandard::A_3a),
        "a-4" => Some(PdfStandard::A_4),
        "a-4f" => Some(PdfStandard::A_4f),
        "a-4e" => Some(PdfStandard::A_4e),
        "ua-1" => Some(PdfStandard::Ua_1),
        _ => None,
    }
}

pub fn parse_pdf_standards(names: &[String]) -> Result<PdfStandards, String> {
    let mut parsed = Vec::with_capacity(names.len());
    for name in names {
        let std = parse_pdf_standard(name)
            .ok_or_else(|| format!("unknown PDFStandard: {name:?}"))?;
        parsed.push(std);
    }
    PdfStandards::new(&parsed)
        .map_err(|err| format!("invalid PDFStandard combination: {}", err.message()))
}

pub fn parse_feature(name: &str) -> Option<Feature> {
    if name.eq_ignore_ascii_case("html") {
        Some(Feature::Html)
    } else if name.eq_ignore_ascii_case("bundle") {
        Some(Feature::Bundle)
    } else if name.eq_ignore_ascii_case("a11y-extras") || name.eq_ignore_ascii_case("a11yextras") {
        Some(Feature::A11yExtras)
    } else {
        None
    }
}

pub fn parse_features(names: &[String]) -> Result<Vec<Feature>, String> {
    let mut selected = Vec::with_capacity(names.len());
    for name in names {
        let feature = parse_feature(name)
            .ok_or_else(|| format!("unknown Typst feature: {name:?}"))?;
        selected.push(feature);
    }
    Ok(selected)
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
        diagnostics.push(diagnostics::warning(
            "Package downloads are disabled; enable downloads to fetch missing packages.",
        ));
    }
}

/// Compile a Typst source document, export it to the requested format, and
/// optionally evaluate Typst code expressions against the compiled document.
pub fn compile(request: CompileRequest) -> Result<CompileSuccess, CompileFailure> {
    let mut diagnostics = request.extra_diagnostics;

    let base_path = if request.base_path.is_absolute() {
        request.base_path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(request.base_path)
    };

    let features = resolve_features(request.features, request.format);

    let download_scope = crate::enter_call_downloads(request.allow_downloads);
    let world = MathemeWorld::with_options(
        request.source,
        request.inputs,
        base_path,
        features,
        request.world_date,
    );

    let mut typst_diags: Vec<SourceDiagnostic> = Vec::new();
    let mut output: Option<CompileOutput> = None;
    let mut evaluated: Option<EvaluatedValues> = None;

    let failed = match request.format {
        OutputFormat::Pdf | OutputFormat::Svg | OutputFormat::Graphics => {
            let warned = typst::compile::<PagedDocument>(&world);
            record_typst_diags(&world, &warned.warnings, &mut typst_diags, &mut diagnostics);
            match warned.output {
                Ok(document) => {
                    let total_pages = document.pages().len();
                    let export_res = match request.format {
                        OutputFormat::Pdf => {
                            let opts = PdfOptions {
                                standards: request.pdf_standards,
                                tagged: request.pdf_tagged,
                                timestamp: request.pdf_timestamp,
                                page_ranges: request.pages.clone(),
                                ..PdfOptions::default()
                            };
                            match pdf(&document, &opts) {
                                Ok(bytes) => {
                                    output = Some(CompileOutput::Pdf {
                                        bytes,
                                        pages: total_pages,
                                    });
                                    Ok(())
                                }
                                Err(errs) => Err(errs),
                            }
                        }
                        OutputFormat::Svg => {
                            let svgs = export_svg(&document, request.pages.as_ref());
                            output = Some(CompileOutput::Svg {
                                pages: svgs,
                                total_pages,
                            });
                            Ok(())
                        }
                        OutputFormat::Graphics => {
                            let pages =
                                graphics::export_graphics(&document, request.pages.as_ref());
                            output = Some(CompileOutput::Graphics { pages, total_pages });
                            Ok(())
                        }
                        OutputFormat::Html => unreachable!(),
                    };

                    if let Err(errs) = export_res {
                        record_typst_diags(&world, &errs, &mut typst_diags, &mut diagnostics);
                        true
                    } else if let Some(ref spec) = request.evaluate {
                        match evaluate_spec(
                            &world,
                            document.introspector().as_ref(),
                            request.format.target(),
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
            }
        }
        OutputFormat::Html => {
            let warned = typst::compile::<HtmlDocument>(&world);
            record_typst_diags(&world, &warned.warnings, &mut typst_diags, &mut diagnostics);
            match warned.output {
                Ok(document) => match html(&document, &HtmlOptions::default()) {
                    Ok(html_text) => {
                        output = Some(CompileOutput::Html { html: html_text });
                        if let Some(ref spec) = request.evaluate {
                            match evaluate_spec(
                                &world,
                                document.introspector().as_ref(),
                                OutputFormat::Html.target(),
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
            }
        }
    };

    if failed {
        maybe_offline_hint(&mut diagnostics, download_scope.download_blocked());
        let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);
        crate::evict_caches(COMEMO_MAX_AGE);

        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .collect();
        let summary = errors
            .first()
            .map(|diagnostic| diagnostic.message.clone())
            .unwrap_or_else(|| "compilation failed".into());
        return Err(CompileFailure {
            summary,
            error_count: errors.len(),
            diagnostics,
            formatted,
        });
    }

    let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);
    crate::evict_caches(COMEMO_MAX_AGE);

    Ok(CompileSuccess {
        output: output.expect("successful compile always produces output"),
        evaluated,
        diagnostics,
        formatted,
    })
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
    module_scope: &typst::foundations::Scope,
    introspector: &dyn Introspector,
    context: &Context,
    code: &str,
    typst_diags: &mut Vec<SourceDiagnostic>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Value, ()> {
    let mut sink = Sink::new();
    let res: SourceResult<Value> = typst_eval::eval_string(
        (world as &dyn World).track(),
        world.library(),
        sink.track_mut(),
        introspector.track(),
        context.track(),
        code,
        SpanMode::Uniform(Span::detached()),
        SyntaxMode::Code,
        module_scope.clone(),
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
    let main_source = match world.source(world.main()) {
        Ok(source) => source,
        Err(err) => {
            diagnostics.push(diagnostics::error(format!(
                "failed to load main source for evaluation: {err}"
            )));
            return Err(());
        }
    };

    // Warnings from `main_source` were already recorded during `typst::compile`
    // above, so we intentionally do not re-emit `module_sink.warnings()`.
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
    let scope = module.scope();

    match spec {
        EvaluateSpec::Single(code) => {
            eval_one(world, scope, introspector, &context, code, typst_diags, diagnostics)
                .map(EvaluatedValues::Single)
        }
        EvaluateSpec::List(codes) => {
            let mut out = Vec::with_capacity(codes.len());
            for code in codes {
                out.push(eval_one(
                    world,
                    scope,
                    introspector,
                    &context,
                    code,
                    typst_diags,
                    diagnostics,
                )?);
            }
            Ok(EvaluatedValues::List(out))
        }
        EvaluateSpec::Map(pairs) => {
            let mut out = Vec::with_capacity(pairs.len());
            for (key, code) in pairs {
                let val = eval_one(
                    world,
                    scope,
                    introspector,
                    &context,
                    code,
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
        let mut diags = vec![diagnostics::error("cannot find variable package")];
        maybe_offline_hint(&mut diags, false);
        assert_eq!(
            diags.len(),
            1,
            "unrelated error mentioning 'package' must not trigger offline hint"
        );

        maybe_offline_hint(&mut diags, true);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[1].severity, DiagnosticSeverity::Warning);
    }
}
