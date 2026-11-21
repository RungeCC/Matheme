//! Wolfram Language adapter for `MathemeCompile`.
//!
//! All Typst compilation, export, evaluation, and diagnostic formatting live in
//! [`matheme_world::compile`]; this module only decodes WL arguments via
//! [`wl_args!`] / [`FromExpr`] and marshals results back into WL expressions.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use matheme_wl::{from_typst, rule, to_typst, Entry, Expr, Rule, Symbol};
use matheme_world::compile::{
    self, CompileFailure, CompileOutput, CompileRequest, EvaluateSpec, EvaluatedValues, Feature,
    OutputFormat, PageRange, PageRanges, PagedCompileRequest, PdfStandards, Timestamp,
};
use matheme_world::{datetime_from_unix, WorldDate};
use typst::foundations::Dict;

use crate::args::{self, AssocExpr, FromArgs, FromExpr};
use crate::call::{Answer, Failure, Outcome};
use crate::diagnostics;
use crate::wl_args;

// ---------------------------------------------------------------------------
// `FromExpr` value parsers for `compile` options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompileFormat {
    #[default]
    Pdf,
    Svg,
    Html,
    Graphics,
}

impl CompileFormat {
    pub fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("graphics") || name.eq_ignore_ascii_case("vector") {
            Some(Self::Graphics)
        } else {
            OutputFormat::parse(name).map(Self::from)
        }
    }
}

impl From<OutputFormat> for CompileFormat {
    fn from(fmt: OutputFormat) -> Self {
        match fmt {
            OutputFormat::Pdf => Self::Pdf,
            OutputFormat::Svg => Self::Svg,
            OutputFormat::Html => Self::Html,
        }
    }
}

impl FromExpr for CompileFormat {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        args::named(expr, Self::parse, "output format")
    }
}

#[derive(Debug, Clone, Default)]
pub struct PagesInput(pub Option<PageRanges>);

fn parse_page_item(expr: &Expr) -> Result<PageRange, String> {
    if expr.as_i64().is_some() {
        let page = NonZeroUsize::from_expr(expr)?;
        return Ok(Some(page)..=Some(page));
    }
    if let Some(normal) = expr.as_normal().filter(|n| n.has_head(&Symbol::SPAN)) {
        let elements = normal.elements();
        if elements.len() != 2 {
            return Err("Span in Pages must have two endpoints".into());
        }
        let lo = NonZeroUsize::from_expr(&elements[0])?;
        let hi = if elements[1].is_symbol(&Symbol::ALL) {
            None
        } else {
            Some(NonZeroUsize::from_expr(&elements[1])?)
        };
        return Ok(Some(lo)..=hi);
    }
    if let Some(pair) = expr.as_list() {
        if pair.len() != 2 {
            return Err("Pages range list must have two elements {start, end}".into());
        }
        let lo = if pair[0].is_symbol(&Symbol::ALL) || pair[0].is_symbol(&Symbol::NONE) {
            None
        } else {
            Some(NonZeroUsize::from_expr(&pair[0])?)
        };
        let hi = if pair[1].is_symbol(&Symbol::ALL) || pair[1].is_symbol(&Symbol::NONE) {
            None
        } else {
            Some(NonZeroUsize::from_expr(&pair[1])?)
        };
        if lo.is_none() && hi.is_none() {
            return Err("Pages range cannot be unbounded on both ends".into());
        }
        return Ok(lo..=hi);
    }
    Err("Pages entry must be an Integer, {start, end}, or Span".into())
}

impl FromExpr for PagesInput {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::ALL) || expr.is_symbol(&Symbol::AUTOMATIC) {
            return Ok(Self(None));
        }
        if let Some(spec) = expr.as_str() {
            return compile::parse_page_str(spec).map(|r| Self(Some(r)));
        }
        if expr.as_i64().is_some()
            || expr
                .as_normal()
                .is_some_and(|n| n.has_head(&Symbol::SPAN))
        {
            return Ok(Self(Some(PageRanges::new(vec![parse_page_item(expr)?]))));
        }
        if let Some(items) = expr.as_list() {
            let ranges = items
                .iter()
                .map(parse_page_item)
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Self(Some(PageRanges::new(ranges))));
        }
        Err("must be All, an Integer, a range String, or a List of pages/ranges".into())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PdfStandardsInput(pub PdfStandards);

impl FromExpr for PdfStandardsInput {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::AUTOMATIC) || expr.is_symbol(&Symbol::NONE) {
            return Ok(Self(PdfStandards::default()));
        }
        let names = if let Some(s) = expr.as_str() {
            vec![s.to_owned()]
        } else if let Some(list) = expr.as_list() {
            list.iter()
                .map(String::from_expr)
                .collect::<Result<Vec<_>, _>>()?
        } else {
            return Err("must be Automatic, a standard String, or a List of Strings".into());
        };
        compile::parse_pdf_standards(&names).map(Self)
    }
}

wl_args! {
    #[action = "timestamp"]
    struct TimestampAssocArgs {
        "Unix" => unix: i64,
        "OffsetMinutes" => offset_minutes: Option<i64> = None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct TimestampInput {
    pub world_date: WorldDate,
    pub pdf_timestamp: Option<Timestamp>,
}

impl FromExpr for TimestampInput {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::AUTOMATIC) {
            return Ok(Self {
                world_date: WorldDate::System,
                pdf_timestamp: None,
            });
        }
        if expr.is_symbol(&Symbol::NONE) || expr.is_symbol(&Symbol::NULL) {
            return Ok(Self {
                world_date: WorldDate::None,
                pdf_timestamp: None,
            });
        }
        if let Some(unix) = expr.as_i64() {
            let dt = datetime_from_unix(unix, None)
                .ok_or_else(|| "timestamp is out of range".to_owned())?;
            return Ok(Self {
                world_date: WorldDate::Fixed(unix),
                pdf_timestamp: Some(Timestamp::new_utc(dt)),
            });
        }
        if let Some(entries) = expr.as_association() {
            let parsed = TimestampAssocArgs::parse_args(&entries).map_err(|f| f.message)?;
            let offset_mins = match parsed.offset_minutes {
                Some(raw) => Some(
                    i32::try_from(raw).map_err(|_| "OffsetMinutes out of range".to_owned())?,
                ),
                None => None,
            };
            let dt = datetime_from_unix(parsed.unix, offset_mins)
                .ok_or_else(|| "timestamp is out of range".to_owned())?;
            let ts = match offset_mins {
                Some(mins) => Timestamp::new_local(dt, mins)
                    .ok_or_else(|| "OffsetMinutes must be within -23:59..=+23:59".to_owned())?,
                None => Timestamp::new_utc(dt),
            };
            return Ok(Self {
                world_date: WorldDate::Fixed(parsed.unix),
                pdf_timestamp: Some(ts),
            });
        }
        Err(
            "must be Automatic, None, an Integer Unix timestamp, or <|\"Unix\" -> ..., \"OffsetMinutes\" -> ...|>"
                .into(),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct FeaturesInput(pub Vec<Feature>);

impl FromExpr for FeaturesInput {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        let names = Vec::<String>::from_expr(expr)
            .map_err(|_| "must be a List of feature strings".to_owned())?;
        compile::parse_features(&names).map(Self)
    }
}

#[derive(Debug, Clone, Default)]
pub struct EvaluateInput(pub Option<EvaluateSpec>);

impl FromExpr for EvaluateInput {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::NONE) || expr.is_symbol(&Symbol::NULL) {
            return Ok(Self(None));
        }
        if let Some(code) = expr.as_str() {
            return Ok(Self(Some(EvaluateSpec::Single(code.to_owned()))));
        }
        if let Some(items) = expr.as_list() {
            let codes = items
                .iter()
                .map(String::from_expr)
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Self(Some(EvaluateSpec::List(codes))));
        }
        if let Some(entries) = expr.as_association() {
            let mut pairs = Vec::with_capacity(entries.len());
            for entry in &entries {
                let (k, v) = entry.pair()?;
                let key = k
                    .as_str()
                    .ok_or_else(|| format!("association key must be a String, got: {k}"))?;
                pairs.push((key.to_owned(), String::from_expr(v)?));
            }
            return Ok(Self(Some(EvaluateSpec::Map(pairs))));
        }
        Err(
            "must be None, a code String, a List of code Strings, or an Association of key -> code String"
                .into(),
        )
    }
}

// ---------------------------------------------------------------------------
// Declarative `compile` arguments schema
// ---------------------------------------------------------------------------

wl_args! {
    #[action = "compile"]
    pub struct CompileArgs {
        "Source" => pub source: String,
        "Inputs" => pub inputs: AssocExpr,
        "BasePath" => pub base_path: PathBuf,
        "AllowDownloads" => pub allow_downloads: bool,
        "Format" => pub format: CompileFormat = CompileFormat::Pdf,
        "Pages" => pub pages: PagesInput = PagesInput::default(),
        "PDFStandard" => pub pdf_standards: PdfStandardsInput = PdfStandardsInput::default(),
        "PDFTags" => pub pdf_tagged: bool = true,
        "CreationTimestamp" => pub timestamp: TimestampInput = TimestampInput::default(),
        "Features" => pub features: FeaturesInput = FeaturesInput::default(),
        "Evaluate" => pub evaluate: EvaluateInput = EvaluateInput::default(),
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

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

fn evaluated_to_expr(values: &EvaluatedValues, warnings: &mut Vec<String>) -> Expr {
    match values {
        EvaluatedValues::Single(val) => from_typst(val, warnings),
        EvaluatedValues::List(vals) => {
            Expr::list(vals.iter().map(|val| from_typst(val, warnings)))
        }
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
    mut warnings: Vec<diagnostics::Diagnostic>,
    formatted: String,
) -> Answer {
    if let Some(ref evaluated) = evaluated {
        let mut conv_warnings = Vec::new();
        let expr = evaluated_to_expr(evaluated, &mut conv_warnings);
        warnings.extend(conv_warnings.into_iter().map(diagnostics::warning));
        outcome_entries.push(rule!("Evaluated" -> expr));
    }
    outcome_entries.push(rule!("Formatted" -> formatted));

    let mut outcome = Outcome::new(outcome_entries);
    outcome.warnings = warnings;
    outcome.into()
}

fn finish_failure(failure: CompileFailure) -> Answer {
    Failure::new("Compile", failure.display_message())
        .with(failure.diagnostics, Some(failure.formatted))
        .into()
}

pub fn run(input: CompileArgs) -> Answer {
    let mut inputs = Dict::new();
    let mut input_warnings = Vec::new();
    if let Some(input_entries) = input.inputs.0.as_association() {
        read_inputs(&input_entries, &mut inputs, &mut input_warnings);
    }
    let extra_diagnostics = input_warnings
        .into_iter()
        .map(diagnostics::warning)
        .collect();

    if input.format == CompileFormat::Graphics {
        let paged_request = PagedCompileRequest {
            source: input.source,
            inputs,
            base_path: input.base_path,
            allow_downloads: input.allow_downloads,
            pages: input.pages.0,
            world_date: input.timestamp.world_date,
            features: input.features.0,
            evaluate: input.evaluate.0,
            extra_diagnostics,
        };
        return match compile::compile_paged_with(paged_request, |doc, pages| {
            let export =
                matheme_graphics::export_graphics(doc, pages, matheme_world::font_generation());
            let warnings = export
                .warnings
                .into_iter()
                .map(|w| diagnostics::warning(w.message()))
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
        pages: input.pages.0,
        pdf_standards: input.pdf_standards.0,
        pdf_tagged: input.pdf_tagged,
        pdf_timestamp: input.timestamp.pdf_timestamp,
        world_date: input.timestamp.world_date,
        features: input.features.0,
        evaluate: input.evaluate.0,
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
        let parsed = CompileArgs::parse_args(&expr.as_association().unwrap())
            .expect("valid compile args");

        assert_eq!(parsed.format, CompileFormat::Svg);
        assert!(!parsed.pdf_tagged);
        assert_eq!(parsed.timestamp.world_date, WorldDate::Fixed(1_795_003_200));
        assert!(parsed.features.0.contains(&Feature::A11yExtras));
        assert_eq!(
            parsed.evaluate.0,
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
        assert!(err.message.contains("unknown compile key: Bogus"), "{}", err.message);

        let dup = base_args([Rule::new("Format", "PDF"), Rule::new("Format", "SVG")]);
        let err = CompileArgs::parse_args(&dup.as_association().unwrap())
            .err()
            .expect("duplicate key");
        assert!(err.message.contains("duplicate compile key: Format"), "{}", err.message);
    }
}
