//! Fully-qualified Wolfram Language symbols.
//!
//! The bridge talks to the kernel through strings, so every symbol it cares
//! about would otherwise be a bare string literal at the call site. [`Symbol`]
//! gives those names a home: the common `System`` symbols are constants, and
//! [`Symbol::system`] builds one for the occasional dynamic case.
//!
//! ```
//! # use matheme_wl::Symbol;
//! assert_eq!(Symbol::TRUE.as_str(), "System`True");
//! assert_eq!(Symbol::system("List"), Symbol::LIST);
//! ```

use std::borrow::{Borrow, Cow};
use std::fmt;

/// A fully-qualified Wolfram Language symbol name, e.g. `System`List`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(Cow<'static, str>);

impl Symbol {
    /// Wrap an already fully-qualified name.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    /// A symbol in the `System`` context, e.g. `Symbol::system("List")`.
    pub fn system(name: &str) -> Self {
        Self(Cow::Owned(format!("System`{name}")))
    }

    /// The fully-qualified name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `System`True`.
    pub const TRUE: Self = Self(Cow::Borrowed("System`True"));
    /// `System`False`.
    pub const FALSE: Self = Self(Cow::Borrowed("System`False"));
    /// `System`Null`.
    pub const NULL: Self = Self(Cow::Borrowed("System`Null"));
    /// `System`None`, the WL spelling of "no value" used by some APIs.
    pub const NONE: Self = Self(Cow::Borrowed("System`None"));
    /// `System`Automatic`, the WL spelling of Typst's `auto`.
    pub const AUTOMATIC: Self = Self(Cow::Borrowed("System`Automatic"));
    /// `System`All`, used for an unbounded query result.
    pub const ALL: Self = Self(Cow::Borrowed("System`All"));
    /// `System`Indeterminate`, used for non-finite `NaN` floats.
    pub const INDETERMINATE: Self = Self(Cow::Borrowed("System`Indeterminate"));
    /// `System`DirectedInfinity`, used for `±inf` floats.
    pub const DIRECTED_INFINITY: Self = Self(Cow::Borrowed("System`DirectedInfinity"));
    /// `System`List`.
    pub const LIST: Self = Self(Cow::Borrowed("System`List"));
    /// `System`Association`.
    pub const ASSOCIATION: Self = Self(Cow::Borrowed("System`Association"));
    /// `System`Rule` — the immediate rule `key -> value`.
    pub const RULE: Self = Self(Cow::Borrowed("System`Rule"));
    /// `System`RuleDelayed` — the delayed rule `key :> value`.
    pub const RULE_DELAYED: Self = Self(Cow::Borrowed("System`RuleDelayed"));
    /// `System`ByteArray`.
    pub const BYTE_ARRAY: Self = Self(Cow::Borrowed("System`ByteArray"));
    /// `System`Quantity`.
    pub const QUANTITY: Self = Self(Cow::Borrowed("System`Quantity"));
    /// `System`RGBColor`.
    pub const RGB_COLOR: Self = Self(Cow::Borrowed("System`RGBColor"));
    /// `System`DateObject`.
    pub const DATE_OBJECT: Self = Self(Cow::Borrowed("System`DateObject"));
    /// `System`TimeObject`.
    pub const TIME_OBJECT: Self = Self(Cow::Borrowed("System`TimeObject"));
    /// `System`Span`.
    pub const SPAN: Self = Self(Cow::Borrowed("System`Span"));
    /// `System`ReturnPacket`, the head wrapping a kernel evaluation result.
    pub const RETURN_PACKET: Self = Self(Cow::Borrowed("System`ReturnPacket"));
    /// `System`$InstallationDirectory`.
    pub const INSTALLATION_DIRECTORY: Self = Self(Cow::Borrowed("System`$InstallationDirectory"));
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for Symbol {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Symbol {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Symbol {
    fn from(name: &str) -> Self {
        Self::new(name.to_owned())
    }
}

impl From<String> for Symbol {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_symbols_are_fully_qualified() {
        assert_eq!(Symbol::TRUE.as_str(), "System`True");
        assert_eq!(Symbol::system("List"), Symbol::LIST);
        assert_eq!(Symbol::from("System`Null"), Symbol::NULL);
    }
}
