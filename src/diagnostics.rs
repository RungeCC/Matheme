//! Wolfram Language serialization for [`matheme_world::diagnostics`].

use matheme_wl::{assoc, Expr};

pub use matheme_world::diagnostics::{warning, Diagnostic, Location};

fn location_expr(location: &Option<Location>) -> Expr {
    let Some(location) = location else {
        return Expr::null();
    };
    assoc! {
        "File" -> location.file.as_str(),
        "Line" -> location.line,
        "Column" -> location.column,
        "Range" -> Expr::list([location.range.0 as i64, location.range.1 as i64]),
        "Text" -> location.text.as_str(),
    }
}

pub fn expr(diag: &Diagnostic) -> Expr {
    assoc! {
        "Severity" -> diag.severity,
        "Message" -> diag.message.as_str(),
        "Location" -> location_expr(&diag.location),
        "Hints" -> Expr::list(diag.hints.iter().map(|hint| {
            assoc! {
                "Message" -> hint.message.as_str(),
                "Location" -> location_expr(&hint.location),
            }
        })),
        "Trace" -> Expr::list(diag.trace.iter().map(|trace| {
            assoc! {
                "Kind" -> trace.kind.as_str(),
                "Location" -> location_expr(&trace.location),
                "Text" -> trace.text.as_str(),
            }
        })),
    }
}
