//! The standard WSTP call pipeline.
//!
//! The library exposes exactly one link function. The Wolfram package sends a
//! single envelope, `<|"Action" -> name, "Arguments" -> <|…|>|>`, and
//! [`serve`] turns it into a shaped answer. The action table lives in
//! [`crate`]; [`invoke`] applies the shared per-call state (host font paths and
//! the download gate) before dispatching to an [`Endpoint`].
//!
//! Calls always originate in our own Wolfram package, so parsing assumes the
//! declared shape instead of validating every value defensively. A mismatch
//! is a programming bug: `serve` catches it and returns a shaped failure
//! (rendered as an ErrorTools `Failure` by the kernel) rather than aborting
//! the kernel.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use matheme_wl::{Entry, Expr, Rule};
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

    fn parse(args: Vec<Entry>) -> Self::Input;
    fn invoke(input: Self::Input) -> Answer;
}

/// Apply the state shared by every call, then run one endpoint.
pub fn invoke<E: Endpoint>(link: &mut Link, args: Vec<Entry>) -> Answer {
    if E::HOST_FONTS {
        wolfram::configure_world_fonts(link);
    }
    // A per-call download opt-in never outlives the call that set it.
    world::set_call_downloads(false);
    E::invoke(E::parse(args))
}

/// The one expression read from the link: `Action` plus its `Arguments`.
pub struct Envelope {
    pub action: String,
    pub arguments: Vec<Entry>,
}

/// Read the envelope, resolving Wolfram 15's context-free symbol names
/// (e.g. `List`, `True`) into their `System\`` forms.
fn read_envelope(link: &mut Link) -> Envelope {
    let expr: Expr = link
        .get_expr_with_resolver(&mut |name: &str| Some(RawSymbol::new(&format!("System`{name}"))))
        .expect("failed to read WSTP call")
        .into();
    // A LinkObject function always delivers its WL arguments as a List; the
    // package passes exactly one expression, the Action/Arguments envelope.
    let args: Vec<Expr> = expr
        .as_list()
        .expect("expected an argument list")
        .into_iter()
        .collect();
    assert!(args.len() == 1, "expected exactly one call envelope");
    let entries = args[0]
        .as_association()
        .expect("expected an Action/Arguments association");
    let action = string(field(&entries, "Action").expect("missing Action"));
    let arguments = field(&entries, "Arguments")
        .expect("missing Arguments")
        .as_association()
        .expect("Arguments must be an Association");
    Envelope { action, arguments }
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
        Answer::Failure(failure) => Expr::association([
            Rule::new("Status", "Error"),
            Rule::new("ErrorTag", failure.tag),
            Rule::new("Message", failure.message),
            Rule::new("Diagnostics", diagnostics_expr(&failure.diagnostics)),
            Rule::new("Formatted", failure.formatted.unwrap_or_default()),
        ]),
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
    let answer = panic::catch_unwind(AssertUnwindSafe(|| {
        let envelope = read_envelope(link);
        dispatch(link, &envelope.action, envelope.arguments)
    }))
    .unwrap_or_else(|payload| {
        Answer::Failure(Failure::new("Call", panic_message(payload, "matheme_call")))
    });

    link.put_expr(render(answer).raw())
        .unwrap_or_else(|_| panic!("matheme_call: failed to write result expression"));
}

// ---------------------------------------------------------------------------
// Shared association readers. These encode the declared shapes and nothing
// else: a wrong shape is a bug in our own Wolfram package, not user input.
// ---------------------------------------------------------------------------

/// Pair an association entry; delayed rules are rejected with their reason.
pub fn pair(entry: &Entry) -> (&Expr, &Expr) {
    entry.pair().unwrap_or_else(|reason| panic!("{reason}"))
}

/// The string key of an association entry.
pub fn key(key: &Expr) -> &str {
    key.as_str()
        .unwrap_or_else(|| panic!("association key must be a String, got: {}", key.raw()))
}

/// A present-or-absent association field.
pub fn field<'a>(entries: &'a [Entry], name: &str) -> Option<&'a Expr> {
    entries
        .iter()
        .find(|entry| entry.key_name() == Some(name))
        .map(|entry| pair(entry).1)
}

/// A field the caller must supply.
pub fn required<'a>(entries: &'a [Entry], name: &str) -> &'a Expr {
    field(entries, name).unwrap_or_else(|| panic!("missing association key: {name}"))
}

pub fn string(expr: &Expr) -> String {
    expr.as_str().expect("expected a WL String").to_owned()
}

pub fn boolean(expr: &Expr) -> bool {
    expr.as_bool().expect("expected True or False")
}

pub fn integer(expr: &Expr) -> i64 {
    expr.as_i64().expect("expected a WL Integer")
}

pub fn nonnegative(expr: &Expr, name: &str) -> i64 {
    let value = integer(expr);
    assert!(value >= 0, "{name} must be a non-negative Integer");
    value
}

pub fn character(expr: &Expr) -> char {
    let text = string(expr);
    let mut chars = text.chars();
    let first = chars.next().expect("expected a non-empty String");
    assert!(
        chars.next().is_none(),
        "Character must be a single character"
    );
    first
}

/// Parse a named enumeration value, e.g. a font source or package origin.
pub fn named<T>(expr: &Expr, parse: impl Fn(&str) -> Option<T>, what: &str) -> T {
    let name = string(expr);
    parse(&name).unwrap_or_else(|| panic!("unknown {what}: {name}"))
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
        assert_eq!(string(required(&args, "Source")), "text");
        assert!(field(&args, "Missing").is_none());
    }

    #[test]
    #[should_panic(expected = "missing association key")]
    fn a_missing_key_is_a_programming_error() {
        required(&[], "Source");
    }

    #[test]
    fn failure_renders_uniformly() {
        let expr = render(Answer::Failure(Failure::new("Compile", "bad source")));
        let args = expr.as_association().expect("association");
        let tag = field(&args, "ErrorTag").expect("tag");
        assert_eq!(tag.as_str(), Some("Compile"));
    }
}
