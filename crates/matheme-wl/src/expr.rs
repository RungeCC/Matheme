//! Ergonomic wrapper around Wolfram Language expressions.
//!
//! [`Expr`] owns a raw expression but exposes construction through `From` and
//! inspection through typed accessors, so bridge code never matches on
//! `ExprKind` for everyday values. Composite shapes have their own wrappers:
//! [`Normal`] for `head[args…]` and [`Entry`] for association elements.

use wolfram_library_link::expr::{Expr as RawExpr, ExprKind, RuleEntry, Symbol as RawSymbol};

use crate::entry::{Entry, Rule};
use crate::normal::Normal;
use crate::symbol::Symbol;

/// An owned Wolfram Language expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr(RawExpr);

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

    pub fn symbol(symbol: impl Into<Symbol>) -> Self {
        RawExpr::new(ExprKind::Symbol(RawSymbol::new(symbol.into().as_str()))).into()
    }

    pub fn boolean(value: bool) -> Self {
        Self::symbol(if value { Symbol::TRUE } else { Symbol::FALSE })
    }

    pub fn null() -> Self {
        Self::symbol(Symbol::NULL)
    }

    /// A normal expression `head[elements…]`.
    pub fn normal(
        head: impl Into<Symbol>,
        elements: impl IntoIterator<Item = impl Into<Expr>>,
    ) -> Self {
        let head = RawExpr::new(ExprKind::Symbol(RawSymbol::new(head.into().as_str())));
        RawExpr::normal(
            head,
            elements.into_iter().map(|e| e.into().into_raw()).collect(),
        )
        .into()
    }

    pub fn list<T: Into<Expr>>(items: impl IntoIterator<Item = T>) -> Self {
        RawExpr::list(
            items
                .into_iter()
                .map(|item| item.into().into_raw())
                .collect(),
        )
        .into()
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

    /// Fully-qualified name, when this is a symbol.
    pub fn as_symbol(&self) -> Option<&str> {
        match self.0.kind() {
            ExprKind::Symbol(symbol) => Some(symbol.as_str()),
            _ => None,
        }
    }

    pub fn is_symbol(&self, symbol: &Symbol) -> bool {
        self.as_symbol() == Some(symbol.as_str())
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.as_symbol() {
            Some(name) if name == Symbol::TRUE.as_str() => Some(true),
            Some(name) if name == Symbol::FALSE.as_str() => Some(false),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self.0.kind() {
            ExprKind::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self.0.kind() {
            ExprKind::Real(value) => Some(value.into_inner()),
            _ => None,
        }
    }

    /// Bytes of a WXF `ByteArray`. Over WSTP the same value arrives as
    /// `ByteArray[{…}]`, which [`Expr::as_normal`] exposes instead.
    pub fn as_byte_array(&self) -> Option<&[u8]> {
        match self.0.kind() {
            ExprKind::ByteArray(bytes) => Some(bytes.as_slice()),
            _ => None,
        }
    }

    /// The normal expression behind this value, if it is compound.
    pub fn as_normal(&self) -> Option<Normal<'_>> {
        Normal::from_expr(&self.0)
    }

    pub fn as_list(&self) -> Option<Vec<Expr>> {
        let normal = self.as_normal()?;
        normal.has_head(&Symbol::LIST).then(|| {
            normal
                .elements()
                .iter()
                .cloned()
                .map(Expr::from_raw)
                .collect()
        })
    }

    /// Association entries from both the WXF `ExprKind::Association` form and
    /// the `System`Association[System`Rule[…], …]` form sent over WSTP.
    pub fn as_association(&self) -> Option<Vec<Entry>> {
        match self.0.kind() {
            ExprKind::Association(entries) => Some(
                entries
                    .iter()
                    .map(|entry: &RuleEntry| {
                        Entry::Rule(Rule::new(entry.key.clone(), entry.value.clone()))
                    })
                    .collect(),
            ),
            ExprKind::Normal(normal) => {
                let normal = Normal::new(normal);
                normal
                    .has_head(&Symbol::ASSOCIATION)
                    .then(|| crate::entry::parse(normal.elements()))
            }
            _ => None,
        }
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

impl From<Symbol> for Expr {
    fn from(value: Symbol) -> Self {
        Self::symbol(value)
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

impl<T: Into<Expr>> From<Option<T>> for Expr {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(inner) => inner.into(),
            None => Self::null(),
        }
    }
}

impl PartialEq<Symbol> for Expr {
    fn eq(&self, other: &Symbol) -> bool {
        self.is_symbol(other)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_trait_builds_scalar_expressions() {
        assert_eq!(Expr::from("text").as_str(), Some("text"));
        assert_eq!(Expr::from(42_i64).as_i64(), Some(42));
        assert_eq!(Expr::from(7_i32).as_i64(), Some(7));
        assert_eq!(Expr::from(2.5_f64).as_f64(), Some(2.5));
        assert_eq!(Expr::from(true).as_bool(), Some(true));
        assert_eq!(Expr::from(false).raw().to_string(), "System`False");
        assert_eq!(Expr::from(Some("ok")).as_str(), Some("ok"));
        assert_eq!(Expr::from(Option::<&str>::None), Symbol::NULL);
        assert_eq!(Expr::from(Some(9_i64)).as_i64(), Some(9));
    }

    #[test]
    fn symbols_compare_through_the_wrapper() {
        assert!(Expr::from(Symbol::NULL).is_symbol(&Symbol::NULL));
        assert_eq!(Expr::from(Symbol::TRUE), Symbol::TRUE);
        assert_ne!(Expr::from(Symbol::FALSE), Symbol::TRUE);
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
        assert_eq!(rules[0].key().and_then(Expr::as_str), Some("name"));
        assert_eq!(rules[1].pair().expect("rule").1.as_i64(), Some(2));
        assert_eq!(rules[2].pair().expect("rule").1.as_list().unwrap().len(), 3);
        assert_eq!(rules[3].pair().expect("rule").1.as_bool(), Some(true));
    }

    #[test]
    fn wstp_association_form_is_recognized() {
        let rules = Expr::normal(
            Symbol::ASSOCIATION,
            [Expr::normal(Symbol::RULE, ["key", "value"])],
        );
        let parsed = rules.as_association().expect("WSTP association");
        let (key, value) = parsed[0].pair().expect("immediate rule");
        assert_eq!(key.as_str(), Some("key"));
        assert_eq!(value.as_str(), Some("value"));
    }
}
