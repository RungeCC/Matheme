//! Wolfram Box DSL → Typst math conversion.
//!
//! `TexForm` in the Wolfram kernel is fast because its box renderer is native
//! code. This crate applies the same separation: `MakeBoxes` stays in the
//! kernel, while the Box → Typst stage is a static, parallel Rust renderer.
//! Anything that needs WL evaluation is reported as unsupported so the caller
//! can fall back to the reference implementation in `Kernel/TypstForm/Boxes.wl`.

mod ast;
mod config;
mod convert;
mod error;
mod head;
mod schema;
mod tables;

pub use config::{Config, NATIVE_CONFIG_VERSION};
pub use convert::{ConversionWarning, Converter};
pub use error::{Stage, Unsupported};
pub use head::Head;
use matheme_wl::Expr;
pub use schema::{OPTIONS, capability_fields, claimed_switches, wolfram_option_schema};
pub use tables::source_hash;

/// Convert a Wolfram Box expression using the default TypstForm configuration.
///
/// The returned string is `StringTrim`-ed, matching `BoxesToTypst`. An error
/// means the caller must use the WL converter for this expression.
pub fn boxes_to_typst(boxes: &Expr) -> Result<String, Unsupported> {
    boxes_to_typst_with_config(boxes, &Config::default())
}

/// Convert with a decoded native config patch.
///
/// An empty patch is byte-for-byte equivalent to [`boxes_to_typst`]; a field
/// the native renderer cannot execute is reported as `Unsupported` with
/// `stage = Config` and `field = <key>` rather than being ignored.
pub fn boxes_to_typst_with_config(boxes: &Expr, config: &Config) -> Result<String, Unsupported> {
    let node = ast::from_wolfram(boxes).map_err(Unsupported::from)?;
    Converter::new(config).render(&node)
}

/// Like [`boxes_to_typst_with_config`], but also returns the warnings for
/// box-tree features the renderer recognized and deliberately does not lay out
/// (dropped StyleBox font options, for instance). Warnings are empty whenever
/// the render fails, because the caller then re-renders through the WL engine.
pub fn boxes_to_typst_with_config_and_warnings(
    boxes: &Expr,
    config: &Config,
) -> Result<(String, Vec<ConversionWarning>), Unsupported> {
    let node = ast::from_wolfram(boxes).map_err(Unsupported::from)?;
    Converter::new(config).render_with_warnings(&node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use matheme_wl::{Expr, Rule, Symbol};

    fn box_expr(head: &str, args: Vec<Expr>) -> Expr {
        Expr::normal(Symbol::new(format!("System`{head}")), args)
    }

    fn config(entries: impl IntoIterator<Item = Rule>) -> Config {
        Config::from_expr(&Expr::association(entries)).unwrap()
    }

    fn font_rule(key: &str, value: &str) -> Expr {
        Expr::normal(
            Symbol::new("System`Rule"),
            [
                Expr::symbol(Symbol::new(format!("System`{key}"))),
                Expr::symbol(Symbol::new(format!("System`{value}"))),
            ],
        )
    }

    #[test]
    fn stylebox_font_rules_render_consistently() {
        let styled = |rule: Expr| box_expr("StyleBox", vec![Expr::string("x"), rule]);
        let bold = styled(font_rule("FontWeight", "Bold"));
        assert_eq!(boxes_to_typst(&bold).unwrap(), "bold(x)");
        let italic = styled(font_rule("FontSlant", "Italic"));
        assert_eq!(boxes_to_typst(&italic).unwrap(), "italic(x)");
        let underline = Expr::normal(
            Symbol::new("System`Rule"),
            [
                Expr::symbol(Symbol::new("System`FontVariations")),
                Expr::normal(
                    Symbol::LIST,
                    vec![Expr::normal(
                        Symbol::RULE,
                        [
                            Expr::string("Underline"),
                            Expr::symbol(Symbol::new("System`True")),
                        ],
                    )],
                ),
            ],
        );
        let underlined = styled(underline);
        assert_eq!(boxes_to_typst(&underlined).unwrap(), "underline(x)");
    }

    #[test]
    fn dropped_style_options_warn_instead_of_vanishing() {
        let boxed = box_expr(
            "StyleBox",
            vec![Expr::string("x"), font_rule("FontWeight", "SemiBold")],
        );
        let (typst, warnings) =
            boxes_to_typst_with_config_and_warnings(&boxed, &Config::default()).unwrap();
        assert_eq!(typst, "x");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].field, "StyleBox/FontWeight");
        assert!(warnings[0].message.contains("SemiBold"));
    }

    /// A shape violation is malformed input, not a missing rendering rule.
    #[test]
    fn wrong_arity_is_malformed_not_unsupported() {
        let frac = box_expr("FractionBox", vec![Expr::string("a")]);
        let err = boxes_to_typst(&frac).unwrap_err();
        assert_eq!(err.stage, Stage::Malformed);
        assert_eq!(err.field, "FractionBox");

        let template = box_expr(
            "TemplateBox",
            vec![Expr::string("not-a-list"), Expr::string("Abs")],
        );
        let err = boxes_to_typst(&template).unwrap_err();
        assert_eq!(err.stage, Stage::Malformed);
        assert_eq!(err.field, "TemplateBox");
    }

    #[test]
    fn simple_row_and_fraction() {
        let row = box_expr(
            "RowBox",
            vec![Expr::normal(
                Symbol::LIST,
                vec![Expr::string("a"), Expr::string("+"), Expr::string("b")],
            )],
        );
        assert_eq!(boxes_to_typst(&row).unwrap(), "a + b");

        let frac = box_expr("FractionBox", vec![Expr::string("a"), Expr::string("b")]);
        assert_eq!(boxes_to_typst(&frac).unwrap(), "frac(a, b)");
    }

    #[test]
    fn inactive_head_renders_like_a_string_box() {
        let head = box_expr(
            "TemplateBox",
            vec![
                Expr::normal(Symbol::LIST, vec![Expr::string("sin")]),
                Expr::string("InactiveHead"),
            ],
        );
        let call = box_expr(
            "RowBox",
            vec![Expr::normal(
                Symbol::LIST,
                vec![
                    head,
                    Expr::string("("),
                    Expr::string("x"),
                    Expr::string(")"),
                ],
            )],
        );
        assert_eq!(boxes_to_typst(&call).unwrap(), "sin(x)");
    }

    #[test]
    fn semantic_template_tags_expand_through_the_generated_table() {
        let tagged = |args: Vec<Expr>, tag: &str| {
            box_expr(
                "TemplateBox",
                vec![Expr::normal(Symbol::LIST, args), Expr::string(tag)],
            )
        };
        assert_eq!(
            boxes_to_typst(&tagged(
                vec![Expr::string("2"), Expr::string("x")],
                "BesselY"
            ))
            .unwrap(),
            "Y_2(x)"
        );
        assert_eq!(
            boxes_to_typst(&tagged(vec![Expr::string("m")], "EllipticK")).unwrap(),
            "K(m)"
        );
    }

    #[test]
    fn character_overrides_reach_the_renderer() {
        let row = box_expr(
            "RowBox",
            vec![Expr::normal(Symbol::LIST, vec![Expr::string("\u{3c0}")])],
        );
        let patch = config([Rule::new(
            "CharOverrides",
            Expr::association([Rule::new(Expr::string("\u{3c0}"), Expr::string("my.pi "))]),
        )]);
        assert_eq!(
            boxes_to_typst_with_config(&row, &patch).unwrap(),
            "my.pi",
            "the default map must not shadow a user character override"
        );
        assert_eq!(boxes_to_typst(&row).unwrap(), "pi");
    }

    #[test]
    fn operator_overrides_reach_the_renderer() {
        let row = box_expr(
            "RowBox",
            vec![Expr::normal(Symbol::LIST, vec![Expr::string("myop")])],
        );
        let patch = config([Rule::new(
            "OperatorOverrides",
            Expr::association([Rule::new(
                Expr::string("myop"),
                Expr::string("op(\"custom\")"),
            )]),
        )]);
        assert_eq!(
            boxes_to_typst_with_config(&row, &patch).unwrap(),
            "op(\"custom\")"
        );
    }

    #[test]
    fn template_overrides_prefer_exact_arity_over_tag() {
        let tagged = |tag: &str| {
            box_expr(
                "TemplateBox",
                vec![
                    Expr::normal(Symbol::LIST, vec![Expr::string("x")]),
                    Expr::string(tag),
                ],
            )
        };
        let patch = config([Rule::new(
            "TemplateOverrides",
            Expr::association([
                Rule::new(Expr::string("Abs"), Expr::string("tag")),
                Rule::new(Expr::string("Abs/1"), Expr::string("exact")),
            ]),
        )]);
        assert_eq!(
            boxes_to_typst_with_config(&tagged("Abs"), &patch).unwrap(),
            "exact"
        );
        let generic_only = config([Rule::new(
            "TemplateOverrides",
            Expr::association([Rule::new(Expr::string("Abs"), Expr::string("tag"))]),
        )]);
        assert_eq!(
            boxes_to_typst_with_config(&tagged("Abs"), &generic_only).unwrap(),
            "tag"
        );
        assert_eq!(boxes_to_typst(&tagged("Abs")).unwrap(), "abs(x)");
    }

    #[test]
    fn typst_template_values_substitute_rendered_strings() {
        let tagged = |n: usize| {
            let args = (0..n)
                .map(|i| Expr::string(format!("a{i}")))
                .collect::<Vec<_>>();
            box_expr(
                "TemplateBox",
                vec![Expr::normal(Symbol::LIST, args), Expr::string("MyTag")],
            )
        };
        let template_expr = |body: &str| {
            Expr::normal(
                Symbol::new("Runge`Matheme`TypstTemplate"),
                [Expr::string(body)],
            )
        };
        let patch = config([Rule::new(
            "TemplateOverrides",
            Expr::association([Rule::new(
                Expr::string("MyTag"),
                template_expr("row({1}; {rest})"),
            )]),
        )]);
        assert_eq!(
            boxes_to_typst_with_config(&tagged(1), &patch).unwrap(),
            "row(\"a0\"; )"
        );
        assert_eq!(
            boxes_to_typst_with_config(&tagged(3), &patch).unwrap(),
            "row(\"a0\"; \"a1\", \"a2\")"
        );
    }

    #[test]
    fn a_wolfram_string_template_does_not_become_a_native_template() {
        let patch = Expr::association([Rule::new(
            "TemplateOverrides",
            Expr::association([Rule::new(
                Expr::string("Abs"),
                Expr::normal(
                    Symbol::new("System`StringTemplate"),
                    [Expr::string("{{1}}")],
                ),
            )]),
        )]);
        let err = Config::from_expr(&patch).unwrap_err();
        assert_eq!(err.field, "TemplateRules");
    }

    #[test]
    fn unknown_box_is_unsupported() {
        let unknown = box_expr("MysteryBox", vec![Expr::string("x")]);
        assert!(boxes_to_typst(&unknown).is_err());
    }

    /// Documented typesetting and wrapper heads that the reference converter
    /// also leaves to `handleUnsupportedBox` must not be silently dropped: the
    /// native path reports them so the WL fallback keeps its message.
    #[test]
    fn documented_but_unrendered_heads_fall_back() {
        for head in ["ItemBox", "NamespaceBox", "RotationBox", "EvaluationBox"] {
            let boxed = box_expr(head, vec![Expr::string("x")]);
            assert!(boxes_to_typst(&boxed).is_err(), "{head} must fall back");
        }
    }

    #[test]
    fn integral_rows_keep_the_thin_space() {
        let differential = Expr::string("\u{f74c}");
        let body = box_expr(
            "RowBox",
            vec![Expr::normal(
                Symbol::LIST,
                vec![
                    Expr::string("y"),
                    box_expr(
                        "RowBox",
                        vec![Expr::normal(
                            Symbol::LIST,
                            vec![differential, Expr::string("\u{3c0}")],
                        )],
                    ),
                ],
            )],
        );
        let integral = box_expr(
            "RowBox",
            vec![Expr::normal(
                Symbol::LIST,
                vec![Expr::string("\u{222b}"), body],
            )],
        );
        assert_eq!(
            boxes_to_typst(&integral).unwrap(),
            "integral y thin dif pi",
            "the integral fast path must emit the thin space before dif"
        );
    }
}
