//! Wolfram Language serialization for [`matheme_world::diagnostics`].

use matheme_wl::{assoc, Expr};

pub use matheme_world::diagnostics::{
    format_line, warning, Diagnostic, DiagnosticSource, Location,
};

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
        "Severity" -> diag.severity.as_str(),
        "Source" -> diag.source.as_str(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_severity_and_source() {
        let diag = warning(DiagnosticSource::Graphics, "winding fallback");
        let rendered = expr(&diag);
        let entries = rendered.as_association().expect("association");
        let field = |name: &str| {
            entries
                .iter()
                .find(|entry| entry.key_name() == Some(name))
                .and_then(|entry| entry.pair().ok())
                .and_then(|(_, value)| value.as_str())
        };
        assert_eq!(field("Severity"), Some("Warning"));
        assert_eq!(field("Source"), Some("Graphics"));
        assert_eq!(field("Message"), Some("winding fallback"));
    }
}
