mod wl;
mod world;

use typst::diag::{Severity, SourceDiagnostic};
use typst::foundations::Dict;
use typst::World;
use typst::WorldExt;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use wolfram_library_link::{export, expr::ExprKind, wstp::Link};

use world::MathemeWorld;

#[derive(Debug)]
struct Diagnostic {
    severity: &'static str,
    message: String,
    file: Option<String>,
    line: Option<i64>,
    column: Option<i64>,
    range: Option<(usize, usize)>,
    hints: Vec<String>,
}

fn diagnostic_from(diag: &SourceDiagnostic, w: &MathemeWorld) -> Diagnostic {
    let range = w.range(diag.span);
    let source = diag.span.id().and_then(|id| w.source(id).ok());
    let line_column = source.and_then(|src| {
        range
            .clone()
            .and_then(|r| src.lines().byte_to_line_column(r.start))
    });

    Diagnostic {
        severity: match diag.severity {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
        },
        message: diag.message.to_string(),
        file: diag
            .span
            .id()
            .map(|id| format!("{:?}", id).trim_matches('"').to_string()),
        line: line_column.map(|(line, _)| line as i64 + 1),
        column: line_column.map(|(_, column)| column as i64 + 1),
        range: range.map(|r| (r.start, r.end)),
        hints: diag.hints.iter().map(|h| h.v.to_string()).collect(),
    }
}

fn warning(message: String) -> Diagnostic {
    Diagnostic {
        severity: "Warning",
        message,
        file: None,
        line: None,
        column: None,
        range: None,
        hints: vec![],
    }
}

fn diagnostic_expr(diag: &Diagnostic) -> wolfram_library_link::expr::Expr {
    let file = diag
        .file
        .as_ref()
        .map(|s| wl::string(s.as_str()))
        .unwrap_or_else(wl::null);
    let line = diag.line.map(wl::integer).unwrap_or_else(wl::null);
    let column = diag.column.map(wl::integer).unwrap_or_else(wl::null);
    let range = diag
        .range
        .map(|(a, b)| wl::list(vec![wl::integer(a as i64), wl::integer(b as i64)]))
        .unwrap_or_else(wl::null);

    wl::assoc(vec![
        wl::rule("Severity", wl::string(diag.severity)),
        wl::rule("Message", wl::string(diag.message.as_str())),
        wl::rule("File", file),
        wl::rule("Line", line),
        wl::rule("Column", column),
        wl::rule("Range", range),
        wl::rule(
            "Hints",
            wl::list(diag.hints.iter().map(|h| wl::string(h.as_str())).collect()),
        ),
    ])
}

fn put_result(
    link: &mut Link,
    status: &str,
    pdf: Option<Vec<u8>>,
    pages: i64,
    diagnostics: &[Diagnostic],
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
            wl::list(diagnostics.iter().map(diagnostic_expr).collect()),
        ),
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
            let debug = warning(format!("expected an Association argument, got: {}", args[0]));
            put_result(link, "Error", None, 0, &[debug]);
            return;
        }
    };

    let mut source: Option<String> = None;
    let mut inputs = Dict::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    for entry in entries {
        let key = wl::as_string(&entry.key).unwrap_or("");
        match key {
            "Source" => match wl::as_string(&entry.value) {
                Some(s) => source = Some(s.to_owned()),
                None => diagnostics.push(warning("Source must be a String".into())),
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
                    diagnostics.extend(extra_warnings.into_iter().map(warning));
                }
                None => diagnostics.push(warning("Inputs must be an Association".into())),
            },
            other => diagnostics.push(warning(format!("unknown input key: {}", other))),
        }
    }

    let source = match source {
        Some(s) => s,
        None => {
            put_result(link, "Error", None, 0, &diagnostics);
            return;
        }
    };

    let world = MathemeWorld::new(source, inputs);
    let warned = typst::compile::<PagedDocument>(&world);

    diagnostics.extend(warned.warnings.iter().map(|d| diagnostic_from(d, &world)));

    match warned.output {
        Ok(document) => {
            let pages = document.pages().len() as i64;
            match typst_pdf::pdf(&document, &PdfOptions::default()) {
                Ok(bytes) => put_result(link, "Success", Some(bytes), pages, &diagnostics),
                Err(errors) => {
                    diagnostics.extend(errors.iter().map(|d| diagnostic_from(d, &world)));
                    put_result(link, "Error", None, pages, &diagnostics);
                }
            }
        }
        Err(errors) => {
            diagnostics.extend(errors.iter().map(|d| diagnostic_from(d, &world)));
            put_result(link, "Error", None, 0, &diagnostics);
        }
    }
}

#[export(wstp)]
fn matheme_version(link: &mut Link) {
    let args = wl::read_args(link);
    assert!(args.is_empty(), "matheme_version takes no arguments");

    let result = wl::assoc(vec![
        wl::rule("Status", wl::string("Success")),
        wl::rule("Matheme", wl::string(env!("CARGO_PKG_VERSION"))),
        wl::rule("Typst", wl::string("0.15.1")),
    ]);
    link.put_expr(&result).expect("failed to write version");
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
