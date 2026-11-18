//! General `MathemeCompile` endpoint supporting multiple output formats
//! (PDF, SVG, HTML), `typst-cli`-style compilation options, and simultaneous
//! expression evaluation against the compiled document's scope and introspector.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use matheme_wl::{from_typst, rule, to_typst, Entry, Expr, Rule, Symbol};
use matheme_world::{self as world, datetime_from_unix, MathemeWorld, WorldDate};
use typst::comemo::Track;
use typst::diag::{SourceDiagnostic, SourceResult};
use typst::engine::{Route, Sink, Traced};
use typst::foundations::{Context, Dict, StyleChain, Target, TargetElem, Value};
use typst::introspection::Introspector;
use typst::layout::{PageRange, PageRanges};
use typst::routines::SpanMode;
use typst::syntax::{Span, SyntaxMode};
use typst::{Feature, Features, World};
use typst_html::{html, HtmlDocument, HtmlOptions};
use typst_layout::PagedDocument;
use typst_pdf::{pdf, PdfOptions, PdfStandard, PdfStandards, Timestamp};
use typst_svg::{svg, SvgOptions};

use crate::call::{self, Answer, Endpoint, Failure, Outcome};
use crate::diagnostics::{self, Diagnostic};

/// Evict memoized Typst entries older than this many compilations so memory
/// stays bounded in long-running kernel sessions.
const COMEMO_MAX_AGE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Pdf,
    Svg,
    Html,
}

impl OutputFormat {
    fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("pdf") {
            Some(Self::Pdf)
        } else if name.eq_ignore_ascii_case("svg") {
            Some(Self::Svg)
        } else if name.eq_ignore_ascii_case("html") {
            Some(Self::Html)
        } else {
            None
        }
    }

    fn target(self) -> Target {
        match self {
            Self::Pdf | Self::Svg => Target::Paged,
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

pub struct CompileArgs {
    source: String,
    inputs: Expr,
    base_path: PathBuf,
    allow_downloads: bool,
    format: OutputFormat,
    pages: Option<PageRanges>,
    pdf_standards: PdfStandards,
    pdf_tagged: bool,
    pdf_timestamp: Option<Timestamp>,
    world_date: WorldDate,
    features: Features,
    evaluate: Option<EvaluateSpec>,
}

const COMPILE_FIELDS: &[&str] = &[
    "Source",
    "Inputs",
    "BasePath",
    "AllowDownloads",
    "Format",
    "Pages",
    "PDFStandard",
    "PDFTags",
    "CreationTimestamp",
    "Features",
    "Evaluate",
];

fn maybe_offline_hint(diagnostics: &mut Vec<Diagnostic>, allow_downloads: bool) {
    if allow_downloads {
        return;
    }
    if diagnostics
        .iter()
        .any(|d| d.severity == "Error" && d.message.to_lowercase().contains("package"))
    {
        diagnostics.push(diagnostics::warning(
            "Package downloads are disabled; pass \"AllowDownloads\" -> True or set \
             \"Downloads\" -> True with MathemeSetSettings[].",
        ));
    }
}

/// Convert user-supplied Typst inputs. Unlike the link's own arguments, these
/// values come from the document, so unsupported values are reported as
/// warnings instead of failing the whole call.
fn read_inputs(entries: &[Entry<'_>], inputs: &mut Dict, warnings: &mut Vec<String>) {
    for entry in entries {
        let (key, value) = match entry.pair() {
            Ok(pair) => pair,
            Err(reason) => {
                warnings.push(reason);
                continue;
            }
        };
        let Some(name) = key.as_str() else {
            warnings.push(format!("Inputs key {key} is not a String; skipped"));
            continue;
        };
        let converted = to_typst(value, warnings);
        inputs.insert(typst::foundations::Str::from(name), converted);
    }
}

fn positive_page(expr: &Expr, what: &str) -> Result<NonZeroUsize, Failure> {
    let n = call::integer(expr)?;
    if n < 1 {
        return Err(call::error(format!("{what} must be a positive Integer")));
    }
    usize::try_from(n)
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| call::error(format!("{what} out of page range")))
}

fn parse_page_str(spec: &str) -> Result<PageRanges, Failure> {
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(call::error("empty page range component"));
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
                        .map_err(|_| call::error(format!("invalid page number: {start:?}")))?,
                )
            };
            let hi = if end.is_empty() {
                None
            } else {
                Some(
                    end.parse::<NonZeroUsize>()
                        .map_err(|_| call::error(format!("invalid page number: {end:?}")))?,
                )
            };
            if lo.is_none() && hi.is_none() {
                return Err(call::error("page range '-' cannot have both ends empty"));
            }
            lo..=hi
        } else {
            let page = part
                .parse::<NonZeroUsize>()
                .map_err(|_| call::error(format!("invalid page number: {part:?}")))?;
            Some(page)..=Some(page)
        };
        ranges.push(range);
    }
    Ok(PageRanges::new(ranges))
}

fn parse_page_item(expr: &Expr) -> Result<PageRange, Failure> {
    if let Some(n) = expr.as_i64() {
        let page = if n >= 1 {
            usize::try_from(n)
                .ok()
                .and_then(NonZeroUsize::new)
                .ok_or_else(|| call::error("Pages entry out of range"))?
        } else {
            return Err(call::error("Pages entry must be a positive Integer"));
        };
        return Ok(Some(page)..=Some(page));
    }
    if let Some(normal) = expr.as_normal().filter(|n| n.has_head(&Symbol::SPAN)) {
        let elements = normal.elements();
        if elements.len() != 2 {
            return Err(call::error("Span in Pages must have two endpoints"));
        }
        let lo = positive_page(&elements[0], "Pages span start")?;
        let hi = if elements[1].is_symbol(&Symbol::ALL) {
            None
        } else {
            Some(positive_page(&elements[1], "Pages span end")?)
        };
        return Ok(Some(lo)..=hi);
    }
    if let Some(pair) = expr.as_list() {
        if pair.len() != 2 {
            return Err(call::error("Pages range list must have two elements {start, end}"));
        }
        let lo = if pair[0].is_symbol(&Symbol::ALL) || pair[0].is_symbol(&Symbol::NONE) {
            None
        } else {
            Some(positive_page(&pair[0], "Pages range start")?)
        };
        let hi = if pair[1].is_symbol(&Symbol::ALL) || pair[1].is_symbol(&Symbol::NONE) {
            None
        } else {
            Some(positive_page(&pair[1], "Pages range end")?)
        };
        if lo.is_none() && hi.is_none() {
            return Err(call::error("Pages range cannot be unbounded on both ends"));
        }
        return Ok(lo..=hi);
    }
    Err(call::error(
        "Pages entry must be an Integer, {start, end}, or Span",
    ))
}

fn parse_pages(expr: &Expr) -> Result<Option<PageRanges>, Failure> {
    if expr.is_symbol(&Symbol::ALL) || expr.is_symbol(&Symbol::AUTOMATIC) {
        return Ok(None);
    }
    if let Some(spec) = expr.as_str() {
        return parse_page_str(spec).map(Some);
    }
    if expr.as_i64().is_some()
        || expr
            .as_normal()
            .is_some_and(|n| n.has_head(&Symbol::SPAN))
    {
        return Ok(Some(PageRanges::new(vec![parse_page_item(expr)?])));
    }
    if let Some(items) = expr.as_list() {
        let mut ranges = Vec::with_capacity(items.len());
        for item in items {
            ranges.push(parse_page_item(item)?);
        }
        return Ok(Some(PageRanges::new(ranges)));
    }
    Err(call::error(
        "Pages must be All, an Integer, a range String, or a List of pages/ranges",
    ))
}

fn parse_one_pdf_standard(name: &str) -> Option<PdfStandard> {
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

fn parse_pdf_standards(expr: &Expr) -> Result<PdfStandards, Failure> {
    if expr.is_symbol(&Symbol::AUTOMATIC) || expr.is_symbol(&Symbol::NONE) {
        return Ok(PdfStandards::default());
    }
    let names = if let Some(s) = expr.as_str() {
        vec![s.to_owned()]
    } else if let Some(list) = expr.as_list() {
        list.iter()
            .map(call::string)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        return Err(call::error(
            "PDFStandard must be Automatic, a standard String, or a List of Strings",
        ));
    };

    let mut parsed = Vec::with_capacity(names.len());
    for name in &names {
        let std = parse_one_pdf_standard(name)
            .ok_or_else(|| call::error(format!("unknown PDFStandard: {name:?}")))?;
        parsed.push(std);
    }
    PdfStandards::new(&parsed)
        .map_err(|err| call::error(format!("invalid PDFStandard combination: {}", err.message())))
}

fn parse_timestamp(expr: &Expr) -> Result<(WorldDate, Option<Timestamp>), Failure> {
    if expr.is_symbol(&Symbol::AUTOMATIC) {
        return Ok((WorldDate::System, None));
    }
    if expr.is_symbol(&Symbol::NONE) || expr.is_symbol(&Symbol::NULL) {
        return Ok((WorldDate::None, None));
    }
    if let Some(unix) = expr.as_i64() {
        let dt = datetime_from_unix(unix, None)
            .ok_or_else(|| call::error("CreationTimestamp is out of range"))?;
        return Ok((WorldDate::Fixed(unix), Some(Timestamp::new_utc(dt))));
    }
    if let Some(entries) = expr.as_association() {
        let unix = call::integer(call::required(&entries, "Unix")?)?;
        let offset_mins = match call::field(&entries, "OffsetMinutes")? {
            Some(expr) => Some(
                i32::try_from(call::integer(expr)?)
                    .map_err(|_| call::error("OffsetMinutes out of range"))?,
            ),
            None => None,
        };
        let dt = datetime_from_unix(unix, offset_mins)
            .ok_or_else(|| call::error("CreationTimestamp is out of range"))?;
        let ts = match offset_mins {
            Some(mins) => Timestamp::new_local(dt, mins)
                .ok_or_else(|| call::error("OffsetMinutes must be within -23:59..=+23:59"))?,
            None => Timestamp::new_utc(dt),
        };
        return Ok((WorldDate::Fixed(unix), Some(ts)));
    }
    Err(call::error(
        "CreationTimestamp must be Automatic, None, an Integer Unix timestamp, or <|\"Unix\" -> ..., \"OffsetMinutes\" -> ...|>",
    ))
}

fn parse_features(expr: &Expr, format: OutputFormat) -> Result<Features, Failure> {
    let mut selected = Vec::new();
    let list = expr
        .as_list()
        .ok_or_else(|| call::error("Features must be a List of feature strings"))?;
    for item in list {
        let name = call::string(item)?;
        let feature = match name.to_ascii_lowercase().as_str() {
            "html" => Feature::Html,
            "bundle" => Feature::Bundle,
            "a11y-extras" | "a11yextras" => Feature::A11yExtras,
            _ => return Err(call::error(format!("unknown Typst feature: {name:?}"))),
        };
        selected.push(feature);
    }
    if format == OutputFormat::Html && !selected.contains(&Feature::Html) {
        selected.push(Feature::Html);
    }
    Ok(selected.into_iter().collect())
}

fn parse_evaluate(expr: &Expr) -> Result<Option<EvaluateSpec>, Failure> {
    if expr.is_symbol(&Symbol::NONE) || expr.is_symbol(&Symbol::NULL) {
        return Ok(None);
    }
    if let Some(code) = expr.as_str() {
        return Ok(Some(EvaluateSpec::Single(code.to_owned())));
    }
    if let Some(items) = expr.as_list() {
        let codes = items
            .iter()
            .map(call::string)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Some(EvaluateSpec::List(codes)));
    }
    if let Some(entries) = expr.as_association() {
        let mut pairs = Vec::with_capacity(entries.len());
        for entry in &entries {
            let (k, v) = call::pair(entry)?;
            pairs.push((call::key(k)?.to_owned(), call::string(v)?));
        }
        return Ok(Some(EvaluateSpec::Map(pairs)));
    }
    Err(call::error(
        "Evaluate must be None, a code String, a List of code Strings, or an Association of key -> code String",
    ))
}

impl Endpoint for Compile {
    const NAME: &'static str = "compile";
    type Input = CompileArgs;

    fn parse(entries: &[Entry<'_>]) -> Result<Self::Input, Failure> {
        for entry in entries {
            let name = call::key(call::pair(entry)?.0)?;
            if !COMPILE_FIELDS.contains(&name) {
                return Err(call::error(format!("unknown compile key: {name}")));
            }
        }

        let source = call::string(call::required(entries, "Source")?)?;
        let inputs_expr = call::required(entries, "Inputs")?;
        if inputs_expr.as_association().is_none() {
            return Err(call::error("Inputs must be an Association"));
        }
        let inputs = inputs_expr.clone();
        let base = PathBuf::from(call::string(call::required(entries, "BasePath")?)?);
        let base_path = if base.is_absolute() {
            base
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(base)
        };
        let allow_downloads = call::boolean(call::required(entries, "AllowDownloads")?)?;

        let format = match call::field(entries, "Format")? {
            Some(expr) => call::named(expr, OutputFormat::parse, "output format")?,
            None => OutputFormat::Pdf,
        };
        let pages = match call::field(entries, "Pages")? {
            Some(expr) => parse_pages(expr)?,
            None => None,
        };
        let pdf_standards = match call::field(entries, "PDFStandard")? {
            Some(expr) => parse_pdf_standards(expr)?,
            None => PdfStandards::default(),
        };
        let pdf_tagged = match call::field(entries, "PDFTags")? {
            Some(expr) => call::boolean(expr)?,
            None => true,
        };
        let (world_date, pdf_timestamp) = match call::field(entries, "CreationTimestamp")? {
            Some(expr) => parse_timestamp(expr)?,
            None => (WorldDate::System, None),
        };
        let features = match call::field(entries, "Features")? {
            Some(expr) => parse_features(expr, format)?,
            None => parse_features(&Expr::list(Vec::<Expr>::new()), format)?,
        };
        let evaluate = match call::field(entries, "Evaluate")? {
            Some(expr) => parse_evaluate(expr)?,
            None => None,
        };

        Ok(CompileArgs {
            source,
            inputs,
            base_path,
            allow_downloads,
            format,
            pages,
            pdf_standards,
            pdf_tagged,
            pdf_timestamp,
            world_date,
            features,
            evaluate,
        })
    }

    fn invoke(input: Self::Input) -> Answer {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut inputs = Dict::new();
        let mut input_warnings = Vec::new();
        if let Some(input_entries) = input.inputs.as_association() {
            read_inputs(&input_entries, &mut inputs, &mut input_warnings);
        }
        diagnostics.extend(input_warnings.into_iter().map(diagnostics::warning));

        world::set_call_downloads(input.allow_downloads);
        let world = MathemeWorld::with_options(
            input.source,
            inputs,
            input.base_path,
            input.features,
            input.world_date,
        );

        let mut typst_diags: Vec<SourceDiagnostic> = Vec::new();
        let mut outcome_entries: Vec<Rule> = Vec::new();
        let mut eval_result: Option<Expr> = None;

        let compile_failed = match input.format {
            OutputFormat::Pdf | OutputFormat::Svg => {
                let warned = typst::compile::<PagedDocument>(&world);
                record_typst_diags(&world, &warned.warnings, &mut typst_diags, &mut diagnostics);
                match warned.output {
                    Ok(document) => {
                        let total_pages = document.pages().len() as i64;
                        let export_res = match input.format {
                            OutputFormat::Pdf => {
                                let opts = PdfOptions {
                                    standards: input.pdf_standards,
                                    tagged: input.pdf_tagged,
                                    timestamp: input.pdf_timestamp,
                                    page_ranges: input.pages.clone(),
                                    ..PdfOptions::default()
                                };
                                match pdf(&document, &opts) {
                                    Ok(bytes) => {
                                        outcome_entries.push(rule!("PDF" -> Expr::bytes(bytes)));
                                        outcome_entries.push(rule!("Pages" -> total_pages));
                                        Ok(())
                                    }
                                    Err(errs) => Err(errs),
                                }
                            }
                            OutputFormat::Svg => {
                                let svgs = export_svg(&document, input.pages.as_ref());
                                outcome_entries.push(rule!("SVG" -> Expr::list(svgs)));
                                outcome_entries.push(rule!("Pages" -> total_pages));
                                Ok(())
                            }
                            OutputFormat::Html => unreachable!(),
                        };

                        if let Err(errs) = export_res {
                            record_typst_diags(&world, &errs, &mut typst_diags, &mut diagnostics);
                            true
                        } else if let Some(ref spec) = input.evaluate {
                            match evaluate_spec(
                                &world,
                                document.introspector().as_ref(),
                                OutputFormat::Pdf.target(),
                                spec,
                                &mut typst_diags,
                                &mut diagnostics,
                            ) {
                                Ok(expr) => {
                                    eval_result = Some(expr);
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
                            outcome_entries.push(rule!("HTML" -> html_text));
                            if let Some(ref spec) = input.evaluate {
                                match evaluate_spec(
                                    &world,
                                    document.introspector().as_ref(),
                                    OutputFormat::Html.target(),
                                    spec,
                                    &mut typst_diags,
                                    &mut diagnostics,
                                ) {
                                    Ok(expr) => {
                                        eval_result = Some(expr);
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

        if compile_failed {
            maybe_offline_hint(&mut diagnostics, input.allow_downloads);
            let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);
            world::evict_caches(COMEMO_MAX_AGE);

            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == "Error")
                .collect();
            let summary = errors
                .first()
                .map(|diagnostic| diagnostic.message.as_str())
                .unwrap_or("compilation failed");
            return Failure::new(
                "Compile",
                format!(
                    "Typst compilation failed with {} error(s): {summary}",
                    errors.len()
                ),
            )
            .with(diagnostics, Some(formatted))
            .into();
        }

        let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);
        world::evict_caches(COMEMO_MAX_AGE);

        if let Some(evaluated) = eval_result {
            outcome_entries.push(rule!("Evaluated" -> evaluated));
        }
        outcome_entries.push(rule!("Formatted" -> formatted));

        let mut outcome = Outcome::new(outcome_entries);
        outcome.warnings = diagnostics;
        outcome.into()
    }
}

pub struct Compile;

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
) -> Result<Expr, ()> {
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
        Ok(value) => {
            let mut conv_warnings = Vec::new();
            let expr = from_typst(&value, &mut conv_warnings);
            diagnostics.extend(conv_warnings.into_iter().map(diagnostics::warning));
            Ok(expr)
        }
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
) -> Result<Expr, ()> {
    let main_source = match world.source(world.main()) {
        Ok(source) => source,
        Err(err) => {
            diagnostics.push(diagnostics::warning(format!(
                "failed to load main source for evaluation: {err}"
            )));
            return Err(());
        }
    };

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
            Ok(Expr::list(out))
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
                out.push(Rule::new(key.as_str(), val));
            }
            Ok(Expr::association(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args(extra: impl IntoIterator<Item = Rule>) -> Expr {
        let mut rules = vec![
            Rule::new("Source", "= Hello"),
            Rule::new("Inputs", Expr::association(Vec::<Rule>::new())),
            Rule::new("BasePath", "."),
            Rule::new("AllowDownloads", false),
        ];
        rules.extend(extra);
        Expr::association(rules)
    }

    #[test]
    fn parses_general_compile_options() {
        let expr = base_args([
            Rule::new("Format", "SVG"),
            Rule::new("Pages", "1,3-5"),
            Rule::new("PDFStandard", Expr::list(["a-2b", "ua-1"])),
            Rule::new("PDFTags", false),
            Rule::new("CreationTimestamp", 1_795_003_200_i64),
            Rule::new("Features", Expr::list(["a11y-extras"])),
            Rule::new("Evaluate", Expr::list(["1 + 2", "x"])),
        ]);
        let parsed = Compile::parse(&expr.as_association().unwrap()).expect("valid compile args");

        assert_eq!(parsed.format, OutputFormat::Svg);
        assert!(!parsed.pdf_tagged);
        assert_eq!(parsed.world_date, WorldDate::Fixed(1_795_003_200));
        assert!(parsed.features.is_enabled(Feature::A11yExtras));
        assert_eq!(
            parsed.evaluate,
            Some(EvaluateSpec::List(vec!["1 + 2".into(), "x".into()]))
        );
        let ranges = parsed.pages.expect("page ranges");
        assert!(ranges.includes_page(NonZeroUsize::new(1).unwrap()));
        assert!(!ranges.includes_page(NonZeroUsize::new(2).unwrap()));
        assert!(ranges.includes_page(NonZeroUsize::new(4).unwrap()));
    }

    #[test]
    fn incompatible_pdf_standards_fail_cleanly() {
        let expr = base_args([Rule::new(
            "PDFStandard",
            Expr::list(["1.4", "2.0"]),
        )]);
        let err = Compile::parse(&expr.as_association().unwrap())
            .err()
            .expect("conflicting standards");
        assert_eq!(err.tag, "Call");
    }

    #[test]
    fn html_format_enables_html_feature_automatically() {
        let expr = base_args([Rule::new("Format", "HTML")]);
        let parsed = Compile::parse(&expr.as_association().unwrap()).unwrap();
        assert_eq!(parsed.format, OutputFormat::Html);
        assert!(parsed.features.is_enabled(Feature::Html));
    }
}
