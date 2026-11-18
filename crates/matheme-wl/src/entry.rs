//! Association entries: immediate rules, delayed rules, and everything else.
//!
//! `Rule` (`key -> value`) and `RuleDelayed` (`key :> value`) are different
//! things: the first carries a value, the second an expression that WL would
//! evaluate later. Only immediate rules can be interpreted as Typst values, so
//! [`Entry`] keeps them apart instead of flattening both heads into one type.

use wolfram_library_link::expr::RuleEntry;

use crate::expr::Expr;
use crate::symbol::Symbol;

/// An owned immediate rule `key -> value`, used when constructing associations.
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

/// One borrowed element of an association.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry<'a> {
    /// `key -> value`.
    Rule { key: &'a Expr, value: &'a Expr },
    /// `key :> value`. The key is kept for diagnostics; the value is not, since
    /// WL has not evaluated it and the bridge must not pretend otherwise.
    RuleDelayed { key: &'a Expr },
    /// An element that is not a rule at all.
    Other { expr: &'a Expr },
}

impl<'a> Entry<'a> {
    /// The key, if this entry has one at all.
    pub fn key(&self) -> Option<&'a Expr> {
        match *self {
            Entry::Rule { key, .. } | Entry::RuleDelayed { key } => Some(key),
            Entry::Other { .. } => None,
        }
    }

    /// The string key, when it is a WL String.
    pub fn key_name(&self) -> Option<&'a str> {
        self.key().and_then(Expr::as_str)
    }

    /// The key/value pair of an immediate rule.
    ///
    /// Returns a complete diagnostic sentence for delayed or malformed entries,
    /// so callers can forward it without re-deriving why the entry was skipped.
    pub fn pair(&self) -> Result<(&'a Expr, &'a Expr), String> {
        match *self {
            Entry::Rule { key, value } => Ok((key, value)),
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
        None => key.to_string(),
    }
}

/// Parse association elements without cloning keys or values.
pub(crate) fn parse_slice(elements: &[Expr]) -> Vec<Entry<'_>> {
    elements
        .iter()
        .map(|element| match element.as_normal() {
            Some(normal) if normal.has_head(&Symbol::RULE) && normal.len() == 2 => Entry::Rule {
                key: &normal.elements()[0],
                value: &normal.elements()[1],
            },
            Some(normal) if normal.has_head(&Symbol::RULE_DELAYED) && normal.len() == 2 => {
                Entry::RuleDelayed {
                    key: &normal.elements()[0],
                }
            }
            _ => Entry::Other { expr: element },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
