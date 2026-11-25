//! Declarative, `clap`-like argument and action parsing for Wolfram Language
//! associations.
//!
//! Instead of hand-writing key loops and dispatch tables per endpoint, each
//! action declares its schema with [`wl_args!`] and registers itself in the
//! top-level [`wl_actions!`] enum:
//!
//! - Exact decoders from `matheme_wl::decode` read argument values and
//!   describe their Wolfram expression shapes.
//! - [`FromArgs`] parses an entire argument association in a single pass,
//!   checking key names *before* values, rejecting unknown keys, duplicate keys,
//!   and `RuleDelayed` entries uniformly, and attaching `<action>.<key>` context
//!   to every value error.
//! - [`wl_actions!`] acts like `clap::Subcommand`, serving as the single source
//!   of truth for action names, host-font requirements, argument parsing, and
//!   dispatch.

use matheme_wl::Entry;

use crate::call::{self, Failure};

/// Decode a Wolfram Language argument association into a structured input type.
pub trait FromArgs: Sized {
    fn parse_args(entries: &[Entry<'_>]) -> Result<Self, Failure>;

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

pub(crate) mod schema {
    use matheme_wl::decode::{self, Decoder};
    use serde::de::{self, DeserializeOwned, Visitor, value};

    pub fn parse_name<T: DeserializeOwned>(name: &str, kind: &str) -> Result<T, String> {
        T::deserialize(value::StrDeserializer::<value::Error>::new(
            &name.to_ascii_lowercase(),
        ))
        .map_err(|_| format!("unknown {kind}: {name:?}"))
    }

    pub fn enumeration<T: DeserializeOwned>(
        parse: impl Fn(&str) -> Result<T, String>,
    ) -> impl Decoder<Value = T> {
        enumeration_with(parse, |deserializer| T::deserialize(deserializer))
    }

    pub fn enumeration_with<T>(
        parse: impl Fn(&str) -> Result<T, String>,
        deserialize: impl Fn(EnumNames<'_>) -> Result<T, value::Error>,
    ) -> impl Decoder<Value = T> {
        decode::string()
            .try_map(move |name| parse(&name))
            .with_pattern(move || enum_pattern_with(&deserialize))
    }

    fn enum_pattern_with<T>(
        deserialize: impl FnOnce(EnumNames<'_>) -> Result<T, value::Error>,
    ) -> String {
        let mut names = &[][..];
        deserialize(EnumNames(&mut names)).expect("expected a Serde enum with unit variants");
        let alternatives = names
            .iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("enumOption[{{{alternatives}}}]")
    }

    pub struct EnumNames<'a>(&'a mut &'static [&'static str]);

    impl<'de> de::Deserializer<'de> for EnumNames<'_> {
        type Error = value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(de::Error::custom("expected a Serde enum"))
        }

        fn deserialize_enum<V: Visitor<'de>>(
            self,
            _: &'static str,
            variants: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            *self.0 = variants;
            let first = variants
                .first()
                .ok_or_else(|| de::Error::custom("enum has no variants"))?;
            visitor.visit_enum(value::StrDeserializer::new(first))
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map struct identifier ignored_any
        }
    }
}

// ---------------------------------------------------------------------------
// Declarative macros (`wl_args!` and `wl_actions!`)
// ---------------------------------------------------------------------------

/// Define a typed argument struct and its single-pass [`FromArgs`] parser.
///
/// Each bracketed decoder reads the value and supplies its Wolfram pattern.
/// Fields without `= default` are required; fields with `= default` are
/// optional and fall back to the default expression when omitted.
#[macro_export]
macro_rules! wl_args {
    (
        #[action = $action:literal]
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $key:literal => $fvis:vis $field:ident : $ty:ty [$decoder:expr] $(= $default:expr)?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $( $fvis $field: $ty, )*
        }

        impl $crate::args::FromArgs for $name {
            fn wl_arguments() -> Vec<(&'static str, String)> {
                vec![$(($key, matheme_wl::decode::Decoder::pattern(&$decoder))),*]
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
                                let parsed = matheme_wl::decode::Decoder::decode(&$decoder, value_expr)
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
