//! The link functions' declared inputs and their implementations.
//!
//! Each endpoint owns a small `Args` struct: that is the model of the WSTP
//! arguments it expects. [`crate::call::serve`] handles everything around it,
//! so no exported function repeats argument reading, host configuration,
//! status rendering or link writing.

use std::path::PathBuf;

use matheme_wl::{to_typst_value, Entry, Expr, Rule};
use typst::diag::SourceDiagnostic;
use typst::foundations::Dict;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use wolfram_library_link::expr::{Expr as RawExpr, ExprKind};

use matheme_world as world;
use world::MathemeWorld;

use crate::call::{self, Answer, Endpoint, Failure, Outcome};
use crate::diagnostics::{self, Diagnostic};
use crate::{query, settings};

fn string_or_null(value: Option<&str>) -> Expr {
    match value {
        Some(value) => Expr::string(value),
        None => Expr::null(),
    }
}

fn package_entries(status: &world::PackageStatus) -> Vec<Rule> {
    vec![
        Rule::new("Packages", status.count as i64),
        Rule::new("PackageData", string_or_null(status.data.as_deref())),
        Rule::new("PackageCache", string_or_null(status.cache.as_deref())),
        Rule::new("DownloadsAllowed", status.downloads),
        Rule::new("DownloadsActive", status.downloads_active),
        Rule::new("Proxy", string_or_null(status.proxy.as_deref())),
        Rule::new("ProxyError", string_or_null(status.proxy_error.as_deref())),
    ]
}

/// Font counts, optionally forcing a scan for introspection calls.
fn font_counts_entry(scan: bool) -> Rule {
    let counts = if scan {
        Some(world::font_counts())
    } else {
        world::cached_counts()
    };
    Rule::new(
        "FontCounts",
        counts
            .map(|counts| settings::font_counts_expr(&counts))
            .unwrap_or_else(Expr::null),
    )
}

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
             MathemeSettings[<|\"Downloads\" -> True|>].",
        ));
    }
}

/// Convert user-supplied Typst inputs. Unlike the link's own arguments, these
/// values come from the document, so unsupported values are reported as
/// warnings (the Wolfram layer turns them into messages) instead of failing
/// the whole call.
fn read_inputs(entries: &[Entry], inputs: &mut Dict, warnings: &mut Vec<String>) {
    for entry in entries {
        let (key, value) = match entry.pair() {
            Ok(pair) => pair,
            Err(reason) => {
                warnings.push(reason);
                continue;
            }
        };
        let Some(name) = key.as_str() else {
            warnings.push(format!("Inputs key {} is not a String; skipped", key.raw()));
            continue;
        };
        let converted = to_typst_value(value, warnings);
        inputs.insert(typst::foundations::Str::from(name), converted);
    }
}

// ---------------------------------------------------------------------------
// matheme_compile
// ---------------------------------------------------------------------------

pub struct Compile;

/// The single Association produced by `MathemeCompile`/`MathemeToGraphics`.
pub struct CompileArgs {
    source: String,
    inputs: Vec<Entry>,
    base_path: PathBuf,
    allow_downloads: bool,
}

const COMPILE_FIELDS: &[&str] = &["Source", "Inputs", "BasePath", "AllowDownloads"];

impl Endpoint for Compile {
    const NAME: &'static str = "matheme_compile";
    type Input = CompileArgs;

    fn parse(args: Vec<Expr>) -> Self::Input {
        let entries = call::one_association(args);
        for entry in &entries {
            let name = call::key(call::pair(entry).0);
            assert!(
                COMPILE_FIELDS.contains(&name),
                "unknown compile key: {name}"
            );
        }

        let source = call::string(call::required(&entries, "Source"));
        let inputs = call::required(&entries, "Inputs")
            .as_association()
            .expect("Inputs must be an Association");
        let base = PathBuf::from(call::string(call::required(&entries, "BasePath")));
        let base_path = if base.is_absolute() {
            base
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(base)
        };
        let allow_downloads = call::boolean(call::required(&entries, "AllowDownloads"));

        CompileArgs {
            source,
            inputs,
            base_path,
            allow_downloads,
        }
    }

    fn invoke(input: Self::Input) -> Answer {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut inputs = Dict::new();
        let mut warnings = Vec::new();
        read_inputs(&input.inputs, &mut inputs, &mut warnings);
        diagnostics.extend(warnings.into_iter().map(diagnostics::warning));

        world::set_call_downloads(input.allow_downloads);
        let world = MathemeWorld::new(input.source, inputs, input.base_path);
        let warned = typst::compile::<PagedDocument>(&world);

        let mut typst_diags: Vec<SourceDiagnostic> = warned.warnings.iter().cloned().collect();
        diagnostics.extend(
            warned
                .warnings
                .iter()
                .map(|d| diagnostics::from_typst(d, &world)),
        );

        let (pdf, pages, errors): (Option<Vec<u8>>, i64, Vec<SourceDiagnostic>) =
            match warned.output {
                Ok(document) => {
                    let pages = document.pages().len() as i64;
                    match typst_pdf::pdf(&document, &PdfOptions::default()) {
                        Ok(bytes) => (Some(bytes), pages, Vec::new()),
                        Err(errors) => (None, pages, errors.iter().cloned().collect()),
                    }
                }
                Err(errors) => (None, 0, errors.iter().cloned().collect()),
            };

        typst_diags.extend(errors.iter().cloned());
        diagnostics.extend(errors.iter().map(|d| diagnostics::from_typst(d, &world)));

        let failed = pdf.is_none();
        if failed {
            maybe_offline_hint(&mut diagnostics, input.allow_downloads);
        }
        let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);

        if failed {
            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == "Error")
                .collect();
            let summary = errors
                .first()
                .map(|diagnostic| diagnostic.message.as_str())
                .unwrap_or("PDF generation failed");
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

        let pdf = Expr::from_raw(RawExpr::new(ExprKind::ByteArray(
            pdf.expect("success implies a PDF"),
        )));
        let mut outcome = Outcome::new([
            Rule::new("PDF", pdf),
            Rule::new("Pages", pages),
            Rule::new("Formatted", formatted),
        ]);
        outcome.warnings = diagnostics;
        outcome.into()
    }
}

// ---------------------------------------------------------------------------
// matheme_version
// ---------------------------------------------------------------------------

pub struct Version;

impl Endpoint for Version {
    const NAME: &'static str = "matheme_version";
    const HOST_FONTS: bool = false;
    type Input = ();

    fn parse(args: Vec<Expr>) {
        call::no_args(args);
    }

    fn invoke((): ()) -> Answer {
        // Typst itself derives this from its build-time TYPST_VERSION env var
        // (see typst_utils::version), so bumping the pinned dependency can
        // never leave a stale hard-coded number here.
        let version = typst::utils::version();
        Outcome::new([
            Rule::new("Matheme", env!("CARGO_PKG_VERSION")),
            Rule::new("Typst", version.raw()),
            Rule::new(
                "TypstCommit",
                version
                    .commit()
                    .map(Expr::string)
                    .unwrap_or_else(Expr::null),
            ),
        ])
        .into()
    }
}

// ---------------------------------------------------------------------------
// matheme_settings
// ---------------------------------------------------------------------------

pub struct Settings;

/// No argument reports the effective settings; one Association is a patch.
type SettingsArgs = Option<Vec<Entry>>;

impl Endpoint for Settings {
    const NAME: &'static str = "matheme_settings";
    type Input = SettingsArgs;

    fn parse(args: Vec<Expr>) -> Self::Input {
        let entries = call::optional_association(args);
        (!entries.is_empty()).then_some(entries)
    }

    fn invoke(input: Self::Input) -> Answer {
        let applied = input.map(|entries| world::apply(&settings::parse(&entries)));
        let mut entries = Vec::new();

        if let Some(applied) = &applied {
            entries.push(Rule::new(
                "Changed",
                Expr::association([
                    Rule::new("Fonts", applied.changes.fonts),
                    Rule::new("Packages", applied.changes.packages),
                    Rule::new("Downloads", applied.changes.downloads),
                ]),
            ));
            entries.push(Rule::new("FontsReloaded", applied.fonts_reloaded));
            entries.push(Rule::new("PackagesRebuilt", applied.packages_rebuilt));
        }

        let effective = applied
            .map(|applied| applied.settings)
            .unwrap_or_else(world::settings);
        entries.push(Rule::new("Settings", settings::expr(&effective)));
        entries.push(font_counts_entry(false));
        entries.extend(package_entries(&world::package_status()));
        Outcome::new(entries).into()
    }
}

// ---------------------------------------------------------------------------
// matheme_warmup
// ---------------------------------------------------------------------------

pub struct Warmup;

pub struct WarmupArgs {
    fonts: bool,
    packages: bool,
    package_index: bool,
}

impl Endpoint for Warmup {
    const NAME: &'static str = "matheme_warmup";
    type Input = WarmupArgs;

    fn parse(args: Vec<Expr>) -> Self::Input {
        let mut input = WarmupArgs {
            fonts: true,
            packages: true,
            package_index: true,
        };
        for entry in call::optional_association(args) {
            let (key, value) = call::pair(&entry);
            let enabled = call::boolean(value);
            match call::key(key) {
                "Fonts" => input.fonts = enabled,
                "Packages" => input.packages = enabled,
                "PackageIndex" => input.package_index = enabled,
                other => panic!("unknown warmup key: {other}"),
            }
        }
        input
    }

    fn invoke(input: Self::Input) -> Answer {
        let mut outcome = Outcome::new(Vec::new());

        if input.fonts {
            let start = std::time::Instant::now();
            let count = world::warmup_fonts();
            outcome.extend([
                Rule::new("Fonts", count as i64),
                Rule::new("FontsMs", start.elapsed().as_millis() as i64),
                font_counts_entry(false),
            ]);
        }

        if input.packages {
            let start = std::time::Instant::now();
            let status = world::warmup_packages();
            outcome.extend(package_entries(&status));
            outcome.extend([Rule::new("PackagesMs", start.elapsed().as_millis() as i64)]);
        }

        if input.package_index {
            let start = std::time::Instant::now();
            match world::prefetch_universe_index() {
                Ok(version) => outcome.extend([Rule::new("PackageIndex", version)]),
                Err(err) => {
                    outcome.extend([Rule::new("PackageIndex", Expr::null())]);
                    outcome.warn(format!("failed to prefetch Universe index: {err}"));
                }
            }
            outcome.extend([Rule::new(
                "PackageIndexMs",
                start.elapsed().as_millis() as i64,
            )]);
        }

        outcome.into()
    }
}

// ---------------------------------------------------------------------------
// matheme_reload_fonts / matheme_reload_packages
// ---------------------------------------------------------------------------

pub struct ReloadFonts;

impl Endpoint for ReloadFonts {
    const NAME: &'static str = "matheme_reload_fonts";
    type Input = ();

    fn parse(args: Vec<Expr>) {
        call::no_args(args);
    }

    fn invoke((): ()) -> Answer {
        let count = world::reload_fonts();
        Outcome::new([Rule::new("Fonts", count as i64), font_counts_entry(false)]).into()
    }
}

pub struct ReloadPackages;

impl Endpoint for ReloadPackages {
    const NAME: &'static str = "matheme_reload_packages";
    const HOST_FONTS: bool = false;
    type Input = ();

    fn parse(args: Vec<Expr>) {
        call::no_args(args);
    }

    fn invoke((): ()) -> Answer {
        Outcome::new(package_entries(&world::reload_packages())).into()
    }
}

// ---------------------------------------------------------------------------
// matheme_query_fonts / matheme_query_packages
// ---------------------------------------------------------------------------

pub struct QueryFonts;

impl Endpoint for QueryFonts {
    const NAME: &'static str = "matheme_query_fonts";
    type Input = query::FontQuery;

    fn parse(args: Vec<Expr>) -> Self::Input {
        query::FontQuery::parse(args)
    }

    fn invoke(input: Self::Input) -> Answer {
        query::run_fonts(&input).into()
    }
}

pub struct QueryPackages;

impl Endpoint for QueryPackages {
    const NAME: &'static str = "matheme_query_packages";
    const HOST_FONTS: bool = false;
    type Input = query::PackageQuery;

    fn parse(args: Vec<Expr>) -> Self::Input {
        query::PackageQuery::parse(args)
    }

    fn invoke(input: Self::Input) -> Answer {
        query::run_packages(&input).into()
    }
}

// ---------------------------------------------------------------------------
// matheme_close
// ---------------------------------------------------------------------------

pub struct Close;

impl Endpoint for Close {
    const NAME: &'static str = "matheme_close";
    const HOST_FONTS: bool = false;
    type Input = ();

    fn parse(args: Vec<Expr>) {
        call::no_args(args);
    }

    fn invoke((): ()) -> Answer {
        world::reset_caches();
        Outcome::new([Rule::new(
            "Released",
            Expr::list(["Fonts", "Packages", "Memoization"]),
        )])
        .into()
    }
}
