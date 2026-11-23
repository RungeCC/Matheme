//! Ergonomic wrapper around Wolfram Language expressions.
//!
//! [`Expr`] is a `#[repr(transparent)]` wrapper around `wolfram_library_link`'s
//! raw expression, exposing zero-cost borrowed accessors (`as_*`), `From` /
//! `TryFrom` conversions, and bidirectional Typst conversion via
//! [`Expr::to_typst`] and [`Expr::from_typst`].

use std::borrow::{Borrow, Cow};
use std::fmt;

use typst::foundations::Value;
use wolfram_library_link::expr::{
    Expr as RawExpr, ExprKind, NumericArray, RuleEntry, Symbol as RawSymbol,
};

use crate::convert;
use crate::entry::{Entry, Rule};
use crate::normal::Normal;
use crate::symbol::Symbol;

/// An owned Wolfram Language expression.
#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr(RawExpr);

impl Expr {
    /// Wrap an owned expression produced by the Wolfram expression library.
    pub fn from_raw(raw: RawExpr) -> Self {
        Self(raw)
    }

    /// Borrow a raw expression as an [`Expr`] without cloning.
    pub fn from_raw_ref(raw: &RawExpr) -> &Self {
        // SAFETY: `Expr` is `#[repr(transparent)]` over `RawExpr`.
        unsafe { &*(raw as *const RawExpr as *const Self) }
    }

    /// Borrow a slice of raw expressions as `&[Expr]` without cloning.
    pub fn from_raw_slice(raw: &[RawExpr]) -> &[Self] {
        // SAFETY: `Expr` is `#[repr(transparent)]` over `RawExpr`, so `[Expr]`
        // and `[RawExpr]` have identical element layout and alignment.
        unsafe { &*(raw as *const [RawExpr] as *const [Self]) }
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

    pub fn symbol(symbol: impl Borrow<Symbol>) -> Self {
        RawExpr::new(ExprKind::Symbol(RawSymbol::new(symbol.borrow().as_str()))).into()
    }

    pub fn boolean(value: bool) -> Self {
        Self::symbol(if value { Symbol::TRUE } else { Symbol::FALSE })
    }

    pub fn null() -> Self {
        Self::symbol(Symbol::NULL)
    }

    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        RawExpr::new(ExprKind::ByteArray(bytes.into())).into()
    }

    /// A packed ``System`NumericArray`` of `"UnsignedInteger8"` elements with
    /// the given `dimensions`.
    ///
    /// Over WSTP (`Link::put_expr`), `NumericArray` serializes directly via WXF
    /// (`BinaryDeserialize[ByteArray[...]]`) in a single contiguous byte-array
    /// transfer rather than constructing per-element `List` expressions.
    pub fn numeric_array_u8(dimensions: &[usize], data: &[u8]) -> Self {
        let expected: usize = dimensions.iter().copied().product();
        assert_eq!(
            data.len(),
            expected,
            "NumericArray byte length ({}) does not match product of dimensions ({dimensions:?} -> {expected})",
            data.len()
        );
        RawExpr::new(ExprKind::NumericArray(NumericArray::from_slice::<u8>(
            dimensions.to_vec(),
            data,
        )))
        .into()
    }

    /// A normal expression `head[elements…]`.
    pub fn normal(
        head: impl Borrow<Symbol>,
        elements: impl IntoIterator<Item = impl Into<Expr>>,
    ) -> Self {
        let head = RawExpr::new(ExprKind::Symbol(RawSymbol::new(head.borrow().as_str())));
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

    /// Convert this expression into a Typst [`Value`], collecting warnings for
    /// unsupported nested values.
    pub fn to_typst(&self, warnings: &mut Vec<String>) -> Value {
        convert::to_typst(self, warnings)
    }

    /// Construct a Wolfram expression from a Typst [`Value`], collecting
    /// warnings for opaque or lossy conversions.
    pub fn from_typst(value: &Value, warnings: &mut Vec<String>) -> Self {
        convert::from_typst(value, warnings)
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

    /// Bytes of a WXF `ByteArray`. Prefer [`Expr::to_bytes`] when callers need
    /// to accept both WXF and WSTP (`ByteArray[{…}]`) wire forms.
    pub fn as_byte_array(&self) -> Option<&[u8]> {
        match self.0.kind() {
            ExprKind::ByteArray(bytes) => Some(bytes.as_slice()),
            _ => None,
        }
    }

    /// Borrow the underlying [`NumericArray`] if this expression is one.
    pub fn as_numeric_array(&self) -> Option<&NumericArray> {
        match self.0.kind() {
            ExprKind::NumericArray(arr) => Some(arr),
            _ => None,
        }
    }

    /// Extract raw bytes from either a WXF `ExprKind::ByteArray` (zero-copy) or
    /// the WSTP ``System`ByteArray[{b1, b2, …}]`` wire form.
    ///
    /// Returns:
    /// - `Ok(None)` if this expression is not a `ByteArray`;
    /// - `Ok(Some(bytes))` if it is a valid `ByteArray`;
    /// - `Err(reason)` if it has head ``System`ByteArray`` but malformed contents.
    pub fn to_bytes(&self) -> Result<Option<Cow<'_, [u8]>>, String> {
        if let Some(bytes) = self.as_byte_array() {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        let Some(normal) = self.as_normal().filter(|n| n.has_head(&Symbol::BYTE_ARRAY)) else {
            return Ok(None);
        };
        let [list] = normal.elements() else {
            return Err("ByteArray expects a single List of byte integers".into());
        };
        let Some(elements) = list.as_list() else {
            return Err("ByteArray expects a List of byte integers".into());
        };
        let mut bytes = Vec::with_capacity(elements.len());
        for (index, element) in elements.iter().enumerate() {
            let Some(value) = element.as_i64() else {
                return Err(format!("ByteArray element {index} is not an Integer"));
            };
            match u8::try_from(value) {
                Ok(byte) => bytes.push(byte),
                Err(_) => {
                    return Err(format!(
                        "ByteArray element {index} ({value}) is out of byte range"
                    ));
                }
            }
        }
        Ok(Some(Cow::Owned(bytes)))
    }

    /// The normal expression behind this value, if it is compound.
    pub fn as_normal(&self) -> Option<Normal<'_>> {
        Normal::from_expr(&self.0)
    }

    /// Zero-copy slice of elements if this expression is a ``System`List``.
    pub fn as_list(&self) -> Option<&[Expr]> {
        let normal = self.as_normal()?;
        normal.has_head(&Symbol::LIST).then(|| normal.elements())
    }

    /// Borrowed association entries from both the WXF `ExprKind::Association`
    /// form and the ``System`Association[System`Rule[…], …]`` form sent over WSTP.
    pub fn as_association(&self) -> Option<Vec<Entry<'_>>> {
        match self.0.kind() {
            ExprKind::Association(entries) => Some(
                entries
                    .iter()
                    .map(|entry: &RuleEntry| {
                        if entry.delayed {
                            Entry::RuleDelayed {
                                key: Self::from_raw_ref(&entry.key),
                            }
                        } else {
                            Entry::Rule {
                                key: Self::from_raw_ref(&entry.key),
                                value: Self::from_raw_ref(&entry.value),
                            }
                        }
                    })
                    .collect(),
            ),
            ExprKind::Normal(normal) => {
                let normal = Normal::new(normal);
                normal
                    .has_head(&Symbol::ASSOCIATION)
                    .then(|| crate::entry::parse_slice(normal.elements()))
            }
            _ => None,
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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

impl From<&Symbol> for Expr {
    fn from(value: &Symbol) -> Self {
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

impl From<u32> for Expr {
    fn from(value: u32) -> Self {
        RawExpr::from(i64::from(value)).into()
    }
}

impl From<f64> for Expr {
    fn from(value: f64) -> Self {
        if value.is_finite() {
            RawExpr::real(value).into()
        } else if value.is_nan() {
            Self::symbol(&Symbol::INDETERMINATE)
        } else if value.is_sign_positive() {
            Self::normal(&Symbol::DIRECTED_INFINITY, [1_i64])
        } else {
            Self::normal(&Symbol::DIRECTED_INFINITY, [-1_i64])
        }
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

impl<'a> TryFrom<&'a Expr> for &'a str {
    type Error = &'static str;

    fn try_from(expr: &'a Expr) -> Result<Self, Self::Error> {
        expr.as_str().ok_or("not a WL String")
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

impl TryFrom<&Expr> for f64 {
    type Error = &'static str;

    fn try_from(expr: &Expr) -> Result<Self, Self::Error> {
        expr.as_f64().ok_or("not a WL Real")
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
        assert_eq!(Expr::from(9_u32).as_i64(), Some(9));
        assert_eq!(Expr::from(2.5_f64).as_f64(), Some(2.5));
        assert_eq!(Expr::from(true).as_bool(), Some(true));
        assert_eq!(Expr::from(false).to_string(), "System`False");
        assert_eq!(Expr::from(Some("ok")).as_str(), Some("ok"));
        assert_eq!(Expr::from(Option::<&str>::None), Symbol::NULL);
        assert_eq!(Expr::from(Some(9_i64)).as_i64(), Some(9));
    }

    #[test]
    fn non_finite_floats_map_without_panicking() {
        assert_eq!(Expr::from(f64::NAN), Symbol::INDETERMINATE);

        let pos_inf = Expr::from(f64::INFINITY);
        let pos = pos_inf.as_normal().expect("DirectedInfinity[1]");
        assert!(pos.has_head(&Symbol::DIRECTED_INFINITY));
        assert_eq!(pos.get(0).and_then(Expr::as_i64), Some(1));

        let neg_inf = Expr::from(f64::NEG_INFINITY);
        let neg = neg_inf.as_normal().expect("DirectedInfinity[-1]");
        assert!(neg.has_head(&Symbol::DIRECTED_INFINITY));
        assert_eq!(neg.get(0).and_then(Expr::as_i64), Some(-1));
    }

    #[test]
    fn symbols_compare_through_the_wrapper() {
        assert!(Expr::from(Symbol::NULL).is_symbol(&Symbol::NULL));
        assert_eq!(Expr::from(&Symbol::TRUE), Symbol::TRUE);
        assert_ne!(Expr::from(Symbol::FALSE), Symbol::TRUE);
    }

    #[test]
    fn try_from_extracts_scalar_expressions() {
        assert_eq!(<&str>::try_from(&Expr::from("text")).unwrap(), "text");
        assert_eq!(String::try_from(&Expr::from("text")).unwrap(), "text");
        assert!(bool::try_from(&Expr::from("text")).is_err());
        assert_eq!(i64::try_from(&Expr::from(9)).unwrap(), 9);
        assert_eq!(f64::try_from(&Expr::from(1.25)).unwrap(), 1.25);
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

    #[test]
    fn to_bytes_handles_wxf_and_wstp_uniformly() {
        let wxf = Expr::bytes(vec![10, 20, 30]);
        assert_eq!(wxf.to_bytes().unwrap().as_deref(), Some(&[10, 20, 30][..]));

        let wstp = Expr::normal(Symbol::BYTE_ARRAY, [Expr::list([10_i64, 20, 30])]);
        assert_eq!(wstp.to_bytes().unwrap().as_deref(), Some(&[10, 20, 30][..]));

        assert_eq!(Expr::from("not bytes").to_bytes().unwrap(), None);

        let bad = Expr::normal(Symbol::BYTE_ARRAY, [Expr::list([300_i64])]);
        assert!(bad.to_bytes().is_err());
    }

    #[test]
    fn wxf_association_preserves_delayed_rules() {
        let expr = Expr::from_raw(RawExpr::new(ExprKind::Association(vec![
            RuleEntry::rule(RawExpr::from("a"), RawExpr::from(1_i64)),
            RuleEntry::rule_delayed(RawExpr::from("b"), RawExpr::from(2_i64)),
        ])));
        let entries = expr.as_association().expect("WXF association");
        assert!(entries[0].pair().is_ok());
        assert!(matches!(entries[1], Entry::RuleDelayed { .. }));
    }

    #[test]
    fn numeric_array_u8_round_trips_dimensions_and_bytes() {
        let pixels = [255_u8, 0, 128, 255, 10, 20, 30, 40];
        let expr = Expr::numeric_array_u8(&[1, 2, 4], &pixels);
        let na = expr.as_numeric_array().expect("NumericArray");
        assert_eq!(na.dimensions(), &[1, 2, 4]);
        assert_eq!(na.try_as_slice::<u8>(), Some(pixels.as_slice()));
    }
}
