//! Box-head identity.
//!
//! Dispatching on `&str` made every head a string literal repeated across the
//! renderer, `strip_identity`, the "tall box" test and the table loader. A
//! typo in any of them silently became "unsupported box" — that is, a silent
//! fallback to the WL reference engine — and adding a head meant remembering
//! every list it had to appear in. `Head` gives the set of heads one
//! definition, an exhaustive `match` at the dispatch site, and one spelling
//! for the wire format, which has to carry head names once user box rules are
//! compiled by the kernel and replayed here.
//!
//! The grouping below follows *The Wolfram Box Typesetting Reference*
//! (`PacletRepository/resources/Wolfram/Parser/tutorial/WolframBoxTypesetting`),
//! Part 8: of the ~114 `System` symbols ending in `Box`, only the first group
//! is typesetting, the second is the semantic wrappers that display as their
//! first argument, and everything else (interactive controls, 2D/3D graphics)
//! is out of scope for a math renderer. Heads outside these groups stay
//! representable as [`Head::Other`], which keeps the name for diagnostics.

/// Declares the head set once and derives the two directions of the name
/// mapping from the same list, so `from_name` and `name` cannot drift.
macro_rules! heads {
    ($( $(#[$meta:meta])* $variant:ident => $name:literal, )*) => {
        /// Box heads the renderer reasons about individually.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum Head {
            $( $(#[$meta])* $variant, )*
            /// A head outside the documented typesetting set — an interactive
            /// control, a graphics primitive, or something newer than this
            /// table. Kept by name so `Reason` stays diagnosable.
            Other(Box<str>),
        }

        impl Head {
            pub fn from_name(name: &str) -> Self {
                match name {
                    $( $name => Self::$variant, )*
                    _ => Self::Other(name.into()),
                }
            }

            pub fn name(&self) -> &str {
                match self {
                    $( Self::$variant => $name, )*
                    Self::Other(name) => name,
                }
            }
        }
    };
}

heads! {
    // ---- Typesetting / formatting -------------------------------------
    // The geometry boxes: everything a math renderer has to lay out.
    RowBox => "RowBox",
    GridBox => "GridBox",
    FractionBox => "FractionBox",
    SqrtBox => "SqrtBox",
    RadicalBox => "RadicalBox",
    SubscriptBox => "SubscriptBox",
    SuperscriptBox => "SuperscriptBox",
    SubsuperscriptBox => "SubsuperscriptBox",
    UnderscriptBox => "UnderscriptBox",
    OverscriptBox => "OverscriptBox",
    UnderoverscriptBox => "UnderoverscriptBox",
    StyleBox => "StyleBox",
    AdjustmentBox => "AdjustmentBox",
    FrameBox => "FrameBox",
    ItemBox => "ItemBox",
    PaneBox => "PaneBox",
    PanelBox => "PanelBox",
    RotationBox => "RotationBox",
    OverlayBox => "OverlayBox",
    InsetBox => "InsetBox",

    // ---- Structural / semantic wrappers --------------------------------
    // All of these display as their first argument and only differ in what
    // they mean on input or copy/paste.
    TagBox => "TagBox",
    InterpretationBox => "InterpretationBox",
    FormBox => "FormBox",
    TemplateBox => "TemplateBox",
    TemplateArgBox => "TemplateArgBox",
    DynamicBox => "DynamicBox",
    DynamicModuleBox => "DynamicModuleBox",
    DynamicWrapperBox => "DynamicWrapperBox",
    NamespaceBox => "NamespaceBox",
    ErrorBox => "ErrorBox",
    BoxParentBox => "BoxParentBox",
    EvaluationBox => "EvaluationBox",
    TooltipBox => "TooltipBox",
    ValueBox => "ValueBox",
    OptionValueBox => "OptionValueBox",
    CounterBox => "CounterBox",
    Dynamic => "Dynamic",

    // ---- Interactive controls the reference converter still renders ------
    // Out of the typesetting group, but `Boxes.wl` renders their label, so the
    // native path has to agree with it rather than treat them as unknown.
    ButtonBox => "ButtonBox",
    InputFieldBox => "InputFieldBox",

    // ---- Document containers -------------------------------------------
    // Not boxes in the reference's sense, but `MakeBoxes` emits them and the
    // reference converter has rules for them.
    Cell => "Cell",
    BoxData => "BoxData",
    TextData => "TextData",
    Notebook => "Notebook",

    // ---- Data heads that occur inside box trees ------------------------
    List => "List",
    Rule => "Rule",
    RuleDelayed => "RuleDelayed",

    // ---- Colour specs: only inspected, to detect styles we cannot render
    RgbColor => "RGBColor",
    GrayLevel => "GrayLevel",
    Hue => "Hue",
    CmykColor => "CMYKColor",
}

impl Head {
    /// Heads that render as a row of elements.
    pub fn is_row_like(&self) -> bool {
        matches!(self, Self::RowBox | Self::List)
    }

    /// The reference's typesetting group: the boxes that carry geometry.
    pub fn is_typesetting(&self) -> bool {
        matches!(
            self,
            Self::RowBox
                | Self::GridBox
                | Self::FractionBox
                | Self::SqrtBox
                | Self::RadicalBox
                | Self::SubscriptBox
                | Self::SuperscriptBox
                | Self::SubsuperscriptBox
                | Self::UnderscriptBox
                | Self::OverscriptBox
                | Self::UnderoverscriptBox
                | Self::StyleBox
                | Self::AdjustmentBox
                | Self::FrameBox
                | Self::ItemBox
                | Self::PaneBox
                | Self::PanelBox
                | Self::RotationBox
                | Self::OverlayBox
                | Self::InsetBox
        )
    }

    /// Heads that display as their first argument.
    pub fn is_semantic_wrapper(&self) -> bool {
        matches!(
            self,
            Self::TagBox
                | Self::InterpretationBox
                | Self::FormBox
                | Self::TemplateBox
                | Self::TemplateArgBox
                | Self::DynamicBox
                | Self::DynamicModuleBox
                | Self::DynamicWrapperBox
                | Self::NamespaceBox
                | Self::ErrorBox
                | Self::BoxParentBox
                | Self::EvaluationBox
                | Self::TooltipBox
                | Self::ValueBox
                | Self::OptionValueBox
                | Self::CounterBox
        )
    }

    /// Heads whose entire rendering is their first argument, so the AST may
    /// drop the wrapper. This is exactly the set `stripIdentityBoxes` unwraps;
    /// the other semantic wrappers keep extra meaning (a tag, an input form, a
    /// namespace) that the reference converter does not discard either.
    pub fn is_transparent(&self) -> bool {
        matches!(
            self,
            Self::AdjustmentBox
                | Self::InterpretationBox
                | Self::DynamicBox
                | Self::TooltipBox
                | Self::Dynamic
        )
    }

    /// Heads tall enough that bracketing them is worth an explicit delimiter.
    /// Mirrors `delimiterBoxQ` in `Boxes.wl`.
    pub fn is_tall(&self) -> bool {
        matches!(
            self,
            Self::GridBox
                | Self::SuperscriptBox
                | Self::SubscriptBox
                | Self::SubsuperscriptBox
                | Self::OverscriptBox
                | Self::UnderscriptBox
                | Self::UnderoverscriptBox
                | Self::FractionBox
                | Self::SqrtBox
                | Self::RadicalBox
                | Self::FrameBox
                | Self::ButtonBox
                | Self::PaneBox
                | Self::PanelBox
        )
    }

    /// Heads whose content is their first argument, so a number test may look
    /// through them.
    pub fn unwraps_first_arg(&self) -> bool {
        matches!(
            self,
            Self::RowBox
                | Self::SuperscriptBox
                | Self::SubscriptBox
                | Self::OverscriptBox
                | Self::UnderscriptBox
                | Self::SubsuperscriptBox
                | Self::UnderoverscriptBox
                | Self::StyleBox
                | Self::AdjustmentBox
                | Self::InterpretationBox
                | Self::TagBox
                | Self::FormBox
                | Self::Cell
                | Self::BoxData
                | Self::TextData
                | Self::TooltipBox
                | Self::ValueBox
                | Self::OptionValueBox
        )
    }

    pub fn is_color(&self) -> bool {
        matches!(
            self,
            Self::RgbColor | Self::GrayLevel | Self::Hue | Self::CmykColor
        )
    }

    pub fn is_rule(&self) -> bool {
        matches!(self, Self::Rule | Self::RuleDelayed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for name in [
            "RowBox",
            "GridBox",
            "TemplateBox",
            "Notebook",
            "RuleDelayed",
            "CMYKColor",
            "NamespaceBox",
        ] {
            let head = Head::from_name(name);
            assert_ne!(head, Head::Other(name.into()), "unregistered head: {name}");
            assert_eq!(head.name(), name);
        }
    }

    #[test]
    fn unknown_heads_keep_their_name() {
        let head = Head::from_name("MysteryBox");
        assert_eq!(head, Head::Other("MysteryBox".into()));
        assert_eq!(head.name(), "MysteryBox");
    }

    /// Part 8's first group, verbatim. `GridBoxAlignment` is an option value,
    /// not a box, so it is absent on purpose.
    #[test]
    fn the_documented_typesetting_group_is_covered() {
        for name in [
            "RowBox",
            "GridBox",
            "FractionBox",
            "SqrtBox",
            "RadicalBox",
            "SubscriptBox",
            "SuperscriptBox",
            "SubsuperscriptBox",
            "UnderscriptBox",
            "OverscriptBox",
            "UnderoverscriptBox",
            "StyleBox",
            "AdjustmentBox",
            "FrameBox",
            "ItemBox",
            "PaneBox",
            "PanelBox",
            "RotationBox",
            "OverlayBox",
            "InsetBox",
        ] {
            assert!(
                Head::from_name(name).is_typesetting(),
                "{name} is typesetting but not classified as such"
            );
        }
    }

    /// Part 8's second group. Every one of these displays as its first
    /// argument, so none of them may be classified as geometry.
    #[test]
    fn the_documented_wrapper_group_is_covered() {
        for name in [
            "TagBox",
            "InterpretationBox",
            "FormBox",
            "TemplateBox",
            "TemplateArgBox",
            "DynamicBox",
            "DynamicModuleBox",
            "DynamicWrapperBox",
            "NamespaceBox",
            "ErrorBox",
            "BoxParentBox",
            "EvaluationBox",
            "ValueBox",
            "OptionValueBox",
            "CounterBox",
        ] {
            let head = Head::from_name(name);
            assert!(head.is_semantic_wrapper(), "{name} is a wrapper");
            assert!(!head.is_typesetting(), "{name} carries no geometry");
        }
    }

    /// Interactive controls and graphics primitives stay out of the enum but
    /// must remain recognisable by name.
    #[test]
    fn out_of_scope_heads_are_recognised_by_name() {
        for name in ["CheckboxBox", "GraphicsBox", "Raster3DBox", "Slider2DBox"] {
            assert_eq!(Head::from_name(name), Head::Other(name.into()));
        }
    }
}
