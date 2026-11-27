//! Declarative description of every public TypstForm option.
//!
//! The option list used to be spread over WL defaults, hand-written switch
//! lists in `TypstForm.wl`, and hand-written claim lists in Rust. This module
//! is the single description of what an option is, which plane it lives in,
//! and how (if at all) it reaches the native renderer. The schema generator
//! emits it into the WL capability file so the caller reads the same table the
//! renderer uses instead of guessing.

use std::collections::HashSet;

/// Where an option is handled. Only `Data` options cross the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// Handled by `MakeBoxes` before the box tree exists.
    Expression,
    /// Converted to plain data and sent to Rust.
    Data,
    /// A WL program or handler: always falls back.
    Code,
    /// Controls diagnostics only; never sent to Rust.
    Diagnostic,
}

impl Plane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expression => "Expression",
            Self::Data => "Data",
            Self::Code => "Code",
            Self::Diagnostic => "Diagnostic",
        }
    }
}

/// The shape of an option's value on the native wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireKind {
    /// Never sent (expression/code/diagnostic options).
    None,
    /// One named enum-like value, collected under `Switches`.
    Switch(&'static str),
    /// An entry in the effective character map delta.
    CharMap,
    /// An entry in the effective operator map delta.
    OperatorMap,
    /// An entry in the template overlay. Values may be strings or
    /// `TypstTemplate[...]` functional values.
    TemplateMap,
}

impl WireKind {
    pub fn field(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Switch(_) => "Switches",
            Self::CharMap => "CharOverrides",
            Self::OperatorMap => "OperatorOverrides",
            Self::TemplateMap => "TemplateOverrides",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Switch(key) => key,
            _ => "",
        }
    }
}

/// One public TypstForm option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    pub name: &'static str,
    pub plane: Plane,
    pub kind: WireKind,
    pub claimed: bool,
}

macro_rules! options {
    ($(($name:expr, $plane:expr, $kind:expr, $claimed:expr)),* $(,)?) => {
        &[
            $(OptionSpec { name: $name, plane: $plane, kind: $kind, claimed: $claimed }),*
        ]
    };
}

/// All 21 public options, in the same order as `Kernel/TypstForm/Config.wl`.
pub const OPTIONS: &[OptionSpec] = options![
    ("Form", Plane::Expression, WireKind::None, false),
    ("ExpressionRules", Plane::Expression, WireKind::None, false),
    ("BoxRules", Plane::Code, WireKind::None, false),
    ("TemplateRules", Plane::Data, WireKind::TemplateMap, true),
    ("CharacterRules", Plane::Data, WireKind::CharMap, true),
    ("OperatorNames", Plane::Data, WireKind::OperatorMap, true),
    (
        "DelimiterStyle",
        Plane::Data,
        WireKind::Switch("DelimiterStyle"),
        true
    ),
    (
        "FractionStyle",
        Plane::Data,
        WireKind::Switch("FractionStyle"),
        true
    ),
    (
        "MatrixDelimiter",
        Plane::Data,
        WireKind::Switch("MatrixDelimiter"),
        true
    ),
    (
        "PreserveStyle",
        Plane::Data,
        WireKind::Switch("PreserveStyle"),
        true
    ),
    (
        "StripContext",
        Plane::Data,
        WireKind::Switch("StripContext"),
        true
    ),
    (
        "ConjugateStyle",
        Plane::Data,
        WireKind::Switch("ConjugateStyle"),
        true
    ),
    (
        "MultiLetterStyle",
        Plane::Data,
        WireKind::Switch("MultiLetterStyle"),
        true
    ),
    (
        "TransposeSymbol",
        Plane::Data,
        WireKind::Switch("TransposeSymbol"),
        true
    ),
    ("ConstantStyle", Plane::Data, WireKind::CharMap, true),
    ("DifferentialStyle", Plane::Data, WireKind::CharMap, true),
    ("BlackboardStyle", Plane::Data, WireKind::CharMap, true),
    ("ScriptStyle", Plane::Data, WireKind::CharMap, true),
    (
        "UnknownCharacterHandler",
        Plane::Code,
        WireKind::None,
        false
    ),
    ("UnsupportedBoxHandler", Plane::Code, WireKind::None, false),
    (
        "NativeDiagnostics",
        Plane::Diagnostic,
        WireKind::None,
        false
    ),
];

/// Wire fields the renderer can decode, derived from the schema.
pub fn capability_fields() -> Vec<&'static str> {
    OPTIONS
        .iter()
        .filter(|option| option.claimed && option.kind.field() != "")
        .map(|option| option.kind.field())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

/// Switch keys the renderer can execute, derived from the schema.
pub fn claimed_switches() -> Vec<&'static str> {
    OPTIONS
        .iter()
        .filter(|option| option.claimed && matches!(option.kind, WireKind::Switch(_)))
        .map(|option| option.kind.key())
        .collect()
}

/// Generate the WL association consumed by `nativeConfigPatch`.
pub fn wolfram_option_schema() -> String {
    let entries = OPTIONS
        .iter()
        .map(|option| {
            format!(
                "  \"{}\" -> <|\"Plane\" -> \"{}\", \"Kind\" -> \"{}\", \
                 \"Wire\" -> \"{}\", \"Key\" -> \"{}\", \"Claimed\" -> {}|>",
                option.name,
                option.plane.as_str(),
                match option.kind {
                    WireKind::None => "None",
                    WireKind::Switch(_) => "Switch",
                    WireKind::CharMap => "CharMap",
                    WireKind::OperatorMap => "OperatorMap",
                    WireKind::TemplateMap => "TemplateMap",
                },
                option.kind.field(),
                option.kind.key(),
                if option.claimed { "True" } else { "False" },
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("$typstNativeOptionSchema = <|\n{entries}\n|>;\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_covers_every_public_option_once() {
        let mut names = OPTIONS.iter().map(|o| o.name).collect::<Vec<_>>();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        assert_eq!(count, 21);
    }

    #[test]
    fn capability_fields_are_derived_not_spelled_twice() {
        let mut fields = capability_fields();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "CharOverrides",
                "OperatorOverrides",
                "Switches",
                "TemplateOverrides"
            ]
        );
    }

    #[test]
    fn every_code_option_is_unclaimed() {
        assert!(OPTIONS.iter().all(|o| o.plane != Plane::Code || !o.claimed));
    }

    #[test]
    fn switches_are_exactly_the_claimed_switch_options() {
        assert_eq!(claimed_switches().len(), 8);
        assert!(claimed_switches().contains(&"TransposeSymbol"));
    }
}
