mod diagnostics;
mod wl;

use std::path::PathBuf;

use matheme_world as world;

use typst::diag::SourceDiagnostic;
use typst::foundations::Dict;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use wolfram_library_link::{export, expr::ExprKind, wstp::Link};

use world::MathemeWorld;

fn maybe_offline_hint(diagnostics: &mut Vec<diagnostics::Diagnostic>, allow_downloads: bool) {
    if allow_downloads {
        return;
    }
    if diagnostics
        .iter()
        .any(|d| d.severity == "Error" && d.message.to_lowercase().contains("package"))
    {
        diagnostics.push(diagnostics::warning(
            "Package downloads are disabled; pass \"AllowDownloads\" -> True to use Typst Universe.",
        ));
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
        Some(bytes) => wolfram_library_link::expr::Expr::new(ExprKind::ByteArray(bytes)),
        None => wl::null(),
    };

    let result = wl::assoc(vec![
        wl::rule("Status", wl::string(status)),
        wl::rule("PDF", pdf_expr),
        wl::rule("Pages", wl::integer(pages)),
        wl::rule(
            "Diagnostics",
            wl::list(diagnostics.iter().map(diagnostics::expr).collect()),
        ),
        wl::rule("Formatted", wl::string(formatted)),
    ]);

    link.put_expr(&result)
        .expect("failed to write result expression to WSTP link");
}

#[export(wstp)]
fn matheme_compile(link: &mut Link) {
    let args = wl::read_args(link);
    assert_eq!(args.len(), 1, "matheme_compile expects a single Association");

    let entries = match wl::as_assoc(&args[0]) {
        Some(entries) => entries,
        None => {
            let debug = diagnostics::warning(format!(
                "expected an Association argument, got: {}",
                args[0]
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
        let key = wl::as_string(&entry.key).unwrap_or("");
        match key {
            "Source" => match wl::as_string(&entry.value) {
                Some(s) => source = Some(s.to_owned()),
                None => diagnostics.push(diagnostics::warning("Source must be a String")),
            },
            "Inputs" => match wl::as_assoc(&entry.value) {
                Some(rules) => {
                    let mut extra_warnings = Vec::new();
                    for rule in rules {
                        match wl::as_string(&rule.key) {
                            Some(name) => {
                                let value = wl::to_typst_value(&rule.value, &mut extra_warnings);
                                inputs.insert(typst::foundations::Str::from(name), value);
                            }
                            None => {
                                extra_warnings.push("Inputs key is not a String; skipped".into())
                            }
                        }
                    }
                    diagnostics.extend(extra_warnings.into_iter().map(diagnostics::warning));
                }
                None => diagnostics.push(diagnostics::warning("Inputs must be an Association")),
            },
            "BasePath" => match wl::as_string(&entry.value) {
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
            "AllowDownloads" => match wl::as_bool(&entry.value) {
                Some(value) => allow_downloads = value,
                None => diagnostics
                    .push(diagnostics::warning("AllowDownloads must be True or False")),
            },
            other => diagnostics.push(diagnostics::warning(format!("unknown input key: {}", other))),
        }
    }

    let source = match source {
        Some(s) => s,
        None => {
            put_result(link, "Error", None, 0, &diagnostics, "Missing Source\n");
            return;
        }
    };

    let base_path = base_path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    world::set_downloads_allowed(allow_downloads);
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

fn package_entries(
    status: &world::PackageStatus,
) -> Vec<wolfram_library_link::expr::RuleEntry> {
    let path_or_null = |value: &Option<String>| match value {
        Some(path) => wl::string(path.as_str()),
        None => wl::null(),
    };
    vec![
        wl::rule("Packages", wl::integer(status.count as i64)),
        wl::rule("PackageData", path_or_null(&status.data)),
        wl::rule("PackageCache", path_or_null(&status.cache)),
        wl::rule(
            "DownloadsAllowed",
            wl::symbol(if status.downloads { "System`True" } else { "System`False" }),
        ),
    ]
}

#[export(wstp)]
fn matheme_version(link: &mut Link) {
    let args = wl::read_args(link);
    assert!(args.is_empty(), "matheme_version takes no arguments");

    let mut entries = vec![
        wl::rule("Status", wl::string("Success")),
        wl::rule("Matheme", wl::string(env!("CARGO_PKG_VERSION"))),
        wl::rule("Typst", wl::string("0.15.1")),
    ];
    entries.extend(package_entries(&world::package_status()));
    let result = wl::assoc(entries);
    link.put_expr(&result).expect("failed to write version");
}

/// Eagerly initialize the process-wide caches (fonts, package loader and,
/// optionally, the Universe index) without compiling a document. Accepts an
/// optional Association with keys "Fonts", "Packages", "PackageIndex" (all
/// booleans, defaulting to True) so callers can warm only what they need.
#[export(wstp)]
fn matheme_warmup(link: &mut Link) {
    let args = wl::read_args(link);
    assert!(
        args.is_empty() || args.len() == 1,
        "matheme_warmup takes no arguments or one Association"
    );

    let mut fonts = true;
    let mut packages = true;
    let mut package_index = true;
    let mut diag = Vec::new();

    if let Some(arg) = args.first() {
        match wl::as_assoc(arg) {
            Some(entries) => {
                for entry in entries {
                    let key = wl::as_string(&entry.key).unwrap_or("");
                    let target = match key {
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
                    match wl::as_bool(&entry.value) {
                        Some(value) => *target = value,
                        None => diag.push(diagnostics::warning(format!(
                            "warmup option {key} must be True or False"
                        ))),
                    }
                }
            }
            None => diag.push(diagnostics::warning(
                "warmup options must be an Association; using defaults",
            )),
        }
    }

    let mut entries = vec![wl::rule("Status", wl::string("Success"))];

    if fonts {
        let start = std::time::Instant::now();
        let count = world::warmup_fonts();
        entries.push(wl::rule("Fonts", wl::integer(count as i64)));
        entries.push(wl::rule(
            "FontsMs",
            wl::integer(start.elapsed().as_millis() as i64),
        ));
    }

    if packages {
        let start = std::time::Instant::now();
        let status = world::warmup_packages();
        entries.extend(package_entries(&status));
        entries.push(wl::rule(
            "PackagesMs",
            wl::integer(start.elapsed().as_millis() as i64),
        ));
    }

    if package_index {
        let start = std::time::Instant::now();
        match world::prefetch_universe_index() {
            Ok(version) => entries.push(wl::rule("PackageIndex", wl::string(version))),
            Err(err) => {
                entries.push(wl::rule("PackageIndex", wl::null()));
                diag.push(diagnostics::warning(format!(
                    "failed to prefetch Universe index: {err}"
                )));
            }
        }
        entries.push(wl::rule(
            "PackageIndexMs",
            wl::integer(start.elapsed().as_millis() as i64),
        ));
    }

    if !diag.is_empty() {
        entries.push(wl::rule(
            "Diagnostics",
            wl::list(diag.iter().map(diagnostics::expr).collect()),
        ));
    }

    let result = wl::assoc(entries);
    link.put_expr(&result).expect("failed to write warmup result");
}

#[export(wstp)]
fn matheme_reload_fonts(link: &mut Link) {
    let args = wl::read_args(link);
    assert!(args.is_empty(), "matheme_reload_fonts takes no arguments");

    let count = world::reload_fonts();
    let result = wl::assoc(vec![
        wl::rule("Status", wl::string("Success")),
        wl::rule("Fonts", wl::integer(count as i64)),
    ]);
    link.put_expr(&result).expect("failed to write reload result");
}

#[export(wstp)]
fn matheme_reload_packages(link: &mut Link) {
    let args = wl::read_args(link);
    assert!(args.is_empty(), "matheme_reload_packages takes no arguments");

    let status = world::reload_packages();
    let mut entries = vec![wl::rule("Status", wl::string("Success"))];
    entries.extend(package_entries(&status));
    let result = wl::assoc(entries);
    link.put_expr(&result)
        .expect("failed to write package reload result");
}
