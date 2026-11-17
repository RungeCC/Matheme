mod diagnostics;
mod query;
mod settings;
mod wolfram;

use std::path::PathBuf;

use matheme_world as world;

use typst::diag::SourceDiagnostic;
use typst::foundations::Dict;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use wolfram_library_link::{
    export,
    expr::{Expr as RawExpr, ExprKind, Symbol as RawSymbol},
    wstp::Link,
};

use matheme_wl::{to_typst_value, Entry, Expr, Rule};
use world::MathemeWorld;

/// Read the WSTP argument list, resolving Wolfram 15's context-free symbol
/// names (e.g. `List`, `True`) into their `System\`` forms.
fn read_args(link: &mut Link) -> Vec<Expr> {
    let expr: Expr = link
        .get_expr_with_resolver(&mut |name: &str| {
            let full = format!("System`{}", name);
            Some(RawSymbol::new(&full))
        })
        .expect("failed to read WSTP arguments")
        .into();

    expr.as_list().expect("expected an argument list")
}

fn maybe_offline_hint(diagnostics: &mut Vec<diagnostics::Diagnostic>, allow_downloads: bool) {
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

fn string_or_null(value: Option<&str>) -> Expr {
    match value {
        Some(value) => Expr::string(value),
        None => Expr::null(),
    }
}

fn put_result(
    link: &mut Link,
    status: &str,
    pdf: Option<Vec<u8>>,
    pages: i64,
    diagnostics: &[diagnostics::Diagnostic],
    formatted: &str,
) {
    let pdf_expr = match pdf {
        Some(bytes) => Expr::from_raw(RawExpr::new(ExprKind::ByteArray(bytes))),
        None => Expr::null(),
    };

    let result = Expr::association([
        Rule::new("Status", status),
        Rule::new("PDF", pdf_expr),
        Rule::new("Pages", pages),
        Rule::new(
            "Diagnostics",
            Expr::list(diagnostics.iter().map(diagnostics::expr)),
        ),
        Rule::new("Formatted", formatted),
    ]);

    link.put_expr(result.raw())
        .expect("failed to write result expression to WSTP link");
}

/// Read `Inputs`, reporting every entry that cannot be converted.
fn read_inputs(entry: &Entry, inputs: &mut Dict, diagnostics: &mut Vec<diagnostics::Diagnostic>) {
    let (_, value) = match entry.pair() {
        Ok(pair) => pair,
        Err(reason) => {
            diagnostics.push(diagnostics::warning(reason));
            return;
        }
    };
    let Some(entries) = value.as_association() else {
        diagnostics.push(diagnostics::warning("Inputs must be an Association"));
        return;
    };

    let mut warnings = Vec::new();
    for entry in entries {
        let (key, value) = match entry.pair() {
            Ok(pair) => pair,
            Err(reason) => {
                warnings.push(reason);
                continue;
            }
        };
        let Some(name) = key.as_str() else {
            warnings.push("Inputs key is not a String; skipped".into());
            continue;
        };
        let converted = to_typst_value(value, &mut warnings);
        inputs.insert(typst::foundations::Str::from(name), converted);
    }
    diagnostics.extend(warnings.into_iter().map(diagnostics::warning));
}

#[export(wstp)]
fn matheme_compile(link: &mut Link) {
    // A per-call opt-in never outlives the call that set it.
    world::set_call_downloads(false);
    let args = read_args(link);
    assert_eq!(
        args.len(),
        1,
        "matheme_compile expects a single Association"
    );
    wolfram::configure_world_fonts(link);

    let entries = match args[0].as_association() {
        Some(entries) => entries,
        None => {
            let debug = diagnostics::warning(format!(
                "expected an Association argument, got: {}",
                args[0].raw()
            ));
            put_result(link, "Error", None, 0, &[debug], "Invalid argument\n");
            return;
        }
    };

    let mut source: Option<String> = None;
    let mut inputs = Dict::new();
    let mut base_path: Option<PathBuf> = None;
    let mut allow_downloads = false;
    let mut diagnostics: Vec<diagnostics::Diagnostic> = Vec::new();

    for entry in entries {
        let (key, value) = match entry.pair() {
            Ok(pair) => pair,
            Err(reason) => {
                diagnostics.push(diagnostics::warning(reason));
                continue;
            }
        };
        let Some(name) = key.as_str() else {
            diagnostics.push(diagnostics::warning(format!(
                "input key {} is not a String; entry skipped",
                key.raw()
            )));
            continue;
        };
        match name {
            "Source" => match value.as_str() {
                Some(s) => source = Some(s.to_owned()),
                None => diagnostics.push(diagnostics::warning("Source must be a String")),
            },
            "Inputs" => read_inputs(&entry, &mut inputs, &mut diagnostics),
            "BasePath" => match value.as_str() {
                Some(path) => {
                    let path = PathBuf::from(path);
                    base_path = Some(if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir()
                            .unwrap_or_else(|_| PathBuf::from("."))
                            .join(path)
                    });
                }
                None => diagnostics.push(diagnostics::warning("BasePath must be a String")),
            },
            "AllowDownloads" => match value.as_bool() {
                Some(enabled) => allow_downloads = enabled,
                None => {
                    diagnostics.push(diagnostics::warning("AllowDownloads must be True or False"))
                }
            },
            other => diagnostics.push(diagnostics::warning(format!(
                "unknown input key: {}",
                other
            ))),
        }
    }

    let source = match source {
        Some(s) => s,
        None => {
            put_result(link, "Error", None, 0, &diagnostics, "Missing Source\n");
            return;
        }
    };

    let base_path =
        base_path.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    world::set_call_downloads(allow_downloads);
    let world = MathemeWorld::new(source, inputs, base_path);
    let warned = typst::compile::<PagedDocument>(&world);

    let mut typst_diags: Vec<SourceDiagnostic> = warned.warnings.iter().cloned().collect();
    diagnostics.extend(
        warned
            .warnings
            .iter()
            .map(|d| diagnostics::from_typst(d, &world)),
    );

    let (status, pdf, pages) = match warned.output {
        Ok(document) => {
            let pages = document.pages().len() as i64;
            match typst_pdf::pdf(&document, &PdfOptions::default()) {
                Ok(bytes) => ("Success", Some(bytes), pages),
                Err(errors) => {
                    typst_diags.extend(errors.iter().cloned());
                    diagnostics.extend(errors.iter().map(|d| diagnostics::from_typst(d, &world)));
                    maybe_offline_hint(&mut diagnostics, allow_downloads);
                    ("Error", None, pages)
                }
            }
        }
        Err(errors) => {
            typst_diags.extend(errors.iter().cloned());
            diagnostics.extend(errors.iter().map(|d| diagnostics::from_typst(d, &world)));
            maybe_offline_hint(&mut diagnostics, allow_downloads);
            ("Error", None, 0)
        }
    };

    let formatted = diagnostics::format(&world, &typst_diags, &diagnostics);
    put_result(link, status, pdf, pages, &diagnostics, &formatted);
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

#[export(wstp)]
fn matheme_version(link: &mut Link) {
    let args = read_args(link);
    assert!(args.is_empty(), "matheme_version takes no arguments");
    wolfram::configure_world_fonts(link);

    // Typst itself derives this from its build-time TYPST_VERSION env var
    // (see typst_utils::version), so bumping the pinned dependency can never
    // leave a stale hard-coded number here.
    let typst_version = typst::utils::version();
    let mut entries = vec![
        Rule::new("Status", "Success"),
        Rule::new("Matheme", env!("CARGO_PKG_VERSION")),
        Rule::new("Typst", typst_version.raw()),
        Rule::new(
            "TypstCommit",
            typst_version
                .commit()
                .map(Expr::string)
                .unwrap_or_else(Expr::null),
        ),
    ];
    let font_dirs = wolfram::font_dirs(link);
    entries.push(Rule::new(
        "WolframFontDirs",
        Expr::list(font_dirs.iter().map(|path| path.display().to_string())),
    ));
    entries.push(Rule::new(
        "WolframFontFiles",
        wolfram::font_file_count(&font_dirs) as i64,
    ));
    entries.push(Rule::new(
        "WolframFontError",
        string_or_null(wolfram::installation_error(link)),
    ));
    entries.push(Rule::new("Settings", settings::expr(&world::settings())));
    entries.push(font_counts_entry(true));
    entries.extend(package_entries(&world::package_status()));
    let result = Expr::association(entries);
    link.put_expr(result.raw())
        .expect("failed to write version");
}

/// Read or patch the process-wide settings.
///
/// `MathemeSettings[]` reports the effective configuration; passing an
/// Association applies a patch and reports what changed. `Null` values reset a
/// field, `"Reset" -> True` restores every default.
#[export(wstp)]
fn matheme_settings(link: &mut Link) {
    let args = read_args(link);
    assert!(
        args.is_empty() || args.len() == 1,
        "matheme_settings takes no arguments or one Association"
    );
    wolfram::configure_world_fonts(link);

    let mut warnings = Vec::new();
    let mut applied = None;

    if let Some(arg) = args.first() {
        match settings::parse(arg) {
            Ok(parsed) => {
                warnings.extend(parsed.warnings);
                applied = Some(world::apply(&parsed.patch));
            }
            Err(reason) => {
                let result =
                    Expr::association([Rule::new("Status", "Error"), Rule::new("Message", reason)]);
                link.put_expr(result.raw())
                    .expect("failed to write settings error");
                return;
            }
        }
    }

    let mut entries = vec![Rule::new("Status", "Success")];
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

    if !warnings.is_empty() {
        entries.push(Rule::new(
            "Diagnostics",
            Expr::list(
                warnings
                    .into_iter()
                    .map(|message| diagnostics::expr(&diagnostics::warning(message))),
            ),
        ));
    }

    let result = Expr::association(entries);
    link.put_expr(result.raw())
        .expect("failed to write settings result");
}

/// Eagerly initialize the process-wide caches (fonts, package loader and,
/// optionally, the Universe index) without compiling a document. Accepts an
/// optional Association with keys "Fonts", "Packages", "PackageIndex" (all
/// booleans, defaulting to True) so callers can warm only what they need.
#[export(wstp)]
fn matheme_warmup(link: &mut Link) {
    world::set_call_downloads(false);
    let args = read_args(link);
    assert!(
        args.is_empty() || args.len() == 1,
        "matheme_warmup takes no arguments or one Association"
    );
    wolfram::configure_world_fonts(link);

    let mut fonts = true;
    let mut packages = true;
    let mut package_index = true;
    let mut diag = Vec::new();

    if let Some(arg) = args.first() {
        match arg.as_association() {
            Some(options) => {
                for option in options {
                    let (key, value) = match option.pair() {
                        Ok(pair) => pair,
                        Err(reason) => {
                            diag.push(diagnostics::warning(reason));
                            continue;
                        }
                    };
                    let name = key.as_str().unwrap_or("");
                    let target = match name {
                        "Fonts" => &mut fonts,
                        "Packages" => &mut packages,
                        "PackageIndex" => &mut package_index,
                        other => {
                            diag.push(diagnostics::warning(format!(
                                "unknown warmup option: {other}"
                            )));
                            continue;
                        }
                    };
                    match value.as_bool() {
                        Some(value) => *target = value,
                        None => diag.push(diagnostics::warning(format!(
                            "warmup option {name} must be True or False"
                        ))),
                    }
                }
            }
            None => diag.push(diagnostics::warning(
                "warmup options must be an Association; using defaults",
            )),
        }
    }

    let mut entries = vec![Rule::new("Status", "Success")];

    if fonts {
        let start = std::time::Instant::now();
        let count = world::warmup_fonts();
        entries.push(Rule::new("Fonts", count as i64));
        entries.push(Rule::new("FontsMs", start.elapsed().as_millis() as i64));
        entries.push(font_counts_entry(false));
    }

    if packages {
        let start = std::time::Instant::now();
        let status = world::warmup_packages();
        entries.extend(package_entries(&status));
        entries.push(Rule::new("PackagesMs", start.elapsed().as_millis() as i64));
    }

    if package_index {
        let start = std::time::Instant::now();
        match world::prefetch_universe_index() {
            Ok(version) => entries.push(Rule::new("PackageIndex", version)),
            Err(err) => {
                entries.push(Rule::new("PackageIndex", Expr::null()));
                diag.push(diagnostics::warning(format!(
                    "failed to prefetch Universe index: {err}"
                )));
            }
        }
        entries.push(Rule::new(
            "PackageIndexMs",
            start.elapsed().as_millis() as i64,
        ));
    }

    if !diag.is_empty() {
        entries.push(Rule::new(
            "Diagnostics",
            Expr::list(diag.iter().map(diagnostics::expr)),
        ));
    }

    let result = Expr::association(entries);
    link.put_expr(result.raw())
        .expect("failed to write warmup result");
}

#[export(wstp)]
fn matheme_reload_fonts(link: &mut Link) {
    let args = read_args(link);
    assert!(args.is_empty(), "matheme_reload_fonts takes no arguments");
    wolfram::configure_world_fonts(link);

    let count = world::reload_fonts();
    let result = Expr::association([
        Rule::new("Status", "Success"),
        Rule::new("Fonts", count as i64),
        font_counts_entry(false),
    ]);
    link.put_expr(result.raw())
        .expect("failed to write reload result");
}

#[export(wstp)]
fn matheme_reload_packages(link: &mut Link) {
    let args = read_args(link);
    assert!(
        args.is_empty(),
        "matheme_reload_packages takes no arguments"
    );

    let status = world::reload_packages();
    let mut entries = vec![Rule::new("Status", "Success")];
    entries.extend(package_entries(&status));
    let result = Expr::association(entries);
    link.put_expr(result.raw())
        .expect("failed to write package reload result");
}

/// Read-only queries over the metadata the process-wide caches hold: font
/// book entries (family, variant, source, residency) and package entries
/// (local installs plus the Universe index once it has been fetched).
///
/// Call shapes: `MathemeQuery[]` (cache overview), `MathemeQuery["Fonts"]`,
/// `MathemeQuery["Packages"]`, `MathemeQuery[association]` and
/// `MathemeQuery[target, filters]`. A query never loads font bytes or reaches
/// the network unless `AllowDownloads -> True` is given explicitly.
#[export(wstp)]
fn matheme_query(link: &mut Link) {
    // A per-call opt-in never outlives the call that set it.
    world::set_call_downloads(false);
    let args = read_args(link);
    wolfram::configure_world_fonts(link);

    let result = match query::parse(&args) {
        Ok(parsed) => query::run(&parsed),
        Err(reason) => query::error(&reason),
    };
    link.put_expr(result.raw())
        .expect("failed to write query result");
}

/// Release the memory held by the process-wide caches: the font book with its
/// lazily loaded faces, the package loader and typst's memoized compilation
/// results. The next call transparently rebuilds what it needs, so this is
/// safe to call at any time.
#[export(wstp)]
fn matheme_close(link: &mut Link) {
    let args = read_args(link);
    assert!(args.is_empty(), "matheme_close takes no arguments");

    world::reset_caches();
    let result = Expr::association([
        Rule::new("Status", "Success"),
        Rule::new(
            "Released",
            Expr::list(["Fonts", "Packages", "Memoization"]),
        ),
    ]);
    link.put_expr(result.raw())
        .expect("failed to write close result");
}
