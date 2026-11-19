//! Diagnostics in the shape used by `typst-cli`.
//!
//! Typst 0.15 spans are `DiagSpan`, not plain `Span`: a numbered span must be
//! resolved through `Source::range`, an external span already carries a byte
//! range, and a detached span points nowhere. This module produces both a
//! stable structured form and a plain-text rendering that matches `typst-cli`.

use std::ops::Range;

use typst::diag::{Severity, SourceDiagnostic};
use typst::syntax::{DiagSpan, DiagSpanKind, FileId, Spanned};
use typst::World;
use typst_kit::diagnostics::{
    emit as emit_diagnostics, termcolor, DiagnosticFormat, DiagnosticWorld,
};

use crate::world::MathemeWorld;

/// A resolved source location: user-facing file name, 1-based line/column,
/// byte range and the text of the containing line.
#[derive(Debug, Clone)]
pub struct Location {
    pub file: String,
    pub line: i64,
    pub column: i64,
    pub range: (usize, usize),
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Hint {
    pub message: String,
    pub location: Option<Location>,
}

#[derive(Debug, Clone)]
pub struct Trace {
    pub kind: String,
    pub location: Option<Location>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub message: String,
    pub location: Option<Location>,
    pub hints: Vec<Hint>,
    pub trace: Vec<Trace>,
    /// True for host/bridge-level warnings that did not come from Typst.
    pub custom: bool,
}

/// Resolve a diagnostic span the same way `typst-kit`'s `WorldFiles::range`
/// does.
fn resolve_span(span: DiagSpan, world: &MathemeWorld) -> Option<(FileId, Range<usize>)> {
    match span.get() {
        DiagSpanKind::Detached => None,
        DiagSpanKind::Number { id, num, sub_range } => {
            let source = world.source(id).ok()?;
            source.range(num, sub_range).map(|range| (id, range))
        }
        DiagSpanKind::Range { id, range } => Some((id, range)),
    }
}

fn location(id: FileId, range: Range<usize>, world: &MathemeWorld) -> Option<Location> {
    let source = world.source(id).ok()?;
    let lines = source.lines();
    let (line, column) = lines.byte_to_line_column(range.start)?;
    let line_range = lines.line_to_range(line).unwrap_or_else(|| range.clone());
    let text = lines
        .text()
        .get(line_range)
        .unwrap_or("")
        .trim_end_matches(['\r', '\n'])
        .to_owned();

    Some(Location {
        file: world.name(id),
        line: line as i64 + 1,
        column: column as i64 + 1,
        range: (range.start, range.end),
        text,
    })
}

fn hint_location(
    hint: &Spanned<typst::diag::EcoString, DiagSpan>,
    world: &MathemeWorld,
) -> Option<Location> {
    let (id, range) = resolve_span(hint.span, world)?;
    location(id, range, world)
}

fn trace_location(
    point: &Spanned<typst::diag::Tracepoint>,
    world: &MathemeWorld,
) -> Option<Location> {
    let (id, range) = resolve_span(point.span.into(), world)?;
    location(id, range, world)
}

pub fn from_typst(diag: &SourceDiagnostic, world: &MathemeWorld) -> Diagnostic {
    let location =
        resolve_span(diag.span, world).and_then(|(id, range)| location(id, range, world));

    Diagnostic {
        severity: match diag.severity {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
        },
        message: diag.message.to_string(),
        hints: diag
            .hints
            .iter()
            .map(|hint| Hint {
                message: hint.v.to_string(),
                location: hint_location(hint, world),
            })
            .collect(),
        trace: diag
            .trace
            .iter()
            .map(|point| {
                let location = trace_location(point, world);
                Trace {
                    kind: point.v.to_string(),
                    text: location
                        .as_ref()
                        .map(|loc| loc.text.clone())
                        .unwrap_or_default(),
                    location,
                }
            })
            .collect(),
        location,
        custom: false,
    }
}

/// A host/bridge-level error (failed source load, internal evaluation failure).
pub fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: "Error",
        message: message.into(),
        location: None,
        hints: vec![],
        trace: vec![],
        custom: true,
    }
}

/// A host/bridge-level warning (bad option, offline hint, unsupported input value).
pub fn warning(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: "Warning",
        message: message.into(),
        location: None,
        hints: vec![],
        trace: vec![],
        custom: true,
    }
}

/// Render Typst diagnostics with the CLI's codespan formatter, then append
/// custom diagnostics as plain `error:` / `hint:` lines.
pub fn format(
    world: &MathemeWorld,
    typst_diags: &[SourceDiagnostic],
    all: &[Diagnostic],
) -> String {
    let mut buffer = termcolor::Buffer::no_color();
    if let Err(err) = emit_diagnostics(
        &mut buffer,
        world,
        typst_diags.iter(),
        DiagnosticFormat::Human,
    ) {
        return format!("failed to render diagnostics: {err}");
    }
    let mut text = String::from_utf8_lossy(buffer.as_slice()).into_owned();
    for diag in all.iter().filter(|d| d.custom) {
        let prefix = if diag.severity == "Error" {
            "error"
        } else {
            "hint"
        };
        text.push_str(&format!("{prefix}: {}\n", diag.message));
    }
    text
}
