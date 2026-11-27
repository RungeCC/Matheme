//! RowBox rendering: delimiter pairing, special rows, and token joining.

use super::consts::{
    DIFFERENTIAL, INTEGRAL, INVISIBLE_BASES, PIECEWISE_MARK, TIGHT_CALL_SUFFIXES,
    UNARY_CONTEXT_TOKENS,
};
use super::text::{is_number_string, map_char};
use super::{Converter, Ctx, arg, node_list, unsupported};
use crate::ast::Node;
use crate::error::Unsupported;
use crate::head::Head;
use crate::tables;

impl Converter<'_> {
    pub(super) fn row(&self, elems: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        if let Some(out) = self.special_row(elems, ctx)? {
            return Ok(out);
        }
        let Some((open_idx, close_idx)) = find_matching_delimiter_span(elems) else {
            return self.format_row_elements(elems, ctx);
        };
        let left = elems[open_idx].as_str().unwrap_or_default();
        let right = elems[close_idx].as_str().unwrap_or_default();
        let before = &elems[..open_idx];
        let mid = &elems[open_idx + 1..close_idx];
        let after = &elems[close_idx + 1..];

        // ( GridBox ), [ GridBox ], | GridBox |, ... -> mat(...)
        let clean_mid = mid
            .iter()
            .filter(|node| !super::matrix_padding_spacer(node))
            .collect::<Vec<_>>();
        if clean_mid.len() == 1
            && clean_mid[0].is(&Head::GridBox)
            && tables::matrix_delimiter(left).is_some()
            && !ctx
                .config
                .switch("MatrixDelimiter")
                .is_some_and(|value| value != "Automatic")
        {
            let grid = &clean_mid[0];
            let rows = node_list(
                grid.arg(0)
                    .ok_or_else(|| Unsupported::malformed("GridBox", "matrix grid has no rows"))?,
            )
            .unwrap_or_default();
            let wrapped = self.format_matrix(
                rows,
                tables::matrix_delimiter(left).unwrap(),
                grid.args().get(1..).unwrap_or_default(),
                ctx,
            )?;
            return self.join_row_parts(before, &wrapped, after, ctx);
        }

        if let Some(sem_fn) = tables::semantic_delimiter(left)
            .filter(|_| !ctx.config.switch_is("DelimiterStyle", "Explicit"))
        {
            let inner = self.row(mid, ctx)?;
            let wrapped = format!("{sem_fn}({})", inner.trim());
            return self.join_row_parts(before, &wrapped, after, ctx);
        }

        let Some(ld) = tables::left_delimiter(left) else {
            return Err(unsupported("RowBox delimiter"));
        };
        let Some(rd) = tables::right_delimiter(right) else {
            return Err(unsupported("RowBox delimiter"));
        };
        let inner = self.row(mid, ctx)?;
        let wrapped = if matches!(left, "(" | "[" | "{") && matches!(right, ")" | "]" | "}") {
            format!("{ld}{}{rd}", inner.trim())
        } else if ld == "|" && rd == "|" {
            format!("lr(|{}|)", inner.trim())
        } else {
            format!("lr({ld} {} {rd})", inner.trim())
        };
        self.join_row_parts(before, &wrapped, after, ctx)
    }

    pub(super) fn join_row_parts(
        &self,
        before: &[Node],
        middle: &str,
        after: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let mut tokens = Vec::with_capacity(3);
        if !before.is_empty() {
            tokens.push(self.row(before, ctx)?);
        }
        tokens.push(middle.to_owned());
        if !after.is_empty() {
            tokens.push(self.row(after, ctx)?);
        }
        Ok(join_tokens(&tokens))
    }

    pub(super) fn special_row(
        &self,
        elems: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<Option<String>, Unsupported> {
        if elems.len() == 3
            && elems[0].as_str() == Some("T")
            && elems[1].is(&Head::AdjustmentBox)
            && elems[1].args().first().and_then(Node::as_str) == Some("E")
            && elems[2].as_str() == Some("X")
        {
            return Ok(Some("\"TeX\"".into()));
        }
        if elems.len() == 5
            && elems[0].as_str() == Some("L")
            && elems[1].is(&Head::StyleBox)
            && elems[3].as_str() == Some("T")
            && elems[4].is(&Head::AdjustmentBox)
        {
            return Ok(Some("\"LaTeX\"".into()));
        }

        if elems.len() == 3 && is_inactive_power(elems.get(1)) {
            let base = elems[0].clone();
            let power = elems[2].clone();
            let node = Node::with_args(Head::SuperscriptBox, vec![base, power]);
            return Ok(Some(self.convert(&node, ctx)?));
        }

        // Piecewise: `\[Piecewise] GridBox[...]`, with or without a line break.
        if (elems.len() == 2 || (elems.len() == 3 && elems[1].as_str() == Some("\n")))
            && elems[0].as_str() == Some(PIECEWISE_MARK)
            && elems.last().is_some_and(|node| node.is(&Head::GridBox))
        {
            let grid = elems.last().unwrap();
            let rows = node_list(arg(&Head::GridBox, grid.args(), 0)?).ok_or_else(|| {
                Unsupported::malformed("GridBox", "piecewise payload must be a List of rows")
            })?;
            return Ok(Some(self.format_piecewise(rows, ctx)?));
        }

        // Integrals: `∫ f ⅆx` and `∫_a^b f ⅆx`.
        if elems.len() == 2
            && let Some(int_head) = integral_head(&elems[0])
            && let Some((integrand, var)) = integral_body(&elems[1])
        {
            let prefix = match int_head {
                IntegralHead::Plain => "integral".to_owned(),
                IntegralHead::Scripted { sub, sup } => format!(
                    "integral_{}^{}",
                    self.make_script(sub, ctx)?,
                    self.make_script(sup, ctx)?
                ),
            };
            let diff = map_char(DIFFERENTIAL, ctx)?;
            return Ok(Some(format!(
                "{} {} thin {} {}",
                prefix,
                self.convert(integrand, ctx)?.trim(),
                diff.trim(),
                self.convert(var, ctx)?.trim()
            )));
        }

        // Pre-subscripted operators such as HypergeometricPFQ: _p F_q.
        if elems.len() >= 2 && elems[0].is(&Head::SubscriptBox) {
            let head_args = elems[0].args();
            let invisible = head_args
                .first()
                .and_then(Node::as_str)
                .is_some_and(|s| INVISIBLE_BASES.contains(&s));
            if invisible && let Some(pre) = head_args.get(1) {
                let rendered = if elems[1].is(&Head::SubscriptBox) {
                    let base_args = elems[1].args();
                    match (base_args.first(), base_args.get(1)) {
                        (Some(base), Some(post)) => format!(
                            "attach({}, bl: {}, br: {})",
                            self.convert(base, ctx)?.trim(),
                            self.make_script(pre, ctx)?,
                            self.make_script(post, ctx)?
                        ),
                        _ => {
                            return Err(Unsupported::malformed(
                                "SubscriptBox",
                                "pre-subscripted pair needs base and post scripts",
                            ));
                        }
                    }
                } else {
                    format!(
                        "attach({}, bl: {})",
                        self.convert(&elems[1], ctx)?.trim(),
                        self.make_script(pre, ctx)?
                    )
                };
                if elems.len() == 2 {
                    return Ok(Some(rendered));
                }
                let tail = self.row(&elems[2..], ctx)?;
                return Ok(Some(join_tokens(&[rendered, tail])));
            }
        }

        for i in 1..elems.len().saturating_sub(2) {
            if elems[i].as_str() == Some(" ")
                && elems[i + 1].as_str() == Some("mod")
                && elems[i + 2].as_str() == Some(" ")
                && i > 0
                && i + 3 < elems.len()
            {
                let lhs = self.row(&elems[..i], ctx)?;
                let rhs = self.row(&elems[i + 3..], ctx)?;
                return Ok(Some(format!("({} mod {})", lhs.trim(), rhs.trim())));
            }
        }
        Ok(None)
    }

    pub(super) fn format_row_elements(
        &self,
        elems: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let fixed = fix_whitespace(elems);
        let tokens = self.convert_many(&fixed, ctx)?;
        let mut prev = String::new();
        let out = tokens
            .iter()
            .map(|token| -> Result<String, Unsupported> {
                let result = match token.as_str() {
                    "+" | "-" | "\u{2212}" => {
                        let sign = if token == "+" { "+" } else { "-" };
                        if unary_context_token(&prev) {
                            sign.to_owned()
                        } else {
                            format!(" {sign} ")
                        }
                    }
                    "=" | "<" | ">" => format!(" {token} "),
                    "," | ";" => map_char(token, ctx)?,
                    _ => token.clone(),
                };
                if !token.trim().is_empty() {
                    prev = token.clone();
                }
                Ok(result)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(join_tokens(&out))
    }
}

fn is_inactive_power(node: Option<&Node>) -> bool {
    let Some(node) = node else { return false };
    node.is(&Head::TagBox)
        && node.args().first().and_then(Node::as_str) == Some("^")
        && node.args().get(1).and_then(Node::as_str) == Some("InactiveToken")
}

/// The `∫` head of an integral row, possibly carrying bounds.
enum IntegralHead<'a> {
    Plain,
    Scripted { sub: &'a Node, sup: &'a Node },
}

fn is_integral_glyph(node: &Node) -> bool {
    node.as_str() == Some(INTEGRAL)
        || (node.is(&Head::StyleBox)
            && node.args().first().and_then(Node::as_str) == Some(INTEGRAL))
}

fn integral_head(node: &Node) -> Option<IntegralHead<'_>> {
    if is_integral_glyph(node) {
        return Some(IntegralHead::Plain);
    }
    if !node.is(&Head::SubsuperscriptBox) {
        return None;
    }
    let args = node.args();
    if !is_integral_glyph(args.first()?) {
        return None;
    }
    Some(IntegralHead::Scripted {
        sub: args.get(1)?,
        sup: args.get(2)?,
    })
}

/// `stripIdentityBoxes` rewrites a single-argument `RowBox` into `List`, and
/// both heads render identically, so row-shaped tests accept either.
fn row_like(node: &Node) -> bool {
    node.head().is_some_and(Head::is_row_like)
}

/// Split `RowBox[{integrand, RowBox[{ⅆ, var}]}]` into its two halves.
fn integral_body(node: &Node) -> Option<(&Node, &Node)> {
    if !row_like(node) {
        return None;
    }
    let args = node.args();
    if args.len() != 2 {
        return None;
    }
    let tail = &args[1];
    if !row_like(tail) {
        return None;
    }
    let tail_args = tail.args();
    if tail_args.len() != 2 {
        return None;
    }
    let differential = tail_args[0].as_str() == Some(DIFFERENTIAL)
        || (tail_args[0].is(&Head::StyleBox)
            && tail_args[0].args().first().and_then(Node::as_str) == Some(DIFFERENTIAL));
    if !differential {
        return None;
    }
    Some((&args[0], &tail_args[1]))
}

fn tall_box(node: &Node) -> bool {
    node.head().is_some_and(Head::is_tall) || node.args().iter().any(tall_box)
}

fn tall_box_slice(nodes: &[Node]) -> bool {
    nodes.iter().any(tall_box)
}

fn single_grid_inside(nodes: &[Node]) -> bool {
    let clean = nodes
        .iter()
        .filter(|node| !super::matrix_padding_spacer(node))
        .collect::<Vec<_>>();
    clean.len() == 1 && clean[0].is(&Head::GridBox)
}

fn find_matching_delimiter_span(elems: &[Node]) -> Option<(usize, usize)> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (i, node) in elems.iter().enumerate() {
        let Some(token) = node.as_str() else { continue };
        if let Some((open_idx, open_token)) = stack.last().cloned()
            && tables::matching_delimiter(&open_token) == Some(token)
        {
            stack.pop();
            let mid = &elems[open_idx + 1..i];
            if !matches!(open_token.as_str(), "(" | "[" | "{")
                || tall_box_slice(mid)
                || single_grid_inside(mid)
            {
                return Some((open_idx, i));
            }
        }
        if tables::matching_delimiter(token).is_some() {
            stack.push((i, token.to_owned()));
        }
    }
    None
}

fn test_next_number(node: &Node) -> bool {
    if let Some(s) = node.as_str() {
        return is_number_string(s);
    }
    if node.head().is_some_and(Head::unwraps_first_arg) {
        return node.arg(0).is_some_and(test_next_number);
    }
    false
}

fn fix_whitespace(elems: &[Node]) -> Vec<Node> {
    elems
        .iter()
        .enumerate()
        .map(|(i, node)| {
            if node.as_str() == Some(" ")
                && i > 0
                && i + 1 < elems.len()
                && elems[i - 1].as_str().is_some_and(is_number_string)
                && test_next_number(&elems[i + 1])
            {
                Node::Str("\u{205f}".into())
            } else {
                node.clone()
            }
        })
        .collect()
}

fn unary_context_token(prev: &str) -> bool {
    let t = prev.trim();
    if t.is_empty() {
        return true;
    }
    UNARY_CONTEXT_TOKENS.contains(&t)
        || matches!(t.chars().last(), Some('(' | '[' | '{' | ',' | ';'))
}

fn trim_start_spaces(s: &str) -> &str {
    s.trim_start_matches(' ')
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

fn is_letter(c: char) -> bool {
    c.is_alphabetic()
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}

fn tight_call_prefix(acc: &str) -> bool {
    if TIGHT_CALL_SUFFIXES
        .iter()
        .any(|suffix| acc.ends_with(suffix))
    {
        return true;
    }
    if let Some(pos) = acc.rfind("op(") {
        let inside = &acc[pos + 3..];
        if let Some(inner) = inside.strip_suffix(')') {
            return !inner.contains('(') && !inner.contains(')');
        }
    }
    false
}

/// Join already-rendered tokens into a single Typst fragment.
///
/// The WL reference rebuilds the accumulator every token, which is quadratic in
/// the row width. Here the accumulator is a single growable buffer and only the
/// few facts the separator rules need (last visible char, trailing space,
/// trimmed length) are tracked incrementally.
pub(super) fn join_tokens(tokens: &[String]) -> String {
    let mut out = String::new();
    // `trim_len` is the byte length of `out` after dropping trailing spaces, so
    // `&out[..trim_len]` is what the WL code calls `acc_trim`.
    let mut trim_len = 0usize;
    let mut last = ' ';
    let mut started = false;

    for raw in tokens {
        if raw.is_empty() {
            continue;
        }
        if !started {
            if raw == " " {
                continue;
            }
            out.push_str(trim_start_spaces(raw));
            trim_len = trimmed_len(&out);
            last = out[..trim_len].chars().next_back().unwrap_or(' ');
            started = !out.is_empty();
            continue;
        }
        let tok_trim = trim_start_spaces(raw);
        let tok_core = raw.trim();
        if tok_core.is_empty() {
            continue;
        }
        let acc_last = last;
        let tok_first = tok_core.chars().next().unwrap_or(' ');
        let acc_ends_with_space = out.as_bytes().last() == Some(&b' ');
        // The reference rebuilds from `acc_trim`, so trailing spaces never
        // survive into the next accumulator. Dropping them here keeps the
        // separator decisions identical without copying the whole buffer.
        out.truncate(trim_len);

        if tables::infix_spaced(tok_core) {
            out.push(' ');
            out.push_str(tok_core);
            out.push(' ');
        } else if tables::no_leading_space(tok_core)
            || matches!(tok_first, ')' | ']' | '}' | '\'' | '!')
        {
            out.push_str(tok_trim);
        } else if tok_core == "," || tok_core == ";" || tok_core == "\\," || tok_core == "\\;" {
            out.push_str(tok_core);
            out.push(' ');
        } else if tables::no_trailing_space_after(acc_last)
            || (tok_first == '(' && acc_ends_with_space && tight_call_prefix(&out[..trim_len]))
        {
            out.push_str(tok_trim);
        } else if acc_ends_with_space
            || raw.starts_with(' ')
            || (acc_last == '"' && (is_word_char(tok_first) || tok_first == ')'))
            || ((is_word_char(acc_last) || acc_last == ')')
                && (tok_first == '"' || is_letter(tok_first)))
            || (is_digit(acc_last) && is_digit(tok_first))
        {
            out.push(' ');
            out.push_str(tok_trim);
        } else {
            out.push_str(tok_trim);
        }

        trim_len = trimmed_len(&out);
        last = out[..trim_len].chars().next_back().unwrap_or(' ');
    }

    out.trim().to_owned()
}

pub(super) fn trimmed_len(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    end
}
