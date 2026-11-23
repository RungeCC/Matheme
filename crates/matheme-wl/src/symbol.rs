//! Fully-qualified Wolfram Language symbols, organized by domain.
//!
//! All built-in symbol constants are zero-allocation compile-time `const`
//! values (``Cow::Borrowed("System`...")``), grouped into:
//!
//! - [`fundamental`]: core Wolfram Language data types, sentinels, containers,
//!   rules, and native value heads mapped by [`crate::to_typst`] /
//!   [`crate::from_typst`].
//! - [`graphics`]: Wolfram Language ``System`Graphics`` primitives, styling
//!   directives, options, and alignment anchors.
//!
//! Transport-specific symbols (such as WSTP packet heads or kernel globals)
//! belong in `matheme-wstp`, not here.
//!
//! ```
//! # use matheme_wl::{fundamental, graphics, Symbol};
//! assert_eq!(fundamental::TRUE.as_str(), "System`True");
//! assert_eq!(graphics::FILLED_CURVE.as_str(), "System`FilledCurve");
//! assert_eq!(Symbol::system("List"), fundamental::LIST);
//! ```

use std::borrow::{Borrow, Cow};
use std::fmt;

/// A fully-qualified Wolfram Language symbol name, e.g. ``System`List``.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(Cow<'static, str>);

impl Symbol {
    /// Wrap a compile-time `'static` fully-qualified symbol name without allocating.
    pub const fn from_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// Wrap an already fully-qualified name.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    /// Build a dynamic symbol in the `` System` `` context, e.g. `Symbol::system("List")`.
    ///
    /// Prefer constants from [`fundamental`] or [`graphics`] when the symbol is
    /// statically known.
    pub fn system(name: &str) -> Self {
        Self(Cow::Owned(format!("System`{name}")))
    }

    /// The fully-qualified name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The unqualified short name (after the final `` ` ``), if any.
    pub fn short_name(&self) -> &str {
        self.0.rsplit_once('`').map_or(&self.0, |(_, short)| short)
    }

    // Convenience aliases for the most commonly used fundamental symbols.
    pub const TRUE: Self = fundamental::TRUE;
    pub const FALSE: Self = fundamental::FALSE;
    pub const NULL: Self = fundamental::NULL;
    pub const NONE: Self = fundamental::NONE;
    pub const AUTOMATIC: Self = fundamental::AUTOMATIC;
    pub const ALL: Self = fundamental::ALL;
    pub const FULL: Self = fundamental::FULL;
    pub const INDETERMINATE: Self = fundamental::INDETERMINATE;
    pub const DIRECTED_INFINITY: Self = fundamental::DIRECTED_INFINITY;
    pub const LIST: Self = fundamental::LIST;
    pub const ASSOCIATION: Self = fundamental::ASSOCIATION;
    pub const RULE: Self = fundamental::RULE;
    pub const RULE_DELAYED: Self = fundamental::RULE_DELAYED;
    pub const BYTE_ARRAY: Self = fundamental::BYTE_ARRAY;
    pub const QUANTITY: Self = fundamental::QUANTITY;
    pub const RGB_COLOR: Self = fundamental::RGB_COLOR;
    pub const DATE_OBJECT: Self = fundamental::DATE_OBJECT;
    pub const TIME_OBJECT: Self = fundamental::TIME_OBJECT;
    pub const SPAN: Self = fundamental::SPAN;
}

macro_rules! define_system_symbols {
    (
        $(#[$mod_meta:meta])*
        pub mod $mod_name:ident {
            $(
                $(#[$meta:meta])*
                $const_name:ident => $short:literal
            ),* $(,)?
        }
    ) => {
        $(#[$mod_meta])*
        pub mod $mod_name {
            use super::Symbol;
            $(
                $(#[$meta])*
                pub const $const_name: Symbol = Symbol::from_static(concat!("System`", $short));
            )*

            /// All `` System` `` symbols defined in this domain module.
            pub const ALL_SYMBOLS: &[Symbol] = &[$($const_name),*];
        }
    };
}

define_system_symbols! {
    /// Fundamental Wolfram Language data, container, rule, and value-conversion symbols.
    pub mod fundamental {
        /// ``System`True``.
        TRUE => "True",
        /// ``System`False``.
        FALSE => "False",
        /// ``System`Null``.
        NULL => "Null",
        /// ``System`None``, the WL spelling of "no value".
        NONE => "None",
        /// ``System`Automatic``, the WL spelling of Typst's `auto`.
        AUTOMATIC => "Automatic",
        /// ``System`All``, used for unbounded ranges/queries.
        ALL => "All",
        /// ``System`Full``.
        FULL => "Full",
        /// ``System`Indeterminate``, used for non-finite `NaN` floats.
        INDETERMINATE => "Indeterminate",
        /// ``System`DirectedInfinity``, used for `±inf` floats.
        DIRECTED_INFINITY => "DirectedInfinity",
        /// ``System`List``.
        LIST => "List",
        /// ``System`Association``.
        ASSOCIATION => "Association",
        /// ``System`Rule`` — immediate rule `key -> value`.
        RULE => "Rule",
        /// ``System`RuleDelayed`` — delayed rule `key :> value`.
        RULE_DELAYED => "RuleDelayed",
        /// ``System`Span``.
        SPAN => "Span",
        /// ``System`ByteArray``.
        BYTE_ARRAY => "ByteArray",
        /// ``System`Quantity``.
        QUANTITY => "Quantity",
        /// ``System`RGBColor``.
        RGB_COLOR => "RGBColor",
        /// ``System`DateObject``.
        DATE_OBJECT => "DateObject",
        /// ``System`TimeObject``.
        TIME_OBJECT => "TimeObject",
    }
}

define_system_symbols! {
    /// Wolfram Language ``System`Graphics`` primitives, directives, options, and anchors.
    pub mod graphics {
        /// ``System`Graphics``.
        GRAPHICS => "Graphics",
        /// ``System`FilledCurve``.
        FILLED_CURVE => "FilledCurve",
        /// ``System`JoinedCurve``.
        JOINED_CURVE => "JoinedCurve",
        /// ``System`GeometricTransformation``.
        GEOMETRIC_TRANSFORMATION => "GeometricTransformation",
        /// ``System`Line``.
        LINE => "Line",
        /// ``System`Polygon``.
        POLYGON => "Polygon",
        /// ``System`Inset``.
        INSET => "Inset",
        /// ``System`Show``.
        SHOW => "Show",
        /// ``System`ImportByteArray``.
        IMPORT_BYTE_ARRAY => "ImportByteArray",
        /// ``System`EdgeForm``.
        EDGE_FORM => "EdgeForm",
        /// ``System`FaceForm``.
        FACE_FORM => "FaceForm",
        /// ``System`LinearGradientFilling``.
        LINEAR_GRADIENT_FILLING => "LinearGradientFilling",
        /// ``System`RadialGradientFilling``.
        RADIAL_GRADIENT_FILLING => "RadialGradientFilling",
        /// ``System`ConicGradientFilling``.
        CONIC_GRADIENT_FILLING => "ConicGradientFilling",
        /// ``System`PatternFilling``.
        PATTERN_FILLING => "PatternFilling",
        /// ``System`Texture``.
        TEXTURE => "Texture",
        /// ``System`Image``.
        IMAGE => "Image",
        /// ``System`Raster``.
        RASTER => "Raster",
        /// ``System`ColorSpace``.
        COLOR_SPACE => "ColorSpace",
        /// ``System`Background``.
        BACKGROUND => "Background",
        /// ``System`Thickness``.
        THICKNESS => "Thickness",
        /// ``System`CapForm``.
        CAP_FORM => "CapForm",
        /// ``System`JoinForm``.
        JOIN_FORM => "JoinForm",
        /// ``System`Dashing``.
        DASHING => "Dashing",
        /// ``System`ImageSize``.
        IMAGE_SIZE => "ImageSize",
        /// ``System`PlotRange``.
        PLOT_RANGE => "PlotRange",
        /// ``System`PlotRangePadding``.
        PLOT_RANGE_PADDING => "PlotRangePadding",
        /// ``System`PlotRangeClipping``.
        PLOT_RANGE_CLIPPING => "PlotRangeClipping",
        /// ``System`ImagePadding``.
        IMAGE_PADDING => "ImagePadding",
        /// ``System`AspectRatio``.
        ASPECT_RATIO => "AspectRatio",
        /// ``System`CurveClosed``.
        CURVE_CLOSED => "CurveClosed",
        /// ``System`Left``.
        LEFT => "Left",
        /// ``System`Bottom``.
        BOTTOM => "Bottom",
    }
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
        assert_eq!(fundamental::TRUE.as_str(), "System`True");
        assert_eq!(graphics::GRAPHICS.as_str(), "System`Graphics");
        assert_eq!(Symbol::system("List"), Symbol::LIST);
        assert_eq!(Symbol::from("System`Null"), Symbol::NULL);
        assert_eq!(graphics::FILLED_CURVE.short_name(), "FilledCurve");
    }

    #[test]
    fn domain_registries_have_only_system_qualified_symbols() {
        for sym in fundamental::ALL_SYMBOLS.iter().chain(graphics::ALL_SYMBOLS) {
            assert!(
                sym.as_str().starts_with("System`"),
                "expected System` prefix: {sym}"
            );
            assert!(!sym.short_name().is_empty());
        }
    }
}
