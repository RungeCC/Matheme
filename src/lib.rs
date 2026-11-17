mod diagnostics;
mod wolfram;

use std::path::PathBuf;

use matheme_world as world;

use typst::diag::SourceDiagnostic;
use typst::foundations::Dict;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use wolfram_library_link::{
    export,
    expr::{Expr as RawExpr, ExprKind, Symbol},
    wstp::Link,
};

use matheme_wl::{to_typst_value, Expr, Rule};
use world::MathemeWorld;

/// Read the WSTP argument list, resolving Wolfram 15's context-free symbol
/// names (e.g. `List`, `True`) into their `System\`` forms.
fn read_args(link: &mut Link) -> Vec<Expr> {
    let expr: Expr = link
        .get_expr_with_resolver(&mut |name: &str| {
            let full = format!("System`{}", name);
            Some(Symbol::new(&full))
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

#[export(wstp)]
fn matheme_compile(link: &mut Link) {
    let args = read_args(link);
    assert_eq!(args.len(), 1, "matheme_compile expects a single Association");
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
        let value = entry.value();
        match entry.key().as_str().unwrap_or("") {
            "Source" => match value.as_str() {
                Some(s) => source = Some(s.to_owned()),
                None => diagnostics.push(diagnostics::warning("Source must be a String")),
            },
            "Inputs" => match value.as_association() {
                Some(rules) => {
                    let mut extra_warnings = Vec::new();
                    for rule in rules {
                        match rule.key().as_str() {
                            Some(name) => {
                                let input = to_typst_value(rule.value(), &mut extra_warnings);
                                inputs.insert(typst::foundations::Str::from(name), input);
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

fn package_entries(status: &world::PackageStatus) -> Vec<Rule> {
    let path_or_null = |value: &Option<String>| match value {
        Some(path) => Expr::string(path.as_str()),
        None => Expr::null(),
    };
    vec![
        Rule::new("Packages", status.count as i64),
        Rule::new("PackageData", path_or_null(&status.data)),
        Rule::new("PackageCache", path_or_null(&status.cache)),
        Rule::new("DownloadsAllowed", Expr::boolean(status.downloads)),
    ]
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
        match wolfram::installation_error(link) {
            Some(err) => Expr::string(err),
            None => Expr::null(),
        },
    ));
    entries.extend(package_entries(&world::package_status()));
    let result = Expr::association(entries);
    link.put_expr(result.raw()).expect("failed to write version");
}

/// Eagerly initialize the process-wide caches (fonts, package loader and,
/// optionally, the Universe index) without compiling a document. Accepts an
/// optional Association with keys "Fonts", "Packages", "PackageIndex" (all
/// booleans, defaulting to True) so callers can warm only what they need.
#[export(wstp)]
fn matheme_warmup(link: &mut Link) {
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
                    let key = option.key().as_str().unwrap_or("");
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
                    match option.value().as_bool() {
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

    let mut entries = vec![Rule::new("Status", "Success")];

    if fonts {
        let start = std::time::Instant::now();
        let count = world::warmup_fonts();
        entries.push(Rule::new("Fonts", count as i64));
        entries.push(Rule::new("FontsMs", start.elapsed().as_millis() as i64));
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
    ]);
    link.put_expr(result.raw())
        .expect("failed to write reload result");
}

#[export(wstp)]
fn matheme_reload_packages(link: &mut Link) {
    let args = read_args(link);
    assert!(args.is_empty(), "matheme_reload_packages takes no arguments");

    let status = world::reload_packages();
    let mut entries = vec![Rule::new("Status", "Success")];
    entries.extend(package_entries(&status));
    let result = Expr::association(entries);
    link.put_expr(result.raw())
        .expect("failed to write package reload result");
}
