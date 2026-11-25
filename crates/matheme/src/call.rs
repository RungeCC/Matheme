//! The standard WSTP call pipeline.
//!
//! The library exposes exactly one link function. The Wolfram package sends a
//! single envelope, `<|"Action" -> name, "Arguments" -> <|…|>|>`, and [`serve`]
//! parses it into a typed [`crate::Action`], applies shared per-call state
//! (host font paths and the download gate), executes the action, and writes
//! back the uniform result association.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use matheme_wl::{Entry, Expr, Rule};
use matheme_wstp::WstpLink;
use wolfram_library_link::wstp::Link;

use crate::diagnostics::{self, Diagnostic, DiagnosticSource};
use crate::wolfram;

/// Failure category serialized onto the wire as `"ErrorKind"` and mapped by
/// `Kernel/Matheme.wl` to the corresponding `Wolfram/ErrorTools` failure tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Invalid user-supplied arguments, options, or option combinations
    /// (`MathemeCallError`).
    Call,
    /// Typst compilation, layout, export, or evaluation failure
    /// (`MathemeCompileError`).
    Compile,
    /// LibraryLink / WSTP transport or bridge envelope protocol mismatch
    /// (`MathemeLibraryError`).
    Library,
    /// Caught Rust panic or unrecoverable internal invariant violation
    /// (`MathemeInternalError`).
    Internal,
}

impl ErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Call => "Call",
            Self::Compile => "Compile",
            Self::Library => "Library",
            Self::Internal => "Internal",
        }
    }
}

impl From<ErrorKind> for Expr {
    fn from(kind: ErrorKind) -> Self {
        Expr::from(kind.as_str())
    }
}

fn non_empty_report(report: Option<String>) -> Option<String> {
    report.filter(|text| !text.trim().is_empty())
}

fn append_report_line(report: &mut Option<String>, diag: &Diagnostic) {
    if let Some(text) = report.as_mut() {
        text.push_str(&diagnostics::format_line(diag));
    }
}

/// The successful answer: named rules, non-fatal diagnostics, and an optional
/// formatted diagnostic report for the Wolfram layer to render into a `Cell`.
#[derive(Default)]
pub struct Outcome {
    pub entries: Vec<Rule>,
    pub diagnostics: Vec<Diagnostic>,
    pub report: Option<String>,
}

impl Outcome {
    pub fn new(entries: impl IntoIterator<Item = Rule>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            diagnostics: Vec::new(),
            report: None,
        }
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = Rule>) {
        self.entries.extend(entries);
    }

    pub fn warn(&mut self, source: DiagnosticSource, message: impl Into<String>) {
        let diag = diagnostics::warning(source, message);
        append_report_line(&mut self.report, &diag);
        self.diagnostics.push(diag);
    }

    pub fn with_diagnostics(
        mut self,
        diagnostics: Vec<Diagnostic>,
        report: Option<String>,
    ) -> Self {
        self.diagnostics = diagnostics;
        self.report = non_empty_report(report);
        self
    }
}

/// A typed error. The Wolfram layer maps `kind` to an `ErrorTools` failure tag
/// and turns any non-empty `report` into `FormattedCell`.
#[derive(Debug)]
pub struct Failure {
    pub kind: ErrorKind,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
    pub report: Option<String>,
}

impl Failure {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostics: Vec::new(),
            report: None,
        }
    }

    pub fn call(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Call, message)
    }

    pub fn compile(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Compile, message)
    }

    pub fn library(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Library, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn with_diagnostics(
        mut self,
        diagnostics: Vec<Diagnostic>,
        report: Option<String>,
    ) -> Self {
        self.diagnostics = diagnostics;
        self.report = non_empty_report(report);
        self
    }

    pub fn push_diagnostic(&mut self, diag: Diagnostic) {
        append_report_line(&mut self.report, &diag);
        self.diagnostics.push(diag);
    }
}

/// Build an [`ErrorKind::Call`] failure for invalid user arguments or options.
pub fn error(message: impl Into<String>) -> Failure {
    Failure::call(message)
}

/// Build an [`ErrorKind::Library`] failure for WSTP / envelope protocol errors.
pub fn library_error(message: impl Into<String>) -> Failure {
    Failure::library(message)
}

pub enum Answer {
    Success(Outcome),
    Failure(Failure),
}

impl From<Outcome> for Answer {
    fn from(outcome: Outcome) -> Self {
        Self::Success(outcome)
    }
}

impl From<Failure> for Answer {
    fn from(failure: Failure) -> Self {
        Self::Failure(failure)
    }
}

/// Read the WSTP envelope and parse it directly into a typed action via
/// `parse_action`, avoiding intermediate clones or redundant association scans.
fn read_action<A>(
    link: &mut Link,
    parse_action: impl FnOnce(&str, &[Entry<'_>]) -> Result<A, Failure>,
) -> Result<A, Failure> {
    let expr = WstpLink::new(link).read_expr().map_err(|err| {
        link.clear_error();
        if let Err(skip) = link.new_packet() {
            return library_error(format!(
                "failed to read WSTP call: {err}; recovery failed: {skip}"
            ));
        }
        library_error(format!("failed to read WSTP call: {err}"))
    })?;

    // A LinkObject function always delivers its WL arguments as a List; the
    // package passes exactly one expression, the Action/Arguments envelope.
    let args = expr
        .as_list()
        .ok_or_else(|| library_error("expected an argument list"))?;
    let [envelope] = args else {
        return Err(library_error("expected exactly one call envelope"));
    };
    let entries = envelope
        .as_association()
        .ok_or_else(|| library_error("expected an Action/Arguments association"))?;

    let mut action: Option<&str> = None;
    let mut arguments: Option<&Expr> = None;

    for entry in &entries {
        let (key_expr, value_expr) = entry.pair().map_err(library_error)?;
        let key = key_expr.as_str().ok_or_else(|| {
            library_error(format!("envelope key must be a String, got: {key_expr}"))
        })?;
        match key {
            "Action" => {
                if action.is_some() {
                    return Err(library_error("duplicate envelope key: Action"));
                }
                action = Some(
                    value_expr
                        .as_str()
                        .ok_or_else(|| library_error("envelope.Action: expected a WL String"))?,
                );
            }
            "Arguments" => {
                if arguments.is_some() {
                    return Err(library_error("duplicate envelope key: Arguments"));
                }
                arguments = Some(value_expr);
            }
            other => return Err(library_error(format!("unknown envelope key: {other}"))),
        }
    }

    let action = action.ok_or_else(|| library_error("missing envelope key: Action"))?;
    let arguments = arguments.ok_or_else(|| library_error("missing envelope key: Arguments"))?;
    let arg_entries = arguments
        .as_association()
        .ok_or_else(|| library_error("Arguments must be an Association"))?;

    parse_action(action, &arg_entries)
}

fn diagnostics_expr(items: &[Diagnostic]) -> Expr {
    Expr::list(items.iter().map(diagnostics::expr))
}

/// The wire shape is deliberately uniform: `Status` decides which branch the
/// Wolfram layer takes, `Diagnostics` carries structured diagnostics,
/// `DiagnosticReport` carries an optional internal plain-text report that
/// `Kernel/Matheme.wl` renders into `FormattedCell` (and drops from the public
/// result), and `ErrorKind` / `Message` describe a failure.
fn render(answer: Answer) -> Expr {
    match answer {
        Answer::Success(outcome) => {
            let mut rules = vec![Rule::new("Status", "Success")];
            rules.extend(outcome.entries);
            if !outcome.diagnostics.is_empty() {
                rules.push(Rule::new(
                    "Diagnostics",
                    diagnostics_expr(&outcome.diagnostics),
                ));
            }
            if let Some(report) = non_empty_report(outcome.report) {
                rules.push(Rule::new("DiagnosticReport", report));
            }
            Expr::association(rules)
        }
        Answer::Failure(failure) => {
            let mut rules = vec![
                Rule::new("Status", "Error"),
                Rule::new("ErrorKind", failure.kind),
                Rule::new("Message", failure.message),
                Rule::new("Diagnostics", diagnostics_expr(&failure.diagnostics)),
            ];
            if let Some(report) = non_empty_report(failure.report) {
                rules.push(Rule::new("DiagnosticReport", report));
            }
            Expr::association(rules)
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>, name: &str) -> String {
    let detail = if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else {
        "unexpected panic".to_owned()
    };
    format!("{name}: {detail}")
}

/// Read one envelope, parse it into `crate::Action`, apply shared per-call
/// state, execute it, and write back the shaped answer.
pub fn serve(link: &mut Link) {
    let answer = panic::catch_unwind(AssertUnwindSafe(|| {
        let action = match read_action(link, crate::Action::parse) {
            Ok(action) => action,
            Err(failure) => return Answer::Failure(failure),
        };
        let host_font_warning = if action.needs_host_fonts() {
            wolfram::configure_world_fonts(link)
        } else {
            None
        };
        let mut answer = action.execute();
        if let Some(warning) = host_font_warning {
            match &mut answer {
                Answer::Success(outcome) => outcome.warn(DiagnosticSource::Host, warning),
                Answer::Failure(failure) => {
                    failure.push_diagnostic(diagnostics::warning(DiagnosticSource::Host, warning));
                }
            }
        }
        answer
    }))
    .unwrap_or_else(|payload| {
        Answer::Failure(Failure::internal(panic_message(payload, "matheme_call")))
    });

    if let Err(err) = WstpLink::new(link).put_expr(&render(answer)) {
        // The export wrapper catches unwinds and handles a failed WSTP write.
        panic::resume_unwind(Box::new(format!("failed to write WSTP response: {err}")));
    }
}

/// Parse an association test fixture and feed its entries into `f`.
#[cfg(test)]
pub fn with_entries<R>(
    rules: impl IntoIterator<Item = Rule>,
    f: impl FnOnce(&[Entry<'_>]) -> R,
) -> R {
    let expr = Expr::association(rules);
    let entries = expr.as_association().expect("association");
    f(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assoc_field<'a>(entries: &'a [Entry<'a>], name: &str) -> Option<&'a Expr> {
        entries
            .iter()
            .find(|entry| entry.key_name() == Some(name))
            .and_then(|entry| entry.pair().ok())
            .map(|(_, v)| v)
    }

    #[test]
    fn failure_renders_typed_error_kind_and_omits_empty_report() {
        let expr = render(Answer::Failure(Failure::compile("bad source")));
        let args = expr.as_association().expect("association");
        assert_eq!(
            assoc_field(&args, "ErrorKind").and_then(Expr::as_str),
            Some("Compile")
        );
        assert!(
            assoc_field(&args, "DiagnosticReport").is_none(),
            "empty report must be omitted from the wire association"
        );
        assert!(assoc_field(&args, "Formatted").is_none());

        let with_report = render(Answer::Failure(
            Failure::internal("panic detail").with_diagnostics(
                vec![diagnostics::warning(DiagnosticSource::Host, "extra")],
                Some("warning: extra\n".into()),
            ),
        ));
        let args = with_report.as_association().expect("association");
        assert_eq!(
            assoc_field(&args, "ErrorKind").and_then(Expr::as_str),
            Some("Internal")
        );
        assert_eq!(
            assoc_field(&args, "DiagnosticReport").and_then(Expr::as_str),
            Some("warning: extra\n")
        );
    }
}
