//! TemplateBox: user rules, hand-written semantic tags, and the generated
//! display-box expansion table.

use super::consts::{MARKER_CLOSE, MARKER_OPEN, MAX_TEMPLATE_DEPTH};
use super::row::join_tokens;
use super::scripts::make_script_string;
use super::{Converter, Ctx, arg, node_list};
use crate::ast::Node;
use crate::config::RuntimeTemplate;
use crate::error::Unsupported;
use crate::head::Head;
use crate::tables;

impl Converter<'_> {
    pub(super) fn template(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let tag = args.get(1).and_then(Node::as_str).ok_or_else(|| {
            Unsupported::malformed("TemplateBox", "second argument must be a String tag")
        })?;
        let inside = node_list(arg(&Head::TemplateBox, args, 0)?).ok_or_else(|| {
            Unsupported::malformed("TemplateBox", "payload must be a List of arguments")
        })?;
        let arg = |i: usize| -> Result<&Node, Unsupported> {
            inside.get(i).ok_or_else(|| {
                Unsupported::malformed(
                    "TemplateBox",
                    format!("template {tag} expects at least {} argument(s)", i + 1),
                )
            })
        };
        let inside_delim =
            |i: usize| -> Result<String, Unsupported> { self.convert_inside_delim(arg(i)?, ctx) };

        // User rules win over built-ins and over the generated display table.
        // An exact `tag/arity` key wins over a tag-wide key, matching the
        // dispatcher in `Templates.wl`.
        if let Some(rule) = ctx.config.template(tag, inside.len()) {
            return match rule {
                RuntimeTemplate::Text(text) => Ok(text.clone()),
                RuntimeTemplate::Typst(template) => {
                    let rendered = (0..inside.len())
                        .map(&inside_delim)
                        .collect::<Result<Vec<_>, _>>()?;
                    template
                        .expand(&rendered)
                        .map_err(|err| Unsupported::box_head("TemplateRules", err.to_string()))
                }
            };
        }

        match tag {
            "MathemeTypstRaw" => Ok(arg(0)?.as_str().unwrap_or_default().to_owned()),
            "MathemeTypstBox" => self.convert(arg(0)?, ctx),
            // Semantic tags the reference engine also handles by hand stay
            // hand-written here so both engines agree on the nicer Typst form.
            // Everything else is expanded from the generated table below.
            "Abs" => self.template_delimited("abs", "|", "|", arg(0)?, ctx),
            "Norm" => self.template_delimited("norm", "bar.v.double", "bar.v.double", arg(0)?, ctx),
            "Floor" => self.template_delimited("floor", "floor.l", "floor.r", arg(0)?, ctx),
            "Ceiling" => self.template_delimited("ceil", "ceil.l", "ceil.r", arg(0)?, ctx),
            "Round" => Ok(format!("round({})", inside_delim(0)?)),
            "Binomial" => Ok(format!("binom({}, {})", inside_delim(0)?, inside_delim(1)?)),
            "QBinomial" => Ok(format!(
                "binom({}, {})_{}",
                inside_delim(0)?,
                inside_delim(1)?,
                self.make_script(arg(2)?, ctx)?
            )),
            "Limit2Arg" => Ok(format!(
                "lim_({} -> {}) {}",
                inside_delim(1)?,
                inside_delim(2)?,
                self.convert(arg(0)?, ctx)?.trim()
            )),
            "ConditionalExpression" => Ok(format!(
                "{} quad \"if\" quad {}",
                self.convert(arg(0)?, ctx)?.trim(),
                self.convert(arg(1)?, ctx)?.trim()
            )),
            "Det" => {
                let stripped = arg(0)?;
                if stripped.is(&Head::GridBox) {
                    let rows = node_list(stripped.arg(0).ok_or_else(|| {
                        Unsupported::malformed("GridBox", "determinant grid has no rows")
                    })?)
                    .unwrap_or_default();
                    self.format_matrix(
                        rows,
                        "\"|\"",
                        stripped.args().get(1..).unwrap_or_default(),
                        ctx,
                    )
                } else {
                    Ok(format!("det({})", inside_delim(0)?))
                }
            }
            "Conjugate" => {
                let base = self.script_base(arg(0)?, ctx)?;
                if ctx.config.switch_is("ConjugateStyle", "Overline") {
                    Ok(format!(
                        "overline({})",
                        self.convert_inside_delim(arg(0)?, ctx)?.trim()
                    ))
                } else {
                    Ok(format!("{base}^*"))
                }
            }
            "Transpose" => {
                let symbol = ctx.config.switch("TransposeSymbol").unwrap_or("upright(T)");
                Ok(format!(
                    "{}^{}",
                    self.script_base(arg(0)?, ctx)?,
                    make_script_string(symbol, false)
                ))
            }
            "ConjugateTranspose" | "HermitianConjugate" => {
                Ok(format!("{}^dagger", self.script_base(arg(0)?, ctx)?))
            }
            "Superscript" => {
                let node =
                    Node::with_args(Head::SuperscriptBox, vec![arg(0)?.clone(), arg(1)?.clone()]);
                self.convert(&node, ctx)
            }
            "Subscript" => {
                let node =
                    Node::with_args(Head::SubscriptBox, vec![arg(0)?.clone(), arg(1)?.clone()]);
                self.convert(&node, ctx)
            }
            "Subsuperscript" => {
                let node = Node::with_args(
                    Head::SubsuperscriptBox,
                    vec![arg(0)?.clone(), arg(1)?.clone(), arg(2)?.clone()],
                );
                self.convert(&node, ctx)
            }
            "Quantity" | "QuantityTF" => {
                let value = self.convert(arg(0)?, ctx)?.trim().to_owned();
                let unit = self.convert_quantity(arg(1)?, ctx)?;
                if unit.is_empty() {
                    Ok(value)
                } else {
                    Ok(format!("{value} thin {unit}"))
                }
            }
            "QuantityPostfix" | "QuantityPostfixTF" => {
                let value = self.convert(arg(0)?, ctx)?.trim().to_owned();
                let unit = self.convert_quantity(arg(1)?, ctx)?;
                Ok(format!("{value}{unit}"))
            }
            "QuantityUnit" => self.convert_quantity(arg(0)?, ctx),
            "Spacer1" | "Spacer2" => {
                let width = match arg(0)? {
                    Node::Num(value) => value.clone(),
                    Node::Str(value) => value.clone(),
                    _ => {
                        return Err(Unsupported::malformed(
                            tag,
                            "spacer width must be a number or string",
                        ));
                    }
                };
                Ok(format!("#h({width}pt)"))
            }
            "RowDefault" | "RowNoSeparators" => {
                let tokens = self.convert_many(inside, ctx)?;
                Ok(join_tokens(&tokens))
            }
            "RowWithSeparators" => {
                let sep = self.convert(arg(0)?, ctx)?;
                let items = self.convert_many(inside.get(2..).unwrap_or_default(), ctx)?;
                Ok(join_with_separator(&items, &sep))
            }
            "NumericalApproximation" => self.convert(arg(0)?, ctx),
            // Fallback: substitute the arguments into the display boxes the
            // kernel itself uses for this template. This mirrors
            // `convertBuiltinTemplate[_, _, tbox]` in Templates.wl, which calls
            // `BoxForm`TemplateBoxToDisplayBoxes`, so both engines expand a tag
            // the same way instead of the native side keeping a tag whitelist.
            _ => {
                if ctx.template_depth >= MAX_TEMPLATE_DEPTH {
                    return Err(Unsupported::render(format!(
                        "template expansion too deep: {tag}"
                    )));
                }
                let expansion = tables::template_expansion(tag, inside.len()).ok_or_else(|| {
                    Unsupported::render(format!("unsupported TemplateBox tag: {tag}"))
                })?;
                let expanded = substitute_template_args(expansion, inside);
                self.convert(
                    &expanded,
                    Ctx {
                        template_depth: ctx.template_depth + 1,
                        ..ctx
                    },
                )
            }
        }
    }

    fn delimited(&self, semantic: &str, inner: String) -> Result<String, Unsupported> {
        Ok(format!("{semantic}({})", inner.trim()))
    }

    /// `formatDelimitedExpr` from `Templates.wl`: semantic functions by
    /// default, explicit `lr(...)` delimiters when the option asks for them.
    fn template_delimited(
        &self,
        semantic: &str,
        left: &str,
        right: &str,
        node: &Node,
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let inner = self.convert_inside_delim(node, ctx)?;
        if ctx.config.switch_is("DelimiterStyle", "Explicit") {
            if left == "|" && right == "|" {
                Ok(format!("lr(|{}|)", inner.trim()))
            } else {
                Ok(format!("lr({left} {} {right})", inner.trim()))
            }
        } else {
            self.delimited(semantic, inner)
        }
    }
}

/// Slot index encoded in a marker string (`0` based), if `text` is one.
fn marker_slot(text: &str) -> Option<usize> {
    let digits = text.strip_prefix(MARKER_OPEN)?.strip_suffix(MARKER_CLOSE)?;
    digits.parse::<usize>().ok()?.checked_sub(1)
}

/// Replace every marker in `tree` with the corresponding argument box.
fn substitute_template_args(tree: &Node, args: &[Node]) -> Node {
    match tree {
        Node::Str(text) => match marker_slot(text).and_then(|slot| args.get(slot)) {
            Some(found) => found.clone(),
            None => tree.clone(),
        },
        Node::Box {
            head,
            args: children,
        } => Node::Box {
            head: head.clone(),
            args: children
                .iter()
                .map(|child| substitute_template_args(child, args))
                .collect(),
        },
        other => other.clone(),
    }
}

/// Join items with an already-rendered separator string.
fn join_with_separator(items: &[String], sep: &str) -> String {
    let mut out = String::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push_str(sep);
        }
        out.push_str(item);
    }
    out
}
