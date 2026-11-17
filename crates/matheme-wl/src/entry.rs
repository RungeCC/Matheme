//! Association entries: immediate rules, delayed rules, and everything else.
//!
//! `Rule` (`key -> value`) and `RuleDelayed` (`key :> value`) are different
//! things: the first carries a value, the second an expression that WL would
//! evaluate later. Only immediate rules can be interpreted as Typst values, so
//! [`Entry`] keeps them apart instead of flattening both heads into one type.

use wolfram_library_link::expr::{Expr as RawExpr, RuleEntry};

use crate::expr::Expr;
use crate::normal::Normal;
use crate::symbol::Symbol;

/// An immediate rule `key -> value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    key: Expr,
    value: Expr,
}

impl Rule {
    pub fn new(key: impl Into<Expr>, value: impl Into<Expr>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    pub fn key(&self) -> &Expr {
        &self.key
    }

    pub fn value(&self) -> &Expr {
        &self.value
    }

    pub fn into_raw(self) -> RuleEntry {
        RuleEntry::rule(self.key.into_raw(), self.value.into_raw())
    }
}

/// One element of an association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// `key -> value`.
    Rule(Rule),
    /// `key :> value`. The key is kept for diagnostics; the value is not, since
    /// WL has not evaluated it and the bridge must not pretend otherwise.
    RuleDelayed { key: Expr },
    /// An element that is not a rule at all.
    Other { expr: Expr },
}

impl Entry {
    /// The key, if this entry has one at all.
    pub fn key(&self) -> Option<&Expr> {
        match self {
            Entry::Rule(rule) => Some(rule.key()),
            Entry::RuleDelayed { key } => Some(key),
            Entry::Other { .. } => None,
        }
    }

    /// The string key, when it is a WL String.
    pub fn key_name(&self) -> Option<&str> {
        self.key().and_then(Expr::as_str)
    }

    /// The key/value pair of an immediate rule.
    ///
    /// Returns a complete diagnostic sentence for delayed or malformed entries,
    /// so callers can forward it without re-deriving why the entry was skipped.
    pub fn pair(&self) -> Result<(&Expr, &Expr), String> {
        match self {
            Entry::Rule(rule) => Ok((rule.key(), rule.value())),
            Entry::RuleDelayed { key } => Err(format!(
                "association entry {} uses RuleDelayed (:>); its right-hand side is not evaluated, \
                 so it cannot be converted",
                describe_key(key)
            )),
            Entry::Other { expr } => Err(format!(
                "association entry {} is not a Rule",
                describe_key(expr)
            )),
        }
    }
}

/// Render a key for diagnostics: strings quoted, everything else in WL form.
fn describe_key(key: &Expr) -> String {
    match key.as_str() {
        Some(name) => format!("\"{name}\""),
        None => key.raw().to_string(),
    }
}

/// Parse association elements, keeping the head of each entry.
pub(crate) fn parse(elements: &[RawExpr]) -> Vec<Entry> {
    elements
        .iter()
        .map(|element| match Normal::from_expr(element) {
            Some(normal) if normal.has_head(&Symbol::RULE) && normal.len() == 2 => Entry::Rule(
                Rule::new(normal.elements()[0].clone(), normal.elements()[1].clone()),
            ),
            Some(normal) if normal.has_head(&Symbol::RULE_DELAYED) && normal.len() == 2 => {
                Entry::RuleDelayed {
                    key: Expr::from_raw(normal.elements()[0].clone()),
                }
            }
            _ => Entry::Other {
                expr: Expr::from_raw(element.clone()),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Expr;

    fn wstp_association(elements: Vec<Expr>) -> Expr {
        Expr::normal(Symbol::ASSOCIATION, elements)
    }

    #[test]
    fn immediate_and_delayed_rules_are_distinguished() {
        let expr = wstp_association(vec![
            Expr::normal(Symbol::RULE, [Expr::from("a"), Expr::from(1_i64)]),
            Expr::normal(Symbol::RULE_DELAYED, [Expr::from("b"), Expr::from(2_i64)]),
        ]);
        let entries = expr.as_association().expect("association");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].pair().expect("immediate rule").1.as_i64(),
            Some(1)
        );
        let error = entries[1].pair().expect_err("delayed rule is rejected");
        assert!(error.contains("RuleDelayed"), "{error}");
        assert!(error.contains("\"b\""), "{error}");
        assert_eq!(entries[1].key_name(), Some("b"));
    }

    #[test]
    fn non_rule_elements_are_reported() {
        let expr = wstp_association(vec![Expr::from(7_i64)]);
        let entries = expr.as_association().expect("association");
        let error = entries[0].pair().expect_err("not a rule");
        assert!(error.contains("is not a Rule"), "{error}");
    }
}
