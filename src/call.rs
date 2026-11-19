//! The standard WSTP call pipeline.
//!
//! The library exposes exactly one link function. The Wolfram package sends a
//! single envelope, `<|"Action" -> name, "Arguments" -> <|…|>|>`, and [`serve`]
//! parses it into a typed [`crate::Action`], applies shared per-call state
//! (host font paths and the download gate), executes the action, and writes
//! back the uniform result association.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use matheme_wl::{assoc, Entry, Expr, Rule};
use wolfram_library_link::{expr::Symbol as RawSymbol, wstp::Link};

use crate::diagnostics::{self, Diagnostic};
use crate::wolfram;

/// The successful answer: named rules plus non-fatal warnings.
#[derive(Default)]
pub struct Outcome {
    pub entries: Vec<Rule>,
    pub warnings: Vec<Diagnostic>,
}

impl Outcome {
    pub fn new(entries: impl IntoIterator<Item = Rule>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            warnings: Vec::new(),
        }
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = Rule>) {
        self.entries.extend(entries);
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(diagnostics::warning(message));
    }
}

/// A shaped error. The Wolfram layer turns `tag` into an ErrorTools failure
/// tag and keeps `diagnostics` / `formatted` as its payload.
#[derive(Debug)]
pub struct Failure {
    pub tag: &'static str,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
    pub formatted: Option<String>,
}

impl Failure {
    pub fn new(tag: &'static str, message: impl Into<String>) -> Self {
        Self {
            tag,
            message: message.into(),
            diagnostics: Vec::new(),
            formatted: None,
        }
    }

    pub fn with(mut self, diagnostics: Vec<Diagnostic>, formatted: Option<String>) -> Self {
        self.diagnostics = diagnostics;
        self.formatted = formatted;
        self
    }
}

/// Build a `"Call"` failure for invalid arguments or bridge-level errors.
pub fn error(message: impl Into<String>) -> Failure {
    Failure::new("Call", message)
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
    let expr: Expr = link
        .get_expr_with_resolver(&mut |name: &str| Some(RawSymbol::new(&format!("System`{name}"))))
        .map_err(|err| error(format!("failed to read WSTP call: {err}")))?
        .into();

    // A LinkObject function always delivers its WL arguments as a List; the
    // package passes exactly one expression, the Action/Arguments envelope.
    let args = expr
        .as_list()
        .ok_or_else(|| error("expected an argument list"))?;
    let [envelope] = args else {
        return Err(error("expected exactly one call envelope"));
    };
    let entries = envelope
        .as_association()
        .ok_or_else(|| error("expected an Action/Arguments association"))?;

    let mut action: Option<&str> = None;
    let mut arguments: Option<&Expr> = None;

    for entry in &entries {
        let (key_expr, value_expr) = entry.pair().map_err(error)?;
        let key = key_expr
            .as_str()
            .ok_or_else(|| error(format!("envelope key must be a String, got: {key_expr}")))?;
        match key {
            "Action" => {
                if action.is_some() {
                    return Err(error("duplicate envelope key: Action"));
                }
                action = Some(
                    value_expr
                        .as_str()
                        .ok_or_else(|| error("envelope.Action: expected a WL String"))?,
                );
            }
            "Arguments" => {
                if arguments.is_some() {
                    return Err(error("duplicate envelope key: Arguments"));
                }
                arguments = Some(value_expr);
            }
            other => return Err(error(format!("unknown envelope key: {other}"))),
        }
    }

    let action = action.ok_or_else(|| error("missing envelope key: Action"))?;
    let arguments = arguments.ok_or_else(|| error("missing envelope key: Arguments"))?;
    let arg_entries = arguments
        .as_association()
        .ok_or_else(|| error("Arguments must be an Association"))?;

    parse_action(action, &arg_entries)
}

fn diagnostics_expr(items: &[Diagnostic]) -> Expr {
    Expr::list(items.iter().map(diagnostics::expr))
}

/// The wire shape is deliberately uniform: `Status` decides which branch the
/// Wolfram layer takes, `Diagnostics` carries warnings for `Message`, and
/// `ErrorTag` / `Message` / `Formatted` describe a failure.
fn render(answer: Answer) -> Expr {
    match answer {
        Answer::Success(outcome) => {
            let mut rules = vec![Rule::new("Status", "Success")];
            rules.extend(outcome.entries);
            if !outcome.warnings.is_empty() {
                rules.push(Rule::new(
                    "Diagnostics",
                    diagnostics_expr(&outcome.warnings),
                ));
            }
            Expr::association(rules)
        }
        Answer::Failure(failure) => assoc! {
            "Status" -> "Error",
            "ErrorTag" -> failure.tag,
            "Message" -> failure.message,
            "Diagnostics" -> diagnostics_expr(&failure.diagnostics),
            "Formatted" -> failure.formatted.unwrap_or_default(),
        },
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
        if action.needs_host_fonts() {
            wolfram::configure_world_fonts(link);
        }
        action.execute()
    }))
    .unwrap_or_else(|payload| {
        Answer::Failure(Failure::new("Call", panic_message(payload, "matheme_call")))
    });

    // Never panic across the FFI boundary if the WSTP link itself failed.
    let _ = link.put_expr(render(answer).raw());
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

    #[test]
    fn failure_renders_uniformly() {
        let expr = render(Answer::Failure(Failure::new("Compile", "bad source")));
        let args = expr.as_association().expect("association");
        let tag = args
            .iter()
            .find(|entry| entry.key_name() == Some("ErrorTag"))
            .and_then(|entry| entry.pair().ok())
            .map(|(_, v)| v)
            .expect("tag");
        assert_eq!(tag.as_str(), Some("Compile"));
    }
}
