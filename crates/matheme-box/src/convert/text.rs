//! String-token formatting: the classification pipeline from `Boxes.wl`'s
//! `formatStringToken`, plus per-character mapping.

use matheme_wl::parse_symbol_name;

use super::Ctx;
use super::scripts::make_script_string;
use crate::ast::Node;
use crate::error::Unsupported;
use crate::tables;

pub(super) fn escape_typst_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn is_number_string(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[i] == b'+' || bytes[i] == b'-' {
        i += 1;
    }
    let mut digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    i == bytes.len() && digits > 0
}

fn valid_precision(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let (digits, tail) = split_digits(s);
    tail.is_empty()
        || (tail.starts_with('.') && tail[1..].chars().all(|c| c.is_ascii_digit()))
            && !digits.is_empty()
}

fn split_digits(s: &str) -> (&str, &str) {
    let index = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    s.split_at(index)
}

fn valid_exponent(s: &str) -> bool {
    let body = if s.starts_with('+') || s.starts_with('-') {
        &s[1..]
    } else {
        s
    };
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit())
}

fn format_wolfram_number_string(text: &str) -> Option<String> {
    let (mantissa, exponent) = if let Some(tick) = text.find('`') {
        let mantissa = &text[..tick];
        let after = &text[tick + 1..];
        let (precision, exponent) = if let Some(pos) = after.find("*^") {
            (&after[..pos], Some(&after[pos + 2..]))
        } else {
            (after, None)
        };
        if !valid_precision(precision) {
            return None;
        }
        (mantissa, exponent)
    } else {
        let pos = text.find("*^")?;
        (&text[..pos], Some(&text[pos + 2..]))
    };
    if !is_number_string(mantissa) {
        return None;
    }
    if exponent.is_some_and(|exp| !valid_exponent(exp)) {
        return None;
    }
    let mantissa = mantissa.trim_end_matches('.');
    match exponent {
        Some(exp) => Some(format!(
            "{mantissa} times 10^{}",
            make_script_string(exp, false)
        )),
        None => Some(mantissa.to_owned()),
    }
}

fn ascii_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

fn ascii_prose(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | ',' | '.' | ':' | ';' | '!' | '?' | '(' | ')' | '-')
    }) && s.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2
}

pub(super) fn format_string_token(text: &str, ctx: Ctx<'_>) -> Result<String, Unsupported> {
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        let unquoted = text[1..text.len() - 1].replace("\\\"", "\"");
        if ctx.quantity_unit {
            match unquoted.as_str() {
                "/" => return Ok("\\/".into()),
                "\u{00b0}" => return Ok("degree".into()),
                _ => {}
            }
        }
        return Ok(format!("\"{}\"", escape_typst_string(&unquoted)));
    }
    if text == " " {
        return Ok(" ".into());
    }
    if is_number_string(text) {
        return Ok(text.to_owned());
    }
    if (text.contains('`') || text.contains("*^"))
        && let Some(number) = format_wolfram_number_string(text)
    {
        return Ok(number);
    }
    if text.contains('`') {
        // `matheme-wl` owns the symbol-name grammar; text that parses as a
        // context-qualified name strips its context unless the option asks to
        // keep it, and anything else (including precision-annotated numbers
        // the parser rejected) stays a quoted literal.
        match parse_symbol_name(text) {
            Some(symbol)
                if symbol.context.is_some() && !ctx.config.switch_is("StripContext", "False") =>
            {
                return format_string_token(symbol.short, ctx);
            }
            _ => return Ok(format!("\"{}\"", escape_typst_string(text))),
        }
    }
    if text.chars().count() == 1 {
        return map_char(text, ctx);
    }
    if let Some(op) = ctx.config.operator(text).or_else(|| tables::operator(text)) {
        return Ok(format!("{op} "));
    }
    if let Some(word) = tables::greek_word(text) {
        return Ok(word.to_owned());
    }
    if ascii_identifier(text) {
        let quoted = format!("\"{}\"", escape_typst_string(text));
        return match ctx.config.switch("MultiLetterStyle").unwrap_or("Quoted") {
            "Italic" => Ok(format!("italic({quoted})")),
            "Upright" => Ok(format!("upright({quoted})")),
            _ => Ok(quoted),
        };
    }
    if ascii_prose(text) {
        return Ok(format!("\"{}\"", escape_typst_string(text)));
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        out.push_str(&map_char(&c.to_string(), ctx)?);
    }
    Ok(out)
}

/// Name of a node that denotes text, whether it arrived as a string or as a
/// symbol. Box options such as `FontWeight -> Bold` reach us as symbols.
pub(super) fn text_of(node: &Node) -> Option<&str> {
    match node {
        Node::Str(value) => Some(value),
        Node::Sym(name) => Some(name),
        _ => None,
    }
}

pub(super) fn map_char(text: &str, ctx: Ctx<'_>) -> Result<String, Unsupported> {
    if text == "," && (ctx.matrix_cell || ctx.cases_cell) {
        return Ok("\\, ".into());
    }
    if text == ";" && ctx.matrix_cell {
        return Ok("\\; ".into());
    }
    if text == "/" && ctx.quantity_unit {
        return Ok("\\/".into());
    }
    if let Some(mapped) = ctx.config.char(text).or_else(|| tables::typst_char(text)) {
        return Ok(mapped.to_owned());
    }
    if let Some(c) = text.chars().next() {
        let code = c as u32;
        if (0xe000..=0xf8ff).contains(&code) || (0xf0000..=0x10ffff).contains(&code) {
            return Err(Unsupported::render(format!(
                "unmapped private-use character U+{code:04X}"
            )));
        }
    }
    Ok(text.to_owned())
}
