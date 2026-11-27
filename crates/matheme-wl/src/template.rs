//! A string-slot template as a WL bridge value.
//!
//! Template bodies use tinytemplate's value interpolation: `{1}`, `{2}`, ...
//! refer to already-rendered string arguments, and `{rest}` inserts the
//! remaining arguments joined by the separator. Blocks (`{{ if ... }}`) are
//! rejected so the WL reference renderer can apply the same template with a
//! small, equivalent substitution. Values are strings, so substitution cannot
//! evaluate WL code. This is distinct from Wolfram's `StringTemplate`.

use std::collections::{HashMap, HashSet};
use std::fmt;

use tinytemplate::TinyTemplate;

use crate::Expr;
use crate::symbol::short_name;

/// Why a template could not be read or expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError(String);

impl TemplateError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TemplateError {}

/// The value-interpolation subset of tinytemplate that both engines share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypstTemplate {
    body: String,
    separator: String,
    fixed_slots: HashSet<usize>,
}

impl TypstTemplate {
    /// Short name of the Wolfram bridge symbol this value decodes from.
    ///
    /// The LibraryFunction WSTP reader transmits symbols without their
    /// context: `WSGetUTF8Symbol` yields the bare name and matheme-wstp
    /// resolves unqualified names into `` System` ``, so
    /// `` Runge`Matheme`TypstTemplate `` arrives here as
    /// `` System`TypstTemplate ``. Matching the short name is therefore as
    /// specific as this transport allows — a foreign-context
    /// `` TypstTemplate `` flattens to the same wire name — while Wolfram's
    /// `StringTemplate` and `TemplateObject` values still fail closed because
    /// their names differ.
    pub const WIRE_HEAD: &str = "TypstTemplate";

    /// Compile a template body. Only value interpolation is accepted: a
    /// tinytemplate block would have no counterpart in the WL renderer.
    pub fn new(
        body: impl Into<String>,
        separator: impl Into<String>,
    ) -> Result<Self, TemplateError> {
        let body = body.into();
        let separator = separator.into();
        if body.contains("{{") {
            return Err(TemplateError::new(
                "TypstTemplate accepts value slots only, not tinytemplate blocks",
            ));
        }
        // Compilation here is validation: a body that survives `new` is one
        // the reference renderer would also accept.
        compile(&body)?;
        Ok(Self {
            fixed_slots: fixed_slots(&body),
            body,
            separator,
        })
    }

    /// Substitute rendered string arguments into the template using
    /// tinytemplate. Missing slots are errors, never empty strings.
    pub fn expand(&self, args: &[String]) -> Result<String, TemplateError> {
        let mut context: HashMap<String, String> = HashMap::with_capacity(args.len() + 1);
        for (index, value) in args.iter().enumerate() {
            context.insert((index + 1).to_string(), value.clone());
        }
        let rest = (1..=args.len())
            .filter(|index| !self.fixed_slots.contains(index))
            .map(|index| args[index - 1].as_str())
            .collect::<Vec<_>>()
            .join(&self.separator);
        context.insert("rest".to_owned(), rest);

        // tinytemplate's registry borrows the template text
        // (`add_template(&'template str)`), so the compiled renderer cannot be
        // stored in this owned value — and `TypstTemplate` is decoded fresh
        // from every config patch, so leaking the body to reach `'static`
        // would grow without bound. Bodies are a few slots; the re-parse is
        // noise against the surrounding tree walk.
        compile(&self.body)?
            .render("t", &context)
            .map_err(|err| TemplateError::new(err.to_string()))
    }
}

fn compile(body: &str) -> Result<TinyTemplate<'_>, TemplateError> {
    let mut tt = TinyTemplate::new();
    tt.set_default_formatter(&tinytemplate::format_unescaped);
    tt.add_template("t", body)
        .map_err(|err| TemplateError::new(err.to_string()))?;
    Ok(tt)
}

/// Collect the numeric slots used by the template. This is metadata for
/// computing `{rest}`; substitution itself is entirely tinytemplate's job.
fn fixed_slots(body: &str) -> HashSet<usize> {
    let mut used = HashSet::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'{') {
            chars.next();
            continue;
        }
        if ch != '{' {
            continue;
        }
        let mut name = String::new();
        let mut closed = false;
        while let Some(&next) = chars.peek() {
            chars.next();
            if next == '}' {
                closed = true;
                break;
            }
            if next == '{' {
                break;
            }
            name.push(next);
        }
        if closed
            && let Ok(index) = name.parse::<usize>()
            && index > 0
        {
            used.insert(index);
        }
    }
    used
}

/// Decode `TypstTemplate[body]` / `TypstTemplate[body, restSeparator]` by the
/// wire head name ([`TypstTemplate::WIRE_HEAD`]).
impl TryFrom<&Expr> for TypstTemplate {
    type Error = TemplateError;

    fn try_from(expr: &Expr) -> Result<Self, Self::Error> {
        let normal = expr
            .as_normal()
            .ok_or_else(|| TemplateError::new("expected a TypstTemplate value"))?;
        let head = normal
            .head()
            .ok_or_else(|| TemplateError::new("expected a TypstTemplate value"))?;
        if short_name(head) != Self::WIRE_HEAD {
            return Err(TemplateError::new(
                "expected a TypstTemplate value, not a Wolfram template",
            ));
        }
        let body = normal
            .get(0)
            .and_then(Expr::as_str)
            .ok_or_else(|| TemplateError::new("TypstTemplate body must be a String"))?;
        let separator = match normal.len() {
            1 => ", ".to_owned(),
            2 => normal
                .get(1)
                .and_then(Expr::as_str)
                .ok_or_else(|| TemplateError::new("TypstTemplate rest separator must be a String"))?
                .to_owned(),
            _ => {
                return Err(TemplateError::new(
                    "TypstTemplate takes one or two arguments",
                ));
            }
        };
        Self::new(body, separator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Symbol;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn fixed_slots_are_replaced_with_rendered_strings() {
        let template = TypstTemplate::new("frac({1}, {2})", ", ").unwrap();
        assert_eq!(template.expand(&args(&["a", "b"])).unwrap(), "frac(a, b)");
    }

    #[test]
    fn a_rest_slot_is_not_a_fixed_arity() {
        let template = TypstTemplate::new("row({1}; {rest})", ", ").unwrap();
        assert_eq!(template.expand(&args(&["a"])).unwrap(), "row(a; )");
        assert_eq!(
            template.expand(&args(&["a", "b", "c"])).unwrap(),
            "row(a; b, c)"
        );
    }

    #[test]
    fn the_rest_separator_is_configurable() {
        let template = TypstTemplate::new("{rest}", " + ").unwrap();
        assert_eq!(template.expand(&args(&["a", "b"])).unwrap(), "a + b");
    }

    #[test]
    fn out_of_range_slots_fail_closed() {
        let template = TypstTemplate::new("{2}", ", ").unwrap();
        assert!(template.expand(&args(&["a"])).is_err());
    }

    #[test]
    fn tinytemplate_blocks_are_rejected() {
        assert!(TypstTemplate::new("{{ if x }}y{{ endif }}", ", ").is_err());
    }

    #[test]
    fn escaped_braces_stay_literal() {
        let template = TypstTemplate::new(r"f(\{1\})", ", ").unwrap();
        assert_eq!(template.expand(&args(&["a"])).unwrap(), "f({1\\})");
    }

    #[test]
    fn decoding_accepts_one_or_two_string_arguments() {
        let head = Symbol::new("Runge`Matheme`TypstTemplate");
        let one = Expr::normal(&head, [Expr::string("{1}")]);
        assert_eq!(
            TypstTemplate::try_from(&one)
                .unwrap()
                .expand(&args(&["x"]))
                .unwrap(),
            "x"
        );
        let two = Expr::normal(&head, [Expr::string("{rest}"), Expr::string("; ")]);
        assert_eq!(
            TypstTemplate::try_from(&two)
                .unwrap()
                .expand(&args(&["x", "y"]))
                .unwrap(),
            "x; y"
        );
    }

    #[test]
    fn decoding_accepts_the_context_flattened_wire_form() {
        // The LibraryFunction WSTP reader transmits `` Runge`Matheme`
        // TypstTemplate `` as `` System`TypstTemplate ``; the short-name check
        // must accept that form.
        let expr = Expr::normal(Symbol::new("System`TypstTemplate"), [Expr::string("{1}")]);
        assert_eq!(
            TypstTemplate::try_from(&expr)
                .unwrap()
                .expand(&args(&["x"]))
                .unwrap(),
            "x"
        );
    }

    #[test]
    fn decoding_rejects_wolfram_templates() {
        let expr = Expr::normal(Symbol::new("System`StringTemplate"), [Expr::string("{1}")]);
        assert!(TypstTemplate::try_from(&expr).is_err());
    }
}
