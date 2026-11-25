//! Wolfram Language adapter for `MathemeCompile`.
//!
//! All Typst compilation, export, evaluation, and diagnostic formatting live in
//! [`matheme_world::compile`]; this module only decodes WL arguments via
//! [`wl_args!`] and marshals results back into WL expressions.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use matheme_wl::decode::{self, Decoder};
use matheme_wl::{Expr, Rule, Symbol, from_typst, rule, to_typst};
use matheme_world::compile::{
    self, CompileFailure, CompileOutput, CompileRequest, EvaluateSpec, EvaluatedValues, Feature,
    OutputFormat, PageRange, PageRanges, PagedCompileRequest, PdfStandards, Timestamp,
};
use matheme_world::{WorldDate, datetime_from_unix};
use serde::Deserialize;
use typst::foundations::Value;

use crate::args::{FromArgs, schema};
use crate::call::{Answer, Failure, Outcome};
use crate::diagnostics::{self, DiagnosticSource};
use crate::wl_args;

// ---------------------------------------------------------------------------
// Value decoders for `compile` options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompileFormat {
    #[default]
    Pdf,
    Svg,
    Html,
    #[serde(alias = "vector")]
    Graphics,
}

fn parse_page_item(expr: &Expr) -> Result<PageRange, String> {
    if expr.as_i64().is_some() {
        let page = decode::positive().decode(expr)?;
        return Ok(Some(page)..=Some(page));
    }
    if let Some(normal) = expr.as_normal().filter(|n| n.has_head(&Symbol::SPAN)) {
        let elements = normal.elements();
        if elements.len() != 2 {
            return Err("Span in Pages must have two endpoints".into());
        }
        let lo = decode::positive().decode(&elements[0])?;
        let hi = if elements[1].is_symbol(&Symbol::ALL) {
            None
        } else {
            Some(decode::positive().decode(&elements[1])?)
        };
        return checked_page_range(Some(lo), hi);
    }
    if let Some(pair) = expr.as_list() {
        if pair.len() != 2 {
            return Err("Pages range list must have two elements {start, end}".into());
        }
        let lo = if pair[0].is_symbol(&Symbol::ALL) || pair[0].is_symbol(&Symbol::NONE) {
            None
        } else {
            Some(decode::positive().decode(&pair[0])?)
        };
        let hi = if pair[1].is_symbol(&Symbol::ALL) || pair[1].is_symbol(&Symbol::NONE) {
            None
        } else {
            Some(decode::positive().decode(&pair[1])?)
        };
        if lo.is_none() && hi.is_none() {
            return Err("Pages range cannot be unbounded on both ends".into());
        }
        return checked_page_range(lo, hi);
    }
    Err("Pages entry must be an Integer, {start, end}, or Span".into())
}

fn checked_page_range(
    lo: Option<NonZeroUsize>,
    hi: Option<NonZeroUsize>,
) -> Result<PageRange, String> {
    if lo.zip(hi).is_some_and(|(start, end)| start > end) {
        return Err("Pages range start must be at most its end".into());
    }
    Ok(lo..=hi)
}

fn pages() -> impl Decoder<Value = Option<PageRanges>> {
    let one_page = |range| Some(PageRanges::new(vec![range]));
    decode::symbol(Symbol::ALL)
        .or(decode::symbol(Symbol::AUTOMATIC))
        .map(|()| None)
        .or(decode::string().try_map(|spec| compile::parse_page_str(&spec).map(Some)))
        .or(decode::positive().map(move |page| one_page(Some(page)..=Some(page))))
        .or(decode::head(Symbol::SPAN).try_map(move |expr| parse_page_item(&expr).map(one_page)))
        .or(decode::head(Symbol::LIST).try_map(|expr| {
            let items = expr.as_list().expect("list decoder matched");
            if items.is_empty() {
                return Err("Pages list must not be empty".into());
            }
            let ranges = items
                .iter()
                .map(parse_page_item)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(PageRanges::new(ranges)))
        }))
}

#[derive(Debug, Clone, Default)]
pub struct PdfStandardsInput {
    standards: PdfStandards,
    specified: bool,
    requires_tags: bool,
}

fn pdf_standards() -> impl Decoder<Value = PdfStandardsInput> {
    let standard = || schema::enumeration(compile::parse_pdf_standard);
    decode::symbol(Symbol::AUTOMATIC)
        .or(decode::symbol(Symbol::NONE))
        .map(|()| PdfStandardsInput::default())
        .or(standard()
            .map(|value| vec![value])
            .or(decode::list(standard()))
            .try_map(|standards| {
                Ok(PdfStandardsInput {
                    standards: compile::validate_pdf_standards(&standards)?,
                    specified: !standards.is_empty(),
                    requires_tags: compile::pdf_standards_require_tags(&standards),
                })
            }))
}

wl_args! {
    #[action = "timestamp"]
    struct TimestampAssocArgs {
        "Unix" => unix: i64 [decode::integer()],
        "OffsetMinutes" => offset_minutes: Option<i64> [decode::integer().map(Some)] = None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct TimestampInput {
    pub world_date: WorldDate,
    pub pdf_timestamp: Option<Timestamp>,
}

fn fixed_timestamp(
    unix_seconds: i64,
    offset_minutes: Option<i32>,
) -> Result<TimestampInput, String> {
    let datetime = datetime_from_unix(unix_seconds, offset_minutes)
        .ok_or_else(|| "timestamp is out of range".to_owned())?;
    let (world_date, pdf_timestamp) = match offset_minutes {
        Some(offset_minutes) => (
            WorldDate::FixedLocal {
                unix_seconds,
                offset_minutes,
            },
            Timestamp::new_local(datetime, offset_minutes)
                .ok_or_else(|| "OffsetMinutes must be within -23:59..=+23:59".to_owned())?,
        ),
        None => (WorldDate::Fixed(unix_seconds), Timestamp::new_utc(datetime)),
    };
    Ok(TimestampInput {
        world_date,
        pdf_timestamp: Some(pdf_timestamp),
    })
}

fn timestamp() -> impl Decoder<Value = TimestampInput> {
    decode::symbol(Symbol::AUTOMATIC)
        .map(|()| TimestampInput::default())
        .or(decode::symbol(Symbol::NONE)
            .or(decode::symbol(Symbol::NULL))
            .map(|()| TimestampInput {
                world_date: WorldDate::None,
                pdf_timestamp: None,
            }))
        .or(decode::integer().try_map(|unix| fixed_timestamp(unix, None)))
        .or(decode::association().try_map(|expr| {
            let entries = expr.as_association().expect("association decoder matched");
            let parsed =
                TimestampAssocArgs::parse_args(&entries).map_err(|failure| failure.message)?;
            let offset = parsed
                .offset_minutes
                .map(i32::try_from)
                .transpose()
                .map_err(|_| "OffsetMinutes out of range".to_owned())?;
            fixed_timestamp(parsed.unix, offset)
        }))
}

fn features() -> impl Decoder<Value = Vec<Feature>> {
    decode::list(schema::enumeration_with(
        compile::parse_feature,
        |deserializer| compile::deserialize_feature(deserializer),
    ))
}

fn evaluate() -> impl Decoder<Value = Option<EvaluateSpec>> {
    decode::symbol(Symbol::NONE)
        .or(decode::symbol(Symbol::NULL))
        .map(|()| None)
        .or(decode::string().map(|code| Some(EvaluateSpec::Single(code))))
        .or(decode::list(decode::string()).map(|codes| Some(EvaluateSpec::List(codes))))
        .or(decode::association().try_map(|expr| {
            let entries = expr.as_association().expect("association decoder matched");
            let pairs = entries
                .iter()
                .map(|entry| {
                    let (key, value) = entry.pair()?;
                    let key = key
                        .as_str()
                        .ok_or_else(|| format!("association key must be a String, got: {key}"))?;
                    Ok((key.to_owned(), decode::string().decode(value)?))
                })
                .collect::<Result<_, String>>()?;
            Ok(Some(EvaluateSpec::Map(pairs)))
        }))
}

// ---------------------------------------------------------------------------
// Declarative `compile` arguments schema
// ---------------------------------------------------------------------------

wl_args! {
    #[action = "compile"]
    pub struct CompileArgs {
        "Source" => pub source: String [decode::string()],
        "Inputs" => pub inputs: Expr [decode::association()],
        "BasePath" => pub base_path: PathBuf [decode::path()],
        "AllowDownloads" => pub allow_downloads: bool [decode::boolean()],
        "Format" => pub format: CompileFormat [schema::enumeration(|name| schema::parse_name::<CompileFormat>(name, "output format"))] = CompileFormat::Pdf,
        "Pages" => pub pages: Option<PageRanges> [self::pages()] = None,
        "PDFStandard" => pub pdf_standards: PdfStandardsInput [self::pdf_standards()] = PdfStandardsInput::default(),
        "PDFTags" => pub pdf_tagged: bool [decode::boolean()] = true,
        "CreationTimestamp" => pub timestamp: TimestampInput [self::timestamp()] = TimestampInput::default(),
        "Features" => pub features: Vec<Feature> [self::features()] = Vec::new(),
        "Evaluate" => pub evaluate: Option<EvaluateSpec> [self::evaluate()] = None,
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

fn evaluated_to_expr(values: &EvaluatedValues, warnings: &mut Vec<String>) -> Expr {
    match values {
        EvaluatedValues::Single(val) => from_typst(val, warnings),
        EvaluatedValues::List(vals) => Expr::list(vals.iter().map(|val| from_typst(val, warnings))),
        EvaluatedValues::Map(pairs) => Expr::association(
            pairs
                .iter()
                .map(|(k, v)| Rule::new(k.as_str(), from_typst(v, warnings))),
        ),
    }
}

fn finish_success(
    mut outcome_entries: Vec<Rule>,
    evaluated: Option<EvaluatedValues>,
    mut diags: Vec<diagnostics::Diagnostic>,
    mut formatted: String,
) -> Answer {
    if let Some(ref evaluated) = evaluated {
        let mut conv_warnings = Vec::new();
        let expr = evaluated_to_expr(evaluated, &mut conv_warnings);
        for warning in conv_warnings {
            let diag = diagnostics::warning(DiagnosticSource::Conversion, warning);
            formatted.push_str(&diagnostics::format_line(&diag));
            diags.push(diag);
        }
        outcome_entries.push(rule!("Evaluated" -> expr));
    }

    Outcome::new(outcome_entries)
        .with_diagnostics(diags, Some(formatted))
        .into()
}

fn finish_failure(failure: CompileFailure) -> Answer {
    Failure::compile(failure.display_message())
        .with_diagnostics(failure.diagnostics, Some(failure.formatted))
        .into()
}

pub fn run(input: CompileArgs) -> Answer {
    if input.format == CompileFormat::Html && input.pages.is_some() {
        return Failure::call("\"Pages\" filtering is not supported for HTML output.").into();
    }
    if input.format == CompileFormat::Pdf && input.pages.is_some() && input.pdf_tagged {
        return Failure::call("PDF page filtering requires \"PDFTags\" -> False.").into();
    }
    if input.format != CompileFormat::Pdf && (input.pdf_standards.specified || !input.pdf_tagged) {
        return Failure::call("\"PDFStandard\" and \"PDFTags\" apply only to PDF output.").into();
    }
    if input.pdf_standards.requires_tags && !input.pdf_tagged {
        return Failure::call("The selected PDF standard requires \"PDFTags\" -> True.").into();
    }

    let mut input_warnings = Vec::new();
    let Value::Dict(inputs) = to_typst(&input.inputs, &mut input_warnings) else {
        return Failure::internal("Inputs association did not convert to a Typst dictionary")
            .into();
    };
    let extra_diagnostics = input_warnings
        .into_iter()
        .map(|msg| {
            let separator = if msg.starts_with('[') { "" } else { ": " };
            diagnostics::warning(
                DiagnosticSource::Conversion,
                format!("Inputs{separator}{msg}"),
            )
        })
        .collect();

    if input.format == CompileFormat::Graphics {
        let paged_request = PagedCompileRequest {
            source: input.source,
            inputs,
            base_path: input.base_path,
            allow_downloads: input.allow_downloads,
            pages: input.pages,
            world_date: input.timestamp.world_date,
            features: input.features,
            evaluate: input.evaluate,
            extra_diagnostics,
        };
        return match compile::compile_paged_with(paged_request, |doc, pages| {
            let export =
                matheme_graphics::export_graphics(doc, pages, matheme_world::font_generation());
            let warnings = export
                .warnings
                .into_iter()
                .map(|w| diagnostics::warning(DiagnosticSource::Graphics, w.message()))
                .collect();
            Ok((export.graphics, warnings))
        }) {
            Ok(success) => finish_success(
                vec![
                    rule!("Graphics" -> success.output),
                    rule!("Pages" -> success.exported_pages as i64),
                    rule!("TotalPages" -> success.total_pages as i64),
                ],
                success.evaluated,
                success.diagnostics,
                success.formatted,
            ),
            Err(failure) => finish_failure(failure),
        };
    }

    let format = match input.format {
        CompileFormat::Pdf => OutputFormat::Pdf,
        CompileFormat::Svg => OutputFormat::Svg,
        CompileFormat::Html => OutputFormat::Html,
        CompileFormat::Graphics => unreachable!(),
    };

    let request = CompileRequest {
        source: input.source,
        inputs,
        base_path: input.base_path,
        allow_downloads: input.allow_downloads,
        format,
        pages: input.pages,
        pdf_standards: input.pdf_standards.standards,
        pdf_tagged: input.pdf_tagged,
        pdf_timestamp: input.timestamp.pdf_timestamp,
        world_date: input.timestamp.world_date,
        features: input.features,
        evaluate: input.evaluate,
        extra_diagnostics,
    };

    match compile::compile(request) {
        Ok(success) => {
            let outcome_entries = match success.output {
                CompileOutput::Pdf {
                    bytes,
                    pages,
                    total_pages,
                } => vec![
                    rule!("PDF" -> Expr::bytes(bytes)),
                    rule!("Pages" -> pages as i64),
                    rule!("TotalPages" -> total_pages as i64),
                ],
                CompileOutput::Svg { pages, total_pages } => {
                    let exported_pages = pages.len();
                    vec![
                        rule!("SVG" -> Expr::list(pages)),
                        rule!("Pages" -> exported_pages as i64),
                        rule!("TotalPages" -> total_pages as i64),
                    ]
                }
                CompileOutput::Html { html } => vec![rule!("HTML" -> html)],
            };
            finish_success(
                outcome_entries,
                success.evaluated,
                success.diagnostics,
                success.formatted,
            )
        }
        Err(failure) => finish_failure(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::FromArgs;

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
        let parsed =
            CompileArgs::parse_args(&expr.as_association().unwrap()).expect("valid compile args");

        assert_eq!(parsed.format, CompileFormat::Svg);
        assert!(!parsed.pdf_tagged);
        assert_eq!(parsed.timestamp.world_date, WorldDate::Fixed(1_795_003_200));
        assert!(parsed.features.contains(&Feature::A11yExtras));
        assert_eq!(
            parsed.evaluate,
            Some(EvaluateSpec::List(vec!["1 + 2".into(), "x".into()]))
        );

        for fmt_name in ["Graphics", "Vector", "graphics", "vector"] {
            let g_expr = base_args([Rule::new("Format", fmt_name)]);
            let g_parsed = CompileArgs::parse_args(&g_expr.as_association().unwrap())
                .expect("graphics format should parse");
            assert_eq!(g_parsed.format, CompileFormat::Graphics);
        }
    }

    #[test]
    fn rejects_unknown_and_duplicate_compile_keys() {
        let unknown = base_args([Rule::new("Bogus", 1_i64)]);
        let err = CompileArgs::parse_args(&unknown.as_association().unwrap())
            .err()
            .expect("unknown key");
        assert!(
            err.message.contains("unknown compile key: Bogus"),
            "{}",
            err.message
        );

        let dup = base_args([Rule::new("Format", "PDF"), Rule::new("Format", "SVG")]);
        let err = CompileArgs::parse_args(&dup.as_association().unwrap())
            .err()
            .expect("duplicate key");
        assert!(
            err.message.contains("duplicate compile key: Format"),
            "{}",
            err.message
        );
    }

    #[test]
    fn html_with_pages_returns_call_failure() {
        let expr = base_args([Rule::new("Format", "HTML"), Rule::new("Pages", "1")]);
        let parsed = CompileArgs::parse_args(&expr.as_association().unwrap()).expect("parsed");
        match run(parsed) {
            Answer::Failure(failure) => {
                assert_eq!(failure.kind, crate::call::ErrorKind::Call);
                assert!(failure.message.contains("HTML"), "{}", failure.message);
                assert!(failure.report.is_none());
            }
            Answer::Success(_) => panic!("HTML with Pages must return Failure"),
        }
    }
}
