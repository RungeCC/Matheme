//! Declarative, `clap`-like argument and action parsing for Wolfram Language
//! associations.
//!
//! Instead of hand-writing key loops and dispatch tables per endpoint, each
//! action declares its schema with [`wl_args!`] and registers itself in the
//! top-level [`wl_actions!`] enum:
//!
//! - [`FromExpr`] acts like `clap`'s `ValueParser`, decoding a single WL
//!   [`Expr`] into a typed Rust value.
//! - [`FromArgs`] parses an entire argument association in a single pass,
//!   checking key names *before* values, rejecting unknown keys, duplicate keys,
//!   and `RuleDelayed` entries uniformly, and attaching `<action>.<key>` context
//!   to every value error.
//! - [`wl_actions!`] acts like `clap::Subcommand`, serving as the single source
//!   of truth for action names, host-font requirements, argument parsing, and
//!   dispatch.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use matheme_wl::{Entry, Expr, Symbol};
use matheme_world::settings::Update;

use crate::call::{self, Failure};

/// Decode a single Wolfram Language expression into a typed Rust value.
pub trait FromExpr: Sized {
    fn from_expr(expr: &Expr) -> Result<Self, String>;

    /// Wolfram pattern for the accepted wire types. Constraints stay in the decoder.
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String;
}

/// Decode a Wolfram Language argument association into a structured input type.
pub trait FromArgs: Sized {
    fn parse_args(entries: &[Entry<'_>]) -> Result<Self, Failure>;

    #[cfg(any(test, feature = "option-schema"))]
    fn wl_arguments() -> Vec<(&'static str, String)>;
}

/// Ensure an action taking no arguments was given an empty Association.
pub fn ensure_empty(entries: &[Entry<'_>], action: &str) -> Result<(), Failure> {
    if entries.is_empty() {
        Ok(())
    } else {
        Err(call::error(format!("{action} takes no arguments")))
    }
}

/// Parse a named enumeration value from a WL String.
pub fn named<T>(expr: &Expr, parse: impl Fn(&str) -> Option<T>, what: &str) -> Result<T, String> {
    let name = String::from_expr(expr)?;
    parse(&name).ok_or_else(|| format!("unknown {what}: {name:?}"))
}

// ---------------------------------------------------------------------------
// Core `FromExpr` implementations
// ---------------------------------------------------------------------------

impl FromExpr for String {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        "_String".into()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        expr.as_str()
            .map(str::to_owned)
            .ok_or_else(|| "expected a WL String".into())
    }
}

impl FromExpr for bool {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        "True | False".into()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        expr.as_bool()
            .ok_or_else(|| "expected True or False".into())
    }
}

impl FromExpr for i64 {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        "_Integer".into()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        expr.as_i64()
            .ok_or_else(|| "expected a WL Integer".into())
    }
}

impl FromExpr for usize {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        i64::wl_pattern()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        let value = i64::from_expr(expr)?;
        if value < 0 {
            return Err("expected a non-negative Integer".into());
        }
        usize::try_from(value).map_err(|_| "Integer out of range".into())
    }
}

impl FromExpr for NonZeroUsize {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        i64::wl_pattern()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        let value = i64::from_expr(expr)?;
        if value < 1 {
            return Err("expected a positive Integer".into());
        }
        usize::try_from(value)
            .ok()
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| "Integer out of range".into())
    }
}

impl FromExpr for char {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        String::wl_pattern()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        let text = String::from_expr(expr)?;
        let mut chars = text.chars();
        let first = chars
            .next()
            .ok_or_else(|| "expected a non-empty String".to_owned())?;
        if chars.next().is_some() {
            return Err("expected a single character".into());
        }
        Ok(first)
    }
}

impl FromExpr for PathBuf {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        String::wl_pattern()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        String::from_expr(expr).map(PathBuf::from)
    }
}

impl<T: FromExpr> FromExpr for Vec<T> {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        format!("{{RepeatedNull[{}]}}", T::wl_pattern())
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        let items = expr
            .as_list()
            .ok_or_else(|| "expected a WL List".to_owned())?;
        items.iter().map(T::from_expr).collect()
    }
}

impl<T: FromExpr> FromExpr for Option<T> {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        T::wl_pattern()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        T::from_expr(expr).map(Some)
    }
}

// ---------------------------------------------------------------------------
// Reusable wrapper types for WL-specific argument idioms
// ---------------------------------------------------------------------------

/// Either ``System`All`` (represented as `None`) or a typed value `Some(T)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllOr<T>(pub Option<T>);

impl<T: FromExpr> FromExpr for AllOr<T> {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        format!("All | {}", T::wl_pattern())
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::ALL) {
            Ok(Self(None))
        } else {
            T::from_expr(expr).map(|value| Self(Some(value)))
        }
    }
}

/// An owned `Expr` guaranteed to be a WL Association.
#[derive(Debug, Clone)]
pub struct AssocExpr(pub Expr);

impl FromExpr for AssocExpr {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        "_Association".into()
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.as_association().is_none() {
            return Err("expected an Association".into());
        }
        Ok(Self(expr.clone()))
    }
}

/// A settings patch field that resets to default on `Null`, otherwise sets `T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resettable<T>(pub Update<T>);

impl<T> Default for Resettable<T> {
    fn default() -> Self {
        Self(Update::Keep)
    }
}

impl<T: FromExpr> FromExpr for Resettable<T> {
    #[cfg(any(test, feature = "option-schema"))]
    fn wl_pattern() -> String {
        format!("Null | {}", T::wl_pattern())
    }

    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::NULL) {
            Ok(Self(Update::Reset))
        } else {
            T::from_expr(expr).map(|v| Self(Update::Set(v)))
        }
    }
}

// ---------------------------------------------------------------------------
// Declarative macros (`wl_args!` and `wl_actions!`)
// ---------------------------------------------------------------------------

/// Define a typed argument struct and its single-pass [`FromArgs`] parser.
///
/// Fields without `= default` are required; fields with `= default` are
/// optional and fall back to the default expression when omitted.
#[macro_export]
macro_rules! wl_args {
    (
        #[action = $action:literal]
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $key:literal => $fvis:vis $field:ident : $ty:ty $(= $default:expr)?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $( $fvis $field: $ty, )*
        }

        impl $crate::args::FromArgs for $name {
            #[cfg(any(test, feature = "option-schema"))]
            fn wl_arguments() -> Vec<(&'static str, String)> {
                vec![$(($key, <$ty as $crate::args::FromExpr>::wl_pattern())),*]
            }

            fn parse_args(
                entries: &[matheme_wl::Entry<'_>],
            ) -> Result<Self, $crate::call::Failure> {
                $( let mut $field: Option<$ty> = None; )*

                for entry in entries {
                    let (key_expr, value_expr) = entry.pair().map_err($crate::call::error)?;
                    let key = key_expr.as_str().ok_or_else(|| {
                        $crate::call::error(format!(
                            "association key must be a String, got: {key_expr}"
                        ))
                    })?;
                    match key {
                        $(
                            $key => {
                                if $field.is_some() {
                                    return Err($crate::call::error(format!(
                                        "duplicate {} key: {}",
                                        $action, key
                                    )));
                                }
                                let parsed = <$ty as $crate::args::FromExpr>::from_expr(value_expr)
                                    .map_err(|reason| {
                                        $crate::call::error(format!(
                                            "{}.{}: {}",
                                            $action, key, reason
                                        ))
                                    })?;
                                $field = Some(parsed);
                            }
                        )*
                        other => {
                            return Err($crate::call::error(format!(
                                "unknown {} key: {}",
                                $action, other
                            )));
                        }
                    }
                }

                Ok(Self {
                    $(
                        $field: $crate::wl_args!(@finish $action, $key, $field $(, $default)?),
                    )*
                })
            }
        }
    };

    (@finish $action:literal, $key:literal, $field:ident) => {
        $field.ok_or_else(|| {
            $crate::call::error(format!("missing required {} key: {}", $action, $key))
        })?
    };
    (@finish $action:literal, $key:literal, $field:ident, $default:expr) => {
        $field.unwrap_or_else(|| $default)
    };
}

/// Define the top-level action enum, unifying action names, host-font
/// requirements, argument parsing, and handler dispatch in one declaration.
#[macro_export]
macro_rules! wl_actions {
    (
        $vis:vis enum $enum_name:ident {
            $(
                $action_name:literal $([$flag:ident])? => $variant:ident $(($args:ty))? => $handler:path
            ),* $(,)?
        }
    ) => {
        $vis enum $enum_name {
            $(
                $variant $(($args))?,
            )*
        }

        impl $enum_name {
            #[cfg(any(test, feature = "option-schema"))]
            pub fn wl_actions() -> impl Iterator<Item = (&'static str, Vec<(&'static str, String)>)> {
                [
                    $(
                        ($action_name, $crate::wl_actions!(@wl_args $($args)?)),
                    )*
                ].into_iter()
            }

            #[allow(dead_code)]
            pub const ALL_NAMES: &'static [&'static str] = &[
                $( $action_name, )*
            ];

            pub fn parse(
                action: &str,
                entries: &[matheme_wl::Entry<'_>],
            ) -> Result<Self, $crate::call::Failure> {
                match action {
                    $(
                        $action_name => $crate::wl_actions!(@parse $action_name, $variant $(, $args)? ; entries),
                    )*
                    other => Err($crate::call::library_error(format!(
                        "unknown action: {other}"
                    ))),
                }
            }

            pub const fn needs_host_fonts(&self) -> bool {
                match self {
                    $(
                        $crate::wl_actions!(@pat $variant $(, $args)?) => {
                            $crate::wl_actions!(@host_fonts $($flag)?)
                        }
                    )*
                }
            }

            pub fn execute(self) -> $crate::call::Answer {
                match self {
                    $(
                        $crate::wl_actions!(@pat_bind $variant, input $(, $args)?) => {
                            $crate::wl_actions!(@call $handler, input $(, $args)?)
                        }
                    )*
                }
            }
        }
    };

    (@wl_args $args:ty) => {
        <$args as $crate::args::FromArgs>::wl_arguments()
    };
    (@wl_args) => {
        Vec::new()
    };

    (@parse $action_name:literal, $variant:ident, $args:ty ; $entries:ident) => {
        <$args as $crate::args::FromArgs>::parse_args($entries).map(Self::$variant)
    };
    (@parse $action_name:literal, $variant:ident ; $entries:ident) => {
        $crate::args::ensure_empty($entries, $action_name).map(|()| Self::$variant)
    };

    (@pat $variant:ident, $args:ty) => {
        Self::$variant(_)
    };
    (@pat $variant:ident) => {
        Self::$variant
    };

    (@pat_bind $variant:ident, $input:ident, $args:ty) => {
        Self::$variant($input)
    };
    (@pat_bind $variant:ident, $input:ident) => {
        Self::$variant
    };

    (@host_fonts no_host_fonts) => {
        false
    };
    (@host_fonts) => {
        true
    };

    (@call $handler:path, $input:ident, $args:ty) => {
        $handler($input)
    };
    (@call $handler:path, $input:ident) => {
        $handler()
    };
}
