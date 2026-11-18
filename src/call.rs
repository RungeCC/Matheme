//! The standard WSTP call pipeline.
//!
//! The library exposes exactly one link function. The Wolfram package sends a
//! single envelope, `<|"Action" -> name, "Arguments" -> <|…|>|>`, and
//! [`serve`] turns it into a shaped answer. The action table lives in
//! [`crate`]; [`invoke`] applies the shared per-call state (host font paths and
//! the download gate) before dispatching to an [`Endpoint`].

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use matheme_wl::{assoc, Entry, Expr, Rule};
use wolfram_library_link::{expr::Symbol as RawSymbol, wstp::Link};

use matheme_world as world;

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

/// The declaration of one action: its name, whether Wolfram's bundled font
/// directories must be configured first, and the expected arguments.
pub trait Endpoint {
    const NAME: &'static str;
    const HOST_FONTS: bool = true;
    type Input;

    fn parse(args: Vec<Entry>) -> Result<Self::Input, Failure>;
    fn invoke(input: Self::Input) -> Answer;
}

/// Apply the state shared by every call, then run one endpoint.
pub fn invoke<E: Endpoint>(link: &mut Link, args: Vec<Entry>) -> Answer {
    if E::HOST_FONTS {
        wolfram::configure_world_fonts(link);
    }
    // A per-call download opt-in never outlives the call that set it.
    world::set_call_downloads(false);
    match E::parse(args) {
        Ok(input) => E::invoke(input),
        Err(failure) => Answer::Failure(failure),
    }
}

/// The one expression read from the link: `Action` plus its `Arguments`.
pub struct Envelope {
    pub action: String,
    pub arguments: Vec<Entry>,
}

/// Read the envelope, resolving Wolfram 15's context-free symbol names
/// (e.g. `List`, `True`) into their `System\`` forms.
fn read_envelope(link: &mut Link) -> Result<Envelope, Failure> {
    let expr: Expr = link
        .get_expr_with_resolver(&mut |name: &str| Some(RawSymbol::new(&format!("System`{name}"))))
        .map_err(|err| error(format!("failed to read WSTP call: {err}")))?
        .into();
    // A LinkObject function always delivers its WL arguments as a List; the
    // package passes exactly one expression, the Action/Arguments envelope.
    let args = expr
        .as_list()
        .ok_or_else(|| error("expected an argument list"))?;
    let [envelope] = args.as_slice() else {
        return Err(error("expected exactly one call envelope"));
    };
    let entries = envelope
        .as_association()
        .ok_or_else(|| error("expected an Action/Arguments association"))?;
    let action = string(required(&entries, "Action")?)?;
    let arguments = required(&entries, "Arguments")?
        .as_association()
        .ok_or_else(|| error("Arguments must be an Association"))?;
    Ok(Envelope { action, arguments })
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

/// Read one envelope, dispatch it, and write the answer. `dispatch` is supplied
/// by the crate root, which owns the action table and imports the endpoints.
pub fn serve(link: &mut Link, dispatch: impl FnOnce(&mut Link, &str, Vec<Entry>) -> Answer) {
    let answer = panic::catch_unwind(AssertUnwindSafe(|| match read_envelope(link) {
        Ok(envelope) => dispatch(link, &envelope.action, envelope.arguments),
        Err(failure) => Answer::Failure(failure),
    }))
    .unwrap_or_else(|payload| {
        Answer::Failure(Failure::new("Call", panic_message(payload, "matheme_call")))
    });

    // Never panic across the FFI boundary if the WSTP link itself failed.
    let _ = link.put_expr(render(answer).raw());
}

// ---------------------------------------------------------------------------
// Shared association readers returning shaped Failures on mismatch.
// ---------------------------------------------------------------------------

/// Ensure an endpoint taking no arguments was given an empty Association.
pub fn ensure_empty(entries: &[Entry], action: &str) -> Result<(), Failure> {
    if entries.is_empty() {
        Ok(())
    } else {
        Err(error(format!("{action} takes no arguments")))
    }
}

/// Pair an association entry; delayed rules are rejected with their reason.
pub fn pair(entry: &Entry) -> Result<(&Expr, &Expr), Failure> {
    entry.pair().map_err(error)
}

/// The string key of an association entry.
pub fn key(key: &Expr) -> Result<&str, Failure> {
    key.as_str()
        .ok_or_else(|| error(format!("association key must be a String, got: {}", key.raw())))
}

/// A present-or-absent association field.
pub fn field<'a>(entries: &'a [Entry], name: &str) -> Result<Option<&'a Expr>, Failure> {
    for entry in entries {
        if entry.key_name() == Some(name) {
            return Ok(Some(pair(entry)?.1));
        }
    }
    Ok(None)
}

/// A field the caller must supply.
pub fn required<'a>(entries: &'a [Entry], name: &str) -> Result<&'a Expr, Failure> {
    field(entries, name)?.ok_or_else(|| error(format!("missing association key: {name}")))
}

pub fn string(expr: &Expr) -> Result<String, Failure> {
    expr.as_str()
        .map(str::to_owned)
        .ok_or_else(|| error("expected a WL String"))
}

pub fn boolean(expr: &Expr) -> Result<bool, Failure> {
    expr.as_bool()
        .ok_or_else(|| error("expected True or False"))
}

pub fn integer(expr: &Expr) -> Result<i64, Failure> {
    expr.as_i64()
        .ok_or_else(|| error("expected a WL Integer"))
}

pub fn nonnegative(expr: &Expr, name: &str) -> Result<i64, Failure> {
    let value = integer(expr)?;
    if value < 0 {
        return Err(error(format!("{name} must be a non-negative Integer")));
    }
    Ok(value)
}

pub fn character(expr: &Expr) -> Result<char, Failure> {
    let text = string(expr)?;
    let mut chars = text.chars();
    let first = chars
        .next()
        .ok_or_else(|| error("expected a non-empty String"))?;
    if chars.next().is_some() {
        return Err(error("Character must be a single character"));
    }
    Ok(first)
}

/// Parse a named enumeration value, e.g. a font source or package origin.
pub fn named<T>(
    expr: &Expr,
    parse: impl Fn(&str) -> Option<T>,
    what: &str,
) -> Result<T, Failure> {
    let name = string(expr)?;
    parse(&name).ok_or_else(|| error(format!("unknown {what}: {name}")))
}

/// Build association entries from rules, mainly for tests.
#[cfg(test)]
pub fn entries(rules: impl IntoIterator<Item = Rule>) -> Vec<Entry> {
    Expr::association(rules)
        .as_association()
        .expect("association")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_shapes_are_read_without_fallbacks() {
        let args = entries([Rule::new("Source", "text")]);
        assert_eq!(string(required(&args, "Source").unwrap()).unwrap(), "text");
        assert!(field(&args, "Missing").unwrap().is_none());
    }

    #[test]
    fn a_missing_key_returns_a_failure() {
        let err = required(&[], "Source").expect_err("missing key");
        assert_eq!(err.tag, "Call");
        assert!(err.message.contains("missing association key"), "{}", err.message);
    }

    #[test]
    fn failure_renders_uniformly() {
        let expr = render(Answer::Failure(Failure::new("Compile", "bad source")));
        let args = expr.as_association().expect("association");
        let tag = field(&args, "ErrorTag").unwrap().expect("tag");
        assert_eq!(tag.as_str(), Some("Compile"));
    }
}
