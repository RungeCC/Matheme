//! Script positions: sub/superscripts, overscripts, underscripts, and the
//! string form a script takes.

use super::consts::{
    BIG_OPERATORS, LIM_LIKE_OPERATORS, OVER_ACCENTS, OVERBRACE_MARK, OVERPAREN_MARK,
    SCRIPT_STYLE_PREFIXES, UNDER_ACCENTS, UNDERBRACE_MARK, UNDERPAREN_MARK, accent,
};
use super::{Converter, Ctx, arg, node_list};
use crate::ast::Node;
use crate::error::Unsupported;
use crate::head::Head;

impl Converter<'_> {
    pub(super) fn superscript(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::SuperscriptBox;
        let base = arg(&head, args, 0)?;
        let power = arg(&head, args, 1)?;
        let base_str = self.convert(base, ctx)?;
        if let Some(text) = power.as_str()
            && !text.is_empty()
            && text.chars().all(|c| c == '\'' || c == '\u{2032}')
        {
            return Ok(format!(
                "{}{}",
                base_str.trim(),
                "'".repeat(text.chars().count())
            ));
        }
        let sup = self.make_script(power, ctx)?;
        Ok(format!("{}^{}", base_str.trim(), sup))
    }

    pub(super) fn subscript(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::SubscriptBox;
        let base = arg(&head, args, 0)?;
        let sub = arg(&head, args, 1)?;
        let base_str = self.convert(base, ctx)?;
        let b = base_str.trim();
        if b.contains('^') {
            Ok(format!(
                "attach({}, b: {})",
                b,
                self.convert_inside_delim(sub, ctx)?
            ))
        } else {
            Ok(format!("{}_{}", b, self.make_script(sub, ctx)?))
        }
    }

    pub(super) fn subsuperscript(
        &self,
        args: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let head = Head::SubsuperscriptBox;
        let base = arg(&head, args, 0)?;
        let sub = arg(&head, args, 1)?;
        let sup = arg(&head, args, 2)?;
        Ok(format!(
            "{}_{}^{}",
            self.convert(base, ctx)?.trim(),
            self.make_script(sub, ctx)?,
            self.make_script(sup, ctx)?
        ))
    }

    pub(super) fn make_script(&self, node: &Node, ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let keep_parens = explicit_parenthesized(node);
        let inner = self.convert_inside_delim(node, ctx)?;
        Ok(make_script_string(&inner, keep_parens))
    }

    pub(super) fn overscript(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::OverscriptBox;
        let base = arg(&head, args, 0)?;
        let over = arg(&head, args, 1)?;
        let over_text = strip_quotes(over.as_str().unwrap_or_default());
        let inner = || self.convert_inside_delim(base, ctx);

        if let Some(nested) = over.as_str().and_then(strip_brace_pair)
            && nested == OVERBRACE_MARK
        {
            return Ok(format!(
                "overbrace({}, {})",
                inner()?,
                self.convert_inside_delim(&Node::Str(nested.to_owned()), ctx)?
            ));
        }
        if base.is(&Head::OverscriptBox) {
            let base_args = base.args();
            if is_quoted_token(base_args.get(1), OVERBRACE_MARK) {
                return Ok(format!(
                    "overbrace({}, {})",
                    self.convert_inside_delim(base_args.first().unwrap(), ctx)?,
                    self.convert_inside_delim(over, ctx)?
                ));
            }
            if is_quoted_token(base_args.get(1), OVERPAREN_MARK) {
                return Ok(format!(
                    "overparen({}, {})",
                    self.convert_inside_delim(base_args.first().unwrap(), ctx)?,
                    self.convert_inside_delim(over, ctx)?
                ));
            }
        }
        if over.is(&Head::OverscriptBox) {
            let over_args = over.args();
            if is_quoted_token(over_args.first(), OVERBRACE_MARK) {
                return Ok(format!(
                    "overbrace({}, {})",
                    inner()?,
                    self.convert_inside_delim(over_args.get(1).unwrap(), ctx)?
                ));
            }
        }
        if is_quoted_token(Some(over), OVERBRACE_MARK) {
            return Ok(format!("overbrace({})", inner()?));
        }
        if is_quoted_token(Some(over), OVERPAREN_MARK) {
            return Ok(format!("overparen({})", inner()?));
        }
        if big_operator(over_text) {
            return Ok(format!(
                "{}^{} ",
                self.convert(base, ctx)?.trim(),
                self.make_script(over, ctx)?
            ));
        }
        // `_` means bar over a single-character base, overline otherwise.
        let accent = if over_text == "_" && base.as_str().is_some_and(|s| s.chars().count() == 1) {
            "bar"
        } else {
            accent(OVER_ACCENTS, over_text).unwrap_or("")
        };
        match accent {
            "bar" => Ok(format!("bar({})", self.convert(base, ctx)?.trim())),
            "overline" => Ok(format!("overline({})", inner()?)),
            "accent-arrow" => Ok(format!("accent({}, arrow.l)", inner()?)),
            "arrow" => Ok(format!("arrow({})", inner()?)),
            "accent-both" => Ok(format!("accent({}, arrow.l.r)", inner()?)),
            other if !other.is_empty() => Ok(format!("{other}({})", inner()?)),
            _ => Ok(format!(
                "attach({}, t: {})",
                inner()?,
                self.convert_inside_delim(over, ctx)?
            )),
        }
    }

    pub(super) fn underscript(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::UnderscriptBox;
        let base = arg(&head, args, 0)?;
        let under = arg(&head, args, 1)?;
        let under_text = strip_quotes(under.as_str().unwrap_or_default());
        let inner = || self.convert_inside_delim(base, ctx);

        if base.is(&Head::UnderscriptBox) {
            let base_args = base.args();
            if is_quoted_token(base_args.get(1), UNDERBRACE_MARK) {
                return Ok(format!(
                    "underbrace({}, {})",
                    self.convert_inside_delim(base_args.first().unwrap(), ctx)?,
                    self.convert_inside_delim(under, ctx)?
                ));
            }
            if is_quoted_token(base_args.get(1), UNDERPAREN_MARK) {
                return Ok(format!(
                    "underparen({}, {})",
                    self.convert_inside_delim(base_args.first().unwrap(), ctx)?,
                    self.convert_inside_delim(under, ctx)?
                ));
            }
        }
        if under.is(&Head::UnderscriptBox) {
            let under_args = under.args();
            if is_quoted_token(under_args.first(), UNDERBRACE_MARK) {
                return Ok(format!(
                    "underbrace({}, {})",
                    inner()?,
                    self.convert_inside_delim(under_args.get(1).unwrap(), ctx)?
                ));
            }
        }
        if is_quoted_token(Some(under), UNDERBRACE_MARK) {
            return Ok(format!("underbrace({})", inner()?));
        }
        if is_quoted_token(Some(under), UNDERPAREN_MARK) {
            return Ok(format!("underparen({})", inner()?));
        }
        if big_operator(under_text) || LIM_LIKE_OPERATORS.contains(&under_text) {
            return Ok(format!(
                "{}_{} ",
                self.convert(base, ctx)?.trim(),
                self.make_script(under, ctx)?
            ));
        }
        match accent(UNDER_ACCENTS, under_text).unwrap_or("") {
            "underline" => Ok(format!("underline({})", inner()?)),
            "attach-arrow.l.r" => Ok(format!("attach({}, b: arrow.l.r)", inner()?)),
            "attach-arrow.r" => Ok(format!("attach({}, b: arrow.r)", inner()?)),
            _ => Ok(format!(
                "attach({}, b: {})",
                inner()?,
                self.convert_inside_delim(under, ctx)?
            )),
        }
    }

    pub(super) fn underoverscript(
        &self,
        args: &[Node],
        ctx: Ctx<'_>,
    ) -> Result<String, Unsupported> {
        let head = Head::UnderoverscriptBox;
        let base = arg(&head, args, 0)?;
        let under = arg(&head, args, 1)?;
        let over = arg(&head, args, 2)?;
        if big_operator(base.as_str().unwrap_or_default()) {
            return Ok(format!(
                "{}_{}^{} ",
                self.convert(base, ctx)?.trim(),
                self.make_script(under, ctx)?,
                self.make_script(over, ctx)?
            ));
        }
        Ok(format!(
            "attach({}, b: {}, t: {})",
            self.convert_inside_delim(base, ctx)?,
            self.convert_inside_delim(under, ctx)?,
            self.convert_inside_delim(over, ctx)?
        ))
    }

    /// Parenthesise script bases that carry a visible infix operator,
    /// mirroring `formatScriptBaseBox` in `Templates.wl`.
    pub(super) fn script_base(&self, node: &Node, ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let s = self.convert(node, ctx)?;
        if node.is(&Head::RowBox)
            && node.args().iter().any(|item| {
                matches!(
                    item.as_str(),
                    Some("+") | Some("-") | Some("\u{2212}") | Some("\u{00b1}") | Some("\u{2213}")
                )
            })
        {
            Ok(format!("({})", s.trim()))
        } else {
            Ok(s.trim().to_owned())
        }
    }
}

fn big_operator(s: &str) -> bool {
    BIG_OPERATORS.contains(&s)
}

fn is_quoted_token(node: Option<&Node>, token: &str) -> bool {
    node.and_then(Node::as_str).is_some_and(|s| {
        (s == token)
            || (s.len() >= 2
                && s.starts_with('"')
                && s.ends_with('"')
                && &s[1..s.len() - 1] == token)
    })
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn strip_brace_pair(s: &str) -> Option<&str> {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        if inner.starts_with('"') && inner.ends_with('"') {
            return Some(&inner[1..inner.len() - 1]);
        }
    }
    None
}

/// The string a script position renders as: bare when it is a single token or
/// an already-styled fragment, parenthesised otherwise.
pub(super) fn make_script_string(raw: &str, force_keep_parens: bool) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return "()".into();
    }
    if force_keep_parens {
        return format!("({s})");
    }
    if s.chars().count() == 1
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '*' | '+' | '-'))
    {
        return s.to_owned();
    }
    let dotted_ascii = s
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphabetic()))
        && s.contains('.');
    if s.chars().all(|c| c.is_ascii_alphabetic()) || dotted_ascii {
        return s.to_owned();
    }
    if SCRIPT_STYLE_PREFIXES.iter().any(|prefix| {
        s.starts_with(&format!("{prefix}("))
            && s.ends_with(')')
            && !s[prefix.len() + 1..s.len() - 1].contains('(')
            && !s[prefix.len() + 1..s.len() - 1].contains(')')
    }) {
        return s.to_owned();
    }
    format!("({s})")
}

fn explicit_parenthesized(node: &Node) -> bool {
    if !node.is(&Head::RowBox) {
        return false;
    }
    let args = node.args();
    let elems = match args {
        [only] if node_list(only).is_some() => node_list(only).unwrap(),
        _ => args,
    };
    elems.first().and_then(Node::as_str) == Some("(")
        && elems.last().and_then(Node::as_str) == Some(")")
}
