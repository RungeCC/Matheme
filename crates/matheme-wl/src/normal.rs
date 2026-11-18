//! Borrowed wrapper around Wolfram's `Normal` expression.
//!
//! Normal expressions (`head[args…]`) are the only composite shape WSTP sends,
//! so instead of matching `ExprKind::Normal` and calling through to the raw
//! type at every call site, [`Normal`] mirrors what [`crate::Expr`] does for
//! owned expressions: it hides `ExprKind` behind head/element accessors.

use std::fmt;

use wolfram_library_link::expr::{Expr as RawExpr, ExprKind, Normal as RawNormal};

use crate::expr::Expr;
use crate::symbol::Symbol;

/// A borrowed normal expression: a head plus its elements.
#[derive(Debug, Clone, Copy)]
pub struct Normal<'a> {
    raw: &'a RawNormal,
}

impl<'a> Normal<'a> {
    pub(crate) fn new(raw: &'a RawNormal) -> Self {
        Self { raw }
    }

    /// Borrow the normal expression inside `raw`, if it is one.
    pub(crate) fn from_expr(raw: &'a RawExpr) -> Option<Self> {
        match raw.kind() {
            ExprKind::Normal(normal) => Some(Self::new(normal)),
            _ => None,
        }
    }

    /// The underlying expression, e.g. to hand back to the WSTP layer.
    pub fn raw(self) -> &'a RawNormal {
        self.raw
    }

    /// Fully-qualified head name, when the head is a symbol.
    pub fn head(&self) -> Option<&'a str> {
        match self.raw.head().kind() {
            ExprKind::Symbol(symbol) => Some(symbol.as_str()),
            _ => None,
        }
    }

    /// Whether the head is exactly `head`.
    pub fn has_head(&self, head: &Symbol) -> bool {
        self.head() == Some(head.as_str())
    }

    /// The elements of the expression as borrowed [`Expr`] slices.
    pub fn elements(&self) -> &'a [Expr] {
        Expr::from_raw_slice(self.raw.elements())
    }

    /// Borrow a single element by index.
    pub fn get(&self, index: usize) -> Option<&'a Expr> {
        self.elements().get(index)
    }

    pub fn len(&self) -> usize {
        self.raw.elements().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for Normal<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_elements_are_exposed() {
        let expr = Expr::normal(Symbol::LIST, [1_i64, 2]);
        let normal = expr.as_normal().expect("normal expression");
        assert!(normal.has_head(&Symbol::LIST));
        assert_eq!(normal.head(), Some("System`List"));
        assert_eq!(normal.len(), 2);
        assert_eq!(normal.get(0).and_then(Expr::as_i64), Some(1));
    }
}
