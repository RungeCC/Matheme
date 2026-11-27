//! Fractions and grids: FractionBox, GridBox matrices, and piecewise cases.

use super::consts::PIECEWISE_MARK;
use super::{Converter, Ctx, PARALLEL_MIN_ELEMS, arg, node_list};
use crate::ast::Node;
use crate::error::Unsupported;
use crate::head::Head;

use rayon::prelude::*;

impl Converter<'_> {
    pub(super) fn fraction(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::FractionBox;
        let num = arg(&head, args, 0)?;
        let den = arg(&head, args, 1)?;
        let (n, d) = self.convert_pair_inside(num, den, ctx)?;
        if ctx.config.switch_is("FractionStyle", "Slash") {
            Ok(format!("({n})/({d})"))
        } else {
            Ok(format!("frac({n}, {d})"))
        }
    }

    pub(super) fn grid(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::GridBox;
        let rows = node_list(arg(&head, args, 0)?).ok_or_else(|| {
            Unsupported::malformed("GridBox", "first argument must be a List of rows")
        })?;
        // Wolfram renders Piecewise as a one-row GridBox whose second cell is
        // the actual cases grid.
        if rows.len() == 1
            && let Some(cells) = node_list(&rows[0])
            && cells.first().and_then(Node::as_str) == Some(PIECEWISE_MARK)
            && let Some(inner) = cells.get(1).filter(|node| node.is(&Head::GridBox))
        {
            let inner_rows =
                node_list(inner.arg(0).ok_or_else(|| {
                    Unsupported::malformed("GridBox", "piecewise grid has no rows")
                })?)
                .unwrap_or_default();
            return self.format_piecewise(inner_rows, ctx);
        }
        self.format_matrix(rows, "None", args.get(1..).unwrap_or_default(), ctx)
    }

    pub(super) fn format_matrix(
        &self,
        rows: &[Node],
        delimiter: &str,
        opts: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        if !is_option_list(opts) {
            return Err(Unsupported::render(
                "GridBox options need the WL fallback".to_owned(),
            ));
        }
        let delim_arg: String = match delimiter {
            "Automatic" => String::new(),
            "None" => "delim: #none, ".into(),
            other => format!("delim: {other}, "),
        };
        let align_arg = matrix_alignment(opts);
        let render_row = |row: &Node| -> Result<String, Unsupported> {
            let cells = node_list(row)
                .ok_or_else(|| Unsupported::malformed("GridBox", "row must be a List of cells"))?;
            let rendered = cells
                .iter()
                .map(|cell| {
                    self.convert(
                        cell,
                        Ctx {
                            matrix_cell: true,
                            cases_cell: false,
                            ..ctx
                        },
                    )
                    .map(|s| s.trim().to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rendered.join(", "))
        };
        // Matrices are usually small; scheduling a 2x2 across the pool costs
        // more than it saves. Rows only go parallel when the whole tree
        // already amortises rayon and there are enough of them.
        let rendered_rows = if ctx.parallel && rows.len() >= PARALLEL_MIN_ELEMS {
            rows.par_iter()
                .map(render_row)
                .collect::<Result<Vec<_>, _>>()?
        } else {
            rows.iter().map(render_row).collect::<Result<Vec<_>, _>>()?
        };
        Ok(format!(
            "mat({}{}{})",
            delim_arg,
            align_arg,
            rendered_rows.join("; ")
        ))
    }

    pub(super) fn format_piecewise(
        &self,
        grid: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let render_row = |row: &Node| -> Result<String, Unsupported> {
            let cells = node_list(row).ok_or_else(|| {
                Unsupported::malformed("Piecewise", "row must be a List of cells")
            })?;
            let rendered = cells
                .iter()
                .map(|cell| {
                    self.convert(
                        cell,
                        Ctx {
                            matrix_cell: false,
                            cases_cell: true,
                            ..ctx
                        },
                    )
                    .map(|s| s.trim().to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rendered.join(" & "))
        };
        let rows = if ctx.parallel && grid.len() >= PARALLEL_MIN_ELEMS {
            grid.par_iter()
                .map(render_row)
                .collect::<Result<Vec<_>, _>>()?
        } else {
            grid.iter().map(render_row).collect::<Result<Vec<_>, _>>()?
        };
        Ok(format!("cases({})", rows.join(", ")))
    }
}

fn is_option_list(nodes: &[Node]) -> bool {
    nodes
        .iter()
        .all(|node| node.is(&Head::Rule) || node.is(&Head::RuleDelayed))
}

fn matrix_alignment(opts: &[Node]) -> &'static str {
    let Some(option) = opts
        .iter()
        .find(|node| node.args().first().and_then(Node::as_str) == Some("ColumnAlignments"))
    else {
        return "";
    };
    let value = option.args().get(1);
    let symbol = match value {
        Some(Node::Sym(name)) => Some(name.as_str()),
        Some(Node::Str(name)) => Some(name.as_str()),
        Some(node) if node.is(&Head::List) => node.args().first().and_then(|item| {
            item.as_str().or(match item {
                Node::Sym(name) => Some(name.as_str()),
                _ => None,
            })
        }),
        _ => None,
    };
    match symbol {
        Some("Left") => "align: #left, ",
        Some("Right") => "align: #right, ",
        _ => "",
    }
}
