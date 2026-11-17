//! Ergonomic wrapper around Wolfram Language expressions.
//!
//! [`Expr`] owns a [`RawExpr`] but exposes constructors through `From` and
//! inspections through `as_*` methods, so bridge code does not have to pattern
//! match on `ExprKind` for everyday values. `AsRef<RawExpr>` keeps the WSTP
//! layer able to put the value on a link without exposing internals.

use wolfram_library_link::expr::{
    Expr as RawExpr, ExprKind, Normal, RuleEntry, Symbol,
};

/// An owned Wolfram Language expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr(RawExpr);

/// A key/value rule, as used by WL associations and our result dictionaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    key: Expr,
    value: Expr,
}

const TRUE: &str = "System`True";
const FALSE: &str = "System`False";
const NULL: &str = "System`Null";
const LIST: &str = "System`List";
const ASSOCIATION: &str = "System`Association";
const RULE: &str = "System`Rule";
const RULE_DELAYED: &str = "System`RuleDelayed";

impl Expr {
    /// Wrap an expression produced by the Wolfram expression library.
    pub fn from_raw(raw: RawExpr) -> Self {
        Self(raw)
    }

    /// Borrow the underlying expression (e.g. to put it on a WSTP link).
    pub fn raw(&self) -> &RawExpr {
        &self.0
    }

    /// Unwrap the underlying expression.
    pub fn into_raw(self) -> RawExpr {
        self.0
    }

    pub fn string(value: impl Into<String>) -> Self {
        RawExpr::new(ExprKind::String(value.into())).into()
    }

    pub fn symbol(name: &str) -> Self {
        RawExpr::new(ExprKind::Symbol(Symbol::new(name))).into()
    }

    pub fn boolean(value: bool) -> Self {
        Self::symbol(if value { TRUE } else { FALSE })
    }

    pub fn null() -> Self {
        Self::symbol(NULL)
    }

    /// A normal expression `head[elements…]`.
    pub fn normal(head: &str, elements: impl IntoIterator<Item = impl Into<Expr>>) -> Self {
        RawExpr::normal(
            Symbol::new(head),
            elements.into_iter().map(|e| e.into().into_raw()).collect(),
        )
        .into()
    }

    pub fn list<T: Into<Expr>>(items: impl IntoIterator<Item = T>) -> Self {
        RawExpr::list(items.into_iter().map(|item| item.into().into_raw()).collect()).into()
    }

    pub fn association(entries: impl IntoIterator<Item = Rule>) -> Self {
        RawExpr::new(ExprKind::Association(
            entries.into_iter().map(Rule::into_raw).collect(),
        ))
        .into()
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.0.kind() {
            ExprKind::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.0.kind() {
            ExprKind::Symbol(symbol) if symbol.as_str() == TRUE => Some(true),
            ExprKind::Symbol(symbol) if symbol.as_str() == FALSE => Some(false),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self.0.kind() {
            ExprKind::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// Elements of a normal expression, without cloning.
    pub fn elements(&self) -> Option<&[RawExpr]> {
        match self.0.kind() {
            ExprKind::Normal(normal) => Some(normal.elements()),
            _ => None,
        }
    }

    /// Fully-qualified head of a normal expression (e.g. `System`List`).
    pub fn head(&self) -> Option<&str> {
        match self.0.kind() {
            ExprKind::Normal(normal) => match normal.head().kind() {
                ExprKind::Symbol(symbol) => Some(symbol.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<Vec<Expr>> {
        match self.0.kind() {
            ExprKind::Normal(normal) if normal.has_head(&Symbol::new(LIST)) => {
                Some(normal.elements().iter().cloned().map(Expr::from_raw).collect())
            }
            _ => None,
        }
    }

    /// Association entries from both the WXF `ExprKind::Association` form and
    /// the `System`Association[System`Rule[…], …]` form sent over WSTP.
    pub fn as_association(&self) -> Option<Vec<Rule>> {
        match self.0.kind() {
            ExprKind::Association(entries) => Some(
                entries
                    .iter()
                    .map(|entry| Rule::new(entry.key.clone(), entry.value.clone()))
                    .collect(),
            ),
            ExprKind::Normal(normal) if normal.has_head(&Symbol::new(ASSOCIATION)) => {
                rules_from_normal(normal)
            }
            _ => None,
        }
    }
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

impl From<(&str, Expr)> for Rule {
    fn from((key, value): (&str, Expr)) -> Self {
        Rule::new(key, value)
    }
}

impl From<RawExpr> for Expr {
    fn from(raw: RawExpr) -> Self {
        Expr(raw)
    }
}

impl From<Expr> for RawExpr {
    fn from(expr: Expr) -> Self {
        expr.0
    }
}

impl AsRef<RawExpr> for Expr {
    fn as_ref(&self) -> &RawExpr {
        &self.0
    }
}

impl From<&str> for Expr {
    fn from(value: &str) -> Self {
        Self::string(value)
    }
}

impl From<String> for Expr {
    fn from(value: String) -> Self {
        Self::string(value)
    }
}

impl From<i64> for Expr {
    fn from(value: i64) -> Self {
        RawExpr::from(value).into()
    }
}

impl From<i32> for Expr {
    fn from(value: i32) -> Self {
        RawExpr::from(i64::from(value)).into()
    }
}

impl From<f64> for Expr {
    fn from(value: f64) -> Self {
        RawExpr::real(value).into()
    }
}

impl From<bool> for Expr {
    fn from(value: bool) -> Self {
        Self::boolean(value)
    }
}

impl TryFrom<&Expr> for String {
    type Error = &'static str;

    fn try_from(expr: &Expr) -> Result<Self, Self::Error> {
        expr.as_str().map(str::to_owned).ok_or("not a WL String")
    }
}

impl TryFrom<&Expr> for bool {
    type Error = &'static str;

    fn try_from(expr: &Expr) -> Result<Self, Self::Error> {
        expr.as_bool().ok_or("not a WL True/False symbol")
    }
}

impl TryFrom<&Expr> for i64 {
    type Error = &'static str;

    fn try_from(expr: &Expr) -> Result<Self, Self::Error> {
        expr.as_i64().ok_or("not a WL Integer")
    }
}

/// Convert `Rule[a, b]` / `RuleDelayed[a, b]` children into `Rule` values.
pub(crate) fn rules_from_normal(normal: &Normal) -> Option<Vec<Rule>> {
    normal
        .elements()
        .iter()
        .map(|element| match element.kind() {
            ExprKind::Normal(rule)
                if (rule.has_head(&Symbol::new(RULE)) || rule.has_head(&Symbol::new(RULE_DELAYED)))
                    && rule.elements().len() == 2 =>
            {
                Some(Rule::new(rule.elements()[0].clone(), rule.elements()[1].clone()))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_trait_builds_scalar_expressions() {
        assert_eq!(Expr::from("text").as_str(), Some("text"));
        assert_eq!(Expr::from(42_i64).as_i64(), Some(42));
        assert_eq!(Expr::from(7_i32).as_i64(), Some(7));
        assert_eq!(Expr::from(true).as_bool(), Some(true));
        assert_eq!(Expr::from(false).raw().to_string(), "System`False");
    }

    #[test]
    fn try_from_extracts_scalar_expressions() {
        assert_eq!(String::try_from(&Expr::from("text")).unwrap(), "text");
        assert!(bool::try_from(&Expr::from("text")).is_err());
        assert_eq!(i64::try_from(&Expr::from(9)).unwrap(), 9);
    }

    #[test]
    fn list_and_association_round_trip() {
        let expr = Expr::association([
            Rule::new("name", "matheme"),
            Rule::new("pages", 2_i64),
            Rule::new("items", Expr::list([1_i64, 2, 3])),
            Rule::new("flag", true),
        ]);
        let rules = expr.as_association().expect("association");
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].key().as_str(), Some("name"));
        assert_eq!(rules[1].value().as_i64(), Some(2));
        assert_eq!(rules[2].value().as_list().expect("list").len(), 3);
        assert_eq!(rules[3].value().as_bool(), Some(true));
    }

    #[test]
    fn wstp_association_form_is_recognized() {
        let rules = Expr::normal(
            "System`Association",
            [Expr::normal("System`Rule", ["key", "value"])],
        );
        let parsed = rules.as_association().expect("WSTP association");
        assert_eq!(parsed[0].key().as_str(), Some("key"));
        assert_eq!(parsed[0].value().as_str(), Some("value"));
    }
}
