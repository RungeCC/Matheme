//! Box DSL renderer.
//!
//! This is a direct port of the common path of `Kernel/TypstForm/Boxes.wl`,
//! split by box family: [`row`] (RowBox and token joining), [`scripts`]
//! (sub/superscripts and accents), [`grid`] (fractions, matrices, piecewise),
//! [`style`] (StyleBox/Cell), [`template`] (TemplateBox) and [`text`] (string
//! tokens and character mapping). Hand-written constant tables live in
//! [`consts`].
//!
//! Nodes that need Wolfram evaluation (user rules, `MakeBoxes` callbacks,
//! colors, unknown templates, ...) return [`Unsupported`] so the WL layer can
//! fall back to the original converter; a tree that violates the documented
//! box grammar instead returns `Stage::Malformed`. Features that are
//! recognized but deliberately not laid out are reported as
//! [`ConversionWarning`]s rather than dropped silently.

mod consts;
mod grid;
mod row;
mod scripts;
mod style;
mod template;
mod text;

use std::sync::Mutex;

use rayon::prelude::*;

use crate::ast::Node;
use crate::config::Config;
use crate::error::Unsupported;
use crate::head::Head;

/// A box-tree feature the renderer recognized but deliberately does not lay
/// out (for example a StyleBox font option outside bold/italic/underline).
/// Reported back to the caller instead of being dropped silently. A render
/// that fails never carries warnings: the WL fallback re-renders the whole
/// tree and would duplicate them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionWarning {
    /// Where the warning came from, e.g. `StyleBox/FontWeight`.
    pub field: String,
    pub message: String,
}

#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub(crate) config: &'a Config,
    pub(crate) matrix_cell: bool,
    pub(crate) cases_cell: bool,
    pub(crate) quantity_unit: bool,
    pub(crate) delimiter_depth: usize,
    /// Set only for the outermost row of a tree big enough to amortise rayon.
    pub(crate) parallel: bool,
    /// Nesting depth of `TemplateBox` expansion; guards against a template that
    /// (indirectly) expands back into itself.
    pub(crate) template_depth: u8,
}

/// Rows below this many siblings never pay for a `par_iter`.
pub(crate) const PARALLEL_MIN_ELEMS: usize = 64;

/// Total node budget under which the whole tree is rendered sequentially.
/// Sized from the Criterion baseline: rayon costs a few microseconds per job,
/// and one node renders in well under a microsecond.
pub(crate) const PARALLEL_MIN_NODES: usize = 4096;

/// A slice is only worth splitting when each element carries at least this
/// much work. One very wide row of leaves loses ~20% to scheduling, while a
/// list of medium rows gains ~1.5×; the per-element cost is what separates them.
pub(crate) const PARALLEL_MIN_ELEM_NODES: usize = 8;

/// Count nodes, stopping early once `cap` is reached so the pre-pass stays
/// cheap. This is the only node counter; parallel-budget checks share it.
pub(crate) fn node_count(node: &Node, cap: usize) -> usize {
    fn walk(node: &Node, cap: usize, seen: usize) -> usize {
        if seen >= cap {
            return seen;
        }
        let mut total = seen + 1;
        for child in node.args() {
            total = walk(child, cap, total);
            if total >= cap {
                break;
            }
        }
        total
    }
    walk(node, cap, 0)
}

/// The payload of a box argument that must be a `List`.
pub(crate) fn node_list(node: &Node) -> Option<&[Node]> {
    if node.is(&Head::List) {
        Some(node.args())
    } else {
        None
    }
}

/// Whether a node is one of the spacer tokens Wolfram pads matrix delimiter
/// rows with (`matrixPaddingSpacerQ` in `Boxes.wl`).
pub(crate) fn matrix_padding_spacer(node: &Node) -> bool {
    let Some(s) = node.as_str() else { return false };
    consts::INVISIBLE_SPACERS.contains(&s) || consts::MATRIX_PADDING_SPACERS.contains(&s)
}

/// A head the renderer knows but has no rule for — the same set the reference
/// converter leaves to `handleUnsupportedBox`.
pub(crate) fn unsupported(head: &str) -> Unsupported {
    Unsupported::box_head(head, format!("unsupported box head: {head}"))
}

/// Borrow argument `index` of a box, or report malformed shape. The documented
/// box grammar fixes each head's arity, so a tree that falls short is
/// mis-shaped input — not a missing rendering rule. The WL fallback still
/// receives the tree, but diagnostics say `Malformed` instead of claiming the
/// head is unsupported.
pub(crate) fn arg<'a>(
    head: &Head,
    args: &'a [Node],
    index: usize,
) -> Result<&'a Node, Unsupported> {
    args.get(index).ok_or_else(|| {
        Unsupported::malformed(
            head.name(),
            format!(
                "expected at least {} argument(s), got {}",
                index + 1,
                args.len()
            ),
        )
    })
}

pub struct Converter<'a> {
    config: &'a Config,
    /// Recognized-but-dropped features, drained by `render_with_warnings`.
    /// The mutex makes `&Converter` shareable across rayon workers.
    warnings: Mutex<Vec<ConversionWarning>>,
}

impl<'a> Converter<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            warnings: Mutex::new(Vec::new()),
        }
    }

    pub fn render(&self, node: &Node) -> Result<String, Unsupported> {
        let ctx = Ctx {
            config: self.config,
            matrix_cell: false,
            cases_cell: false,
            quantity_unit: false,
            delimiter_depth: 0,
            parallel: node_count(node, PARALLEL_MIN_NODES) >= PARALLEL_MIN_NODES,
            template_depth: 0,
        };
        let out = self.convert(node, ctx)?;
        Ok(out.trim().to_owned())
    }

    /// Like [`Converter::render`], but also drains the warnings the render
    /// produced. Warnings are only returned for successful renders; a failed
    /// render falls back to the WL engine, which would duplicate them.
    pub fn render_with_warnings(
        &self,
        node: &Node,
    ) -> Result<(String, Vec<ConversionWarning>), Unsupported> {
        let out = self.render(node)?;
        let warnings =
            std::mem::take(&mut *self.warnings.lock().unwrap_or_else(|err| err.into_inner()));
        Ok((out, warnings))
    }

    pub(crate) fn warn(&self, field: impl Into<String>, message: impl Into<String>) {
        self.warnings
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(ConversionWarning {
                field: field.into(),
                message: message.into(),
            });
    }

    pub(crate) fn convert_many(
        &self,
        nodes: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<Vec<String>, Unsupported> {
        if ctx.parallel
            && nodes.len() >= PARALLEL_MIN_ELEMS
            && nodes
                .iter()
                .take(4)
                .map(|node| node_count(node, PARALLEL_MIN_ELEM_NODES))
                .max()
                .unwrap_or(0)
                >= PARALLEL_MIN_ELEM_NODES
        {
            // Children of a parallel row stay sequential: nesting rayon jobs in
            // a tree this shallow only adds scheduling overhead.
            let inner = Ctx {
                parallel: false,
                ..ctx
            };
            nodes
                .par_iter()
                .map(|node| self.convert(node, inner))
                .collect()
        } else {
            nodes.iter().map(|node| self.convert(node, ctx)).collect()
        }
    }

    pub(crate) fn convert(&self, node: &Node, ctx: Ctx<'_>) -> Result<String, Unsupported> {
        match node {
            Node::Str(value) => text::format_string_token(value, ctx),
            Node::Num(value) => Ok(value.clone()),
            Node::Sym(name) => Err(Unsupported::render(format!(
                "bare symbol in box tree: {name}"
            ))),
            Node::Box { head, args } => match head {
                Head::RowBox | Head::List => {
                    // `strip_identity` rewrites a one-element `RowBox` into
                    // `List`, and both heads render as a plain row.
                    let elems = match args.as_slice() {
                        [
                            Node::Box {
                                head: Head::List,
                                args,
                            },
                        ] => args,
                        _ => args,
                    };
                    self.row(elems, ctx)
                }
                Head::FractionBox => self.fraction(args, ctx),
                Head::SqrtBox => {
                    let x = arg(head, args, 0)?;
                    Ok(format!("sqrt({})", self.convert_inside_delim(x, ctx)?))
                }
                Head::RadicalBox => {
                    let x = arg(head, args, 0)?;
                    let y = arg(head, args, 1)?;
                    Ok(format!(
                        "root({}, {})",
                        self.convert_inside_delim(y, ctx)?,
                        self.convert_inside_delim(x, ctx)?
                    ))
                }
                Head::SuperscriptBox => self.superscript(args, ctx),
                Head::SubscriptBox => self.subscript(args, ctx),
                Head::SubsuperscriptBox => self.subsuperscript(args, ctx),
                Head::OverscriptBox => self.overscript(args, ctx),
                Head::UnderscriptBox => self.underscript(args, ctx),
                Head::UnderoverscriptBox => self.underoverscript(args, ctx),
                Head::GridBox => self.grid(args, ctx),
                Head::StyleBox | Head::Cell => self.style(args, ctx),
                Head::TemplateBox => self.template(args, ctx),
                Head::FormBox
                | Head::BoxData
                | Head::TextData
                | Head::TemplateArgBox
                | Head::PaneBox
                | Head::PanelBox
                | Head::ButtonBox
                | Head::InputFieldBox
                | Head::ErrorBox => self.simple_wrapper(head, args, ctx),
                Head::TagBox => self.tag_box(args, ctx),
                Head::FrameBox => {
                    let inner = arg(head, args, 0)?;
                    Ok(format!(
                        "#box(stroke: 0.5pt, inset: 2pt, ${}$)",
                        self.convert_inside_delim(inner, ctx)?
                    ))
                }
                Head::Notebook => {
                    let cells = self.convert_many(args, ctx)?;
                    Ok(cells
                        .into_iter()
                        .map(|s| s.trim().to_owned())
                        .collect::<Vec<_>>()
                        .join(" \\\n\\\n"))
                }
                // Recognised but deliberately not laid out. The reference
                // converter has no rule for these either, so reporting
                // `Unsupported` hands the node to the WL fallback — and its
                // message — instead of silently dropping the box. Listing them
                // keeps this match exhaustive, so a new head cannot slip into
                // the catch-all unnoticed.
                Head::ItemBox
                | Head::RotationBox
                | Head::OverlayBox
                | Head::InsetBox
                | Head::NamespaceBox
                | Head::DynamicWrapperBox
                | Head::BoxParentBox
                | Head::EvaluationBox
                | Head::CounterBox
                | Head::ValueBox
                | Head::OptionValueBox
                | Head::DynamicModuleBox
                | Head::Dynamic
                | Head::AdjustmentBox
                | Head::InterpretationBox
                | Head::TooltipBox
                | Head::DynamicBox
                | Head::Rule
                | Head::RuleDelayed
                | Head::RgbColor
                | Head::GrayLevel
                | Head::Hue
                | Head::CmykColor => Err(unsupported(head.name())),
                Head::Other(_) => Err(unsupported(head.name())),
            },
        }
    }

    fn simple_wrapper(
        &self,
        head: &Head,
        args: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let inner = arg(head, args, 0)?;
        let out = self.convert(inner, ctx)?;
        if *head == Head::ErrorBox {
            Ok(format!("underline({})", out.trim()))
        } else {
            Ok(out)
        }
    }

    fn tag_box(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let inner = arg(&Head::TagBox, args, 0)?;
        let tag = args.get(1).and_then(Node::as_str);
        if tag != Some("InactiveToken") {
            return self.convert(inner, ctx);
        }
        let op = inner.as_str().ok_or_else(|| {
            Unsupported::malformed("TagBox", "an InactiveToken tag needs a String operator")
        })?;
        match op {
            "+" => Ok(" + ".into()),
            "-" => Ok(" - ".into()),
            "*" => Ok(" times ".into()),
            "/" => Ok("/".into()),
            other => text::format_string_token(other, ctx),
        }
    }

    /// Split binary subtrees across the Rayon pool once they are large enough
    /// to amortize scheduling — and only when the tree as a whole earned the
    /// parallel budget (`ctx.parallel`), so a small expression holding one big
    /// fraction stays sequential.
    fn convert_pair_inside(
        &self,
        left: &Node,
        right: &Node,
        ctx: Ctx<'_>,
    ) -> Result<(String, String), Unsupported> {
        if ctx.parallel && node_count(left, 64) + node_count(right, 64) >= 32 {
            let (a, b) = rayon::join(
                || self.convert_inside_delim(left, ctx),
                || self.convert_inside_delim(right, ctx),
            );
            Ok((a?, b?))
        } else {
            Ok((
                self.convert_inside_delim(left, ctx)?,
                self.convert_inside_delim(right, ctx)?,
            ))
        }
    }

    pub(crate) fn convert_inside_delim(
        &self,
        node: &Node,
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let inner = self.convert(
            node,
            Ctx {
                matrix_cell: false,
                cases_cell: false,
                delimiter_depth: ctx.delimiter_depth + 1,
                ..ctx
            },
        )?;
        Ok(inner.trim().to_owned())
    }

    pub(crate) fn convert_quantity(
        &self,
        node: &Node,
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let out = self.convert(
            node,
            Ctx {
                quantity_unit: true,
                ..ctx
            },
        )?;
        Ok(out.trim().to_owned())
    }
}
