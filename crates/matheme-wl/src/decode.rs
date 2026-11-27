//! Exact decoders for Wolfram call arguments.
//!
//! Each decoder describes its accepted expression shape. Mapping a decoded
//! value preserves that shape, so an interface can generate option patterns
//! from the same declaration it uses to read arguments.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use crate::{Expr, Symbol};

pub trait Decoder: Sized {
    type Value;

    /// `None` means a shape mismatch. A matching shape can still be invalid.
    fn read(&self, expr: &Expr) -> Option<Result<Self::Value, String>>;
    fn pattern(&self) -> String;
    fn expected(&self) -> &'static str;

    fn decode(&self, expr: &Expr) -> Result<Self::Value, String> {
        self.read(expr)
            .unwrap_or_else(|| Err(self.expected().into()))
    }

    fn map<T>(self, map: impl Fn(Self::Value) -> T) -> impl Decoder<Value = T> {
        self.try_map(move |value| Ok(map(value)))
    }

    fn try_map<T>(self, map: impl Fn(Self::Value) -> Result<T, String>) -> impl Decoder<Value = T> {
        Map { base: self, map }
    }

    fn or(self, other: impl Decoder<Value = Self::Value>) -> impl Decoder<Value = Self::Value> {
        Either {
            first: self,
            second: other,
        }
    }

    /// Refines the pattern for a domain decoder, such as a Serde string enum.
    fn with_pattern(self, pattern: impl Fn() -> String) -> impl Decoder<Value = Self::Value> {
        WithPattern {
            base: self,
            pattern,
        }
    }
}

struct Map<D, F> {
    base: D,
    map: F,
}

impl<D: Decoder, T, F: Fn(D::Value) -> Result<T, String>> Decoder for Map<D, F> {
    type Value = T;

    fn read(&self, expr: &Expr) -> Option<Result<T, String>> {
        self.base
            .read(expr)
            .map(|result| result.and_then(&self.map))
    }

    fn pattern(&self) -> String {
        self.base.pattern()
    }

    fn expected(&self) -> &'static str {
        self.base.expected()
    }
}

struct Either<L, R> {
    first: L,
    second: R,
}

impl<L: Decoder, R: Decoder<Value = L::Value>> Decoder for Either<L, R> {
    type Value = L::Value;

    fn read(&self, expr: &Expr) -> Option<Result<Self::Value, String>> {
        self.first.read(expr).or_else(|| self.second.read(expr))
    }

    fn pattern(&self) -> String {
        format!("{} | {}", self.first.pattern(), self.second.pattern())
    }

    fn expected(&self) -> &'static str {
        self.second.expected()
    }
}

struct WithPattern<D, P> {
    base: D,
    pattern: P,
}

impl<D: Decoder, P: Fn() -> String> Decoder for WithPattern<D, P> {
    type Value = D::Value;

    fn read(&self, expr: &Expr) -> Option<Result<Self::Value, String>> {
        self.base.read(expr)
    }

    fn pattern(&self) -> String {
        (self.pattern)()
    }

    fn expected(&self) -> &'static str {
        self.base.expected()
    }
}

struct Atom<F, P> {
    take: F,
    pattern: P,
    expected: &'static str,
}

impl<T, F: Fn(&Expr) -> Option<T>, P: Fn() -> String> Decoder for Atom<F, P> {
    type Value = T;

    fn read(&self, expr: &Expr) -> Option<Result<T, String>> {
        (self.take)(expr).map(Ok)
    }

    fn pattern(&self) -> String {
        (self.pattern)()
    }

    fn expected(&self) -> &'static str {
        self.expected
    }
}

macro_rules! scalar_decoder {
    ($name:ident, $ty:ty, $pattern:literal, $expected:literal) => {
        pub fn $name() -> impl Decoder<Value = $ty> {
            Atom {
                take: |expr: &Expr| <$ty>::try_from(expr).ok(),
                pattern: || $pattern.into(),
                expected: $expected,
            }
        }
    };
}

scalar_decoder!(string, String, "_String", "expected a WL String");
scalar_decoder!(boolean, bool, "True | False", "expected True or False");
scalar_decoder!(
    integer,
    i64,
    "_Integer",
    "expected a signed 64-bit WL Integer"
);

pub fn unsigned() -> impl Decoder<Value = usize> {
    integer().try_map(|value| {
        if value < 0 {
            return Err("expected a non-negative Integer".into());
        }
        usize::try_from(value).map_err(|_| "Integer out of range".into())
    })
}

pub fn positive() -> impl Decoder<Value = NonZeroUsize> {
    unsigned().try_map(|value| {
        NonZeroUsize::new(value).ok_or_else(|| "expected a positive Integer".into())
    })
}

pub fn character() -> impl Decoder<Value = char> {
    string().try_map(|value| {
        let mut chars = value.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => Ok(ch),
            _ => Err("expected a single character".into()),
        }
    })
}

pub fn path() -> impl Decoder<Value = PathBuf> {
    string().map(PathBuf::from)
}

pub fn symbol(symbol: Symbol) -> impl Decoder<Value = ()> {
    let name = symbol.clone();
    Atom {
        pattern: move || name.as_str().trim_start_matches("System`").into(),
        take: move |expr: &Expr| expr.is_symbol(&symbol).then_some(()),
        expected: "expected a WL symbol",
    }
}

pub fn head(symbol: Symbol) -> impl Decoder<Value = Expr> {
    let name = symbol.clone();
    Atom {
        pattern: move || format!("_{}", name.as_str().trim_start_matches("System`")),
        take: move |expr: &Expr| {
            expr.as_normal().filter(|normal| normal.has_head(&symbol))?;
            Some(expr.clone())
        },
        expected: "expected a WL expression with the specified head",
    }
}

/// Accept any WL expression unchanged. Used by transports that pass an opaque
/// expression tree (for example the Box DSL) to a typed action.
pub fn any() -> impl Decoder<Value = Expr> {
    Atom {
        pattern: || "_".into(),
        take: |expr: &Expr| Some(expr.clone()),
        expected: "expected a WL expression",
    }
}

pub fn association() -> impl Decoder<Value = Expr> {
    Atom {
        pattern: || "_Association".into(),
        take: |expr: &Expr| expr.is_association().then(|| expr.clone()),
        expected: "expected an Association",
    }
}

pub fn list<T: Decoder>(item: T) -> impl Decoder<Value = Vec<T::Value>> {
    struct List<T>(T);

    impl<T: Decoder> Decoder for List<T> {
        type Value = Vec<T::Value>;

        fn read(&self, expr: &Expr) -> Option<Result<Self::Value, String>> {
            Some(
                expr.as_list()?
                    .iter()
                    .map(|item| self.0.decode(item))
                    .collect(),
            )
        }

        fn pattern(&self) -> String {
            format!("{{RepeatedNull[{}]}}", self.0.pattern())
        }

        fn expected(&self) -> &'static str {
            "expected a WL List"
        }
    }

    List(item)
}
