//! Every hand-written constant table of the box renderer, in one place.
//!
//! These mirror hand-written fragments of `Kernel/TypstForm/Boxes.wl` (accent
//! tokens, big operators, spacing heuristics, style-name shortcuts). They are
//! deliberately *not* in the generated `tables.json`: unlike the lookup maps
//! the WL side also consults at runtime, these encode decisions only the
//! native renderer makes about tokens it never sends back.

/// Argument markers `scripts/generate-box-tables.wl` embeds in generated
/// template display boxes; `\u{f8f0}<n>\u{f8f1}` marks template argument `n`.
pub(crate) const MARKER_OPEN: char = '\u{f8f0}';
pub(crate) const MARKER_CLOSE: char = '\u{f8f1}';

/// How many times a generated template expansion may nest before the tree is
/// handed back to the WL engine.
pub(crate) const MAX_TEMPLATE_DEPTH: u8 = 8;

/// The integral glyph and the Wolfram differential character of `∫ f ⅆx` rows.
pub(crate) const INTEGRAL: &str = "\u{222b}";
pub(crate) const DIFFERENTIAL: &str = "\u{f74c}";

/// The piecewise brace glyph opening `\[Piecewise] GridBox[...]` rows.
pub(crate) const PIECEWISE_MARK: &str = "\u{f361}";

/// Invisible-application / alignment glyphs a pre-subscripted operator head
/// may use as an empty base (`_p F_q` rows).
pub(crate) const INVISIBLE_BASES: &[&str] = &["\u{f3b3}", "\u{f360}", ""];

/// Script-position brace markers Wolfram uses for decorated scripts.
pub(crate) const OVERBRACE_MARK: &str = "\u{fe37}";
pub(crate) const OVERPAREN_MARK: &str = "\u{fe35}";
pub(crate) const UNDERBRACE_MARK: &str = "\u{fe38}";
pub(crate) const UNDERPAREN_MARK: &str = "\u{fe36}";

/// Tokens whose script position renders as a big operator with limits.
pub(crate) const BIG_OPERATORS: &[&str] = &[
    "\u{2211}", // ∑
    "\u{220f}", // ∏
    "\u{22c3}", "\u{22c2}", "\u{22c0}", "\u{22c1}", "\u{2210}", "\u{f3d9}",
];

/// Underscript tokens that render as operator-with-limits (`lim_x`), mirroring
/// the `UnderscriptBox[op : "lim" | ..., ...]` rule in `Boxes.wl`.
pub(crate) const LIM_LIKE_OPERATORS: &[&str] = &[
    "lim", "liminf", "limsup", "max", "min", "sup", "inf", "\u{f438}", "\u{f439}", "\u{f43a}",
];

/// Overscript accent tokens and the Typst construct that draws them. `_` is
/// resolved by the caller: `bar` over a single-character base, `overline`
/// otherwise.
pub(crate) const OVER_ACCENTS: &[(&str, &str)] = &[
    ("_", "overline"),
    ("\u{2500}", "overline"),
    ("\u{00af}", "overline"),
    ("\u{2190}", "accent-arrow"),
    ("\u{27f5}", "accent-arrow"),
    ("\u{f526}", "accent-arrow"),
    ("\u{2192}", "arrow"),
    ("\u{27f6}", "arrow"),
    ("\u{f525}", "arrow"),
    ("\u{21c0}", "arrow"),
    ("\u{2194}", "accent-both"),
    ("\u{27f7}", "accent-both"),
    ("^", "hat"),
    ("\u{02c6}", "hat"),
    ("\u{02c7}", "caron"),
    ("~", "tilde"),
    ("\u{223c}", "tilde"),
    ("\u{02dc}", "tilde"),
    ("'", "acute"),
    ("\u{2032}", "acute"),
    ("\u{00b4}", "acute"),
    ("`", "grave"),
    ("\u{2035}", "grave"),
    ("\u{02d8}", "breve"),
    (".", "dot"),
    ("\u{2022}", "dot"),
    ("\u{00b7}", "dot"),
    ("..", "dot.double"),
    ("\u{2022}\u{2022}", "dot.double"),
    ("\u{00b7}\u{00b7}", "dot.double"),
    ("\u{00a8}", "dot.double"),
    ("...", "dot.triple"),
    ("\u{2022}\u{2022}\u{2022}", "dot.triple"),
    ("\u{00b7}\u{00b7}\u{00b7}", "dot.triple"),
    ("\u{f758}", "dot.triple"),
    ("\u{2026}", "dot.triple"),
    ("\u{22ef}", "dot.triple"),
    ("....", "dot.quad"),
    ("\u{2022}\u{2022}\u{2022}\u{2022}", "dot.quad"),
    ("\u{00b7}\u{00b7}\u{00b7}\u{00b7}", "dot.quad"),
    ("\u{00a8}\u{00a8}", "dot.quad"),
];

/// Underscript accent tokens, mirroring `Boxes.wl`'s `UnderscriptBox` rules.
pub(crate) const UNDER_ACCENTS: &[(&str, &str)] = &[
    ("_", "underline"),
    ("\u{2500}", "underline"),
    ("\u{2194}", "attach-arrow.l.r"),
    ("\u{27f7}", "attach-arrow.l.r"),
    ("\u{2192}", "attach-arrow.r"),
    ("\u{27f6}", "attach-arrow.r"),
    ("\u{f525}", "attach-arrow.r"),
];

/// Look a token up in an accent table.
pub(crate) fn accent(table: &[(&str, &'static str)], token: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(candidate, _)| *candidate == token)
        .map(|(_, construct)| *construct)
}

/// Prefix-like tokens after which `+` or `-` is unary.
pub(crate) const UNARY_CONTEXT_TOKENS: &[&str] = &[
    "(", "[", "{", ",", ";", "=", "<", ">", "+", "-", "*", "/", ":", "<=", ">=", "!=", "==", "->",
    ":->", "<->", "=>", "|->", "mid(|)", "and", "or", "not", "in", "equiv", "approx",
];

/// Function-like prefixes that attach tightly to `(` — `sin(x)`, `Gamma(z)`,
/// `op("erf")(x)` — mirroring `tightCallPrefixQ`'s suffix list.
pub(crate) const TIGHT_CALL_SUFFIXES: &[&str] = &[
    "arccos",
    "arcsin",
    "arctan",
    "arg",
    "cos",
    "cosh",
    "cot",
    "coth",
    "csc",
    "csch",
    "ctg",
    "deg",
    "det",
    "dim",
    "exp",
    "gcd",
    "hom",
    "id",
    "im",
    "inf",
    "ker",
    "lcm",
    "lg",
    "lim",
    "liminf",
    "limsup",
    "ln",
    "log",
    "max",
    "min",
    "mod",
    "Pr",
    "sec",
    "sech",
    "sin",
    "sinc",
    "sinh",
    "sup",
    "tan",
    "tanh",
    "tg",
    "tr",
    "Re",
    "Im",
    "pee",
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "epsilon.alt",
    "zeta",
    "eta",
    "theta",
    "theta.alt",
    "iota",
    "kappa",
    "kappa.alt",
    "lambda",
    "mu",
    "nu",
    "xi",
    "pi",
    "pi.alt",
    "rho",
    "rho.alt",
    "sigma",
    "sigma.alt",
    "tau",
    "upsilon",
    "phi",
    "phi.alt",
    "chi",
    "psi",
    "omega",
    "Gamma",
    "Delta",
    "Theta",
    "Lambda",
    "Xi",
    "Pi",
    "Sigma",
    "Upsilon",
    "Phi",
    "Psi",
    "Omega",
    "Digamma",
    "digamma",
];

/// Style-script prefixes a script-position string may already carry
/// (`upright(T)` must not be parenthesised again).
pub(crate) const SCRIPT_STYLE_PREFIXES: &[&str] = &[
    "upright", "bold", "italic", "bb", "cal", "scr", "frak", "sans", "mono",
];

/// Invisible spacer tokens (`invisibleSpacerQ` in `Boxes.wl`).
pub(crate) const INVISIBLE_SPACERS: &[&str] = &[
    "\u{2060}", "\u{f360}", "\u{f760}", "\u{f76d}", "\u{f3a0}", "",
];

/// Spacer tokens padding a matrix delimiter row (`matrixPaddingSpacerQ`).
pub(crate) const MATRIX_PADDING_SPACERS: &[&str] = &[
    " ", "\u{205f}", "\u{2009}", "\u{200a}", "\u{2005}", "\u{f3b3}", "\u{f3b4}", "\u{f3ad}",
];

/// Positional style names that set bold (`convertStyleOrCell`).
pub(crate) const BOLD_STYLE_NAMES: &[&str] =
    &["Input", "InputOnly", "InlineInput", "BF", "TB", "MB"];

/// Positional style names that set italic.
pub(crate) const ITALIC_STYLE_NAMES: &[&str] = &["IT", "TI", "MO", "SB"];

/// Positional style names that set bold and italic.
pub(crate) const BOLD_ITALIC_STYLE_NAMES: &[&str] = &["TBI", "MBO", "SBO"];

/// StyleBox font options the native renderer recognizes but does not lay out.
/// Their presence earns a warning; anything outside this list is treated as
/// incidental Cell styling and stays silent.
pub(crate) const DROPPED_FONT_OPTIONS: &[&str] = &[
    "FontFamily",
    "FontSize",
    "FontTracking",
    "FontOpacity",
    "FontEffects",
    "Background",
];
