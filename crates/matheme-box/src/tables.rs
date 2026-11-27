//! Static lookup tables generated from `Kernel/TypstForm/Characters.wl` and
//! `Boxes.wl` by `scripts/generate-box-tables.wl`.
//!
//! The Wolfram package remains the source of truth; this file only mirrors its
//! default configuration so the Rust hot path needs neither a kernel nor a
//! WSTP round trip for table lookups. A generated `"$typstTableSourceHash"`
//! records the MD5 of the source files; `scripts/build.nu` compares it against
//! the current sources so an unregenerated snapshot fails the build instead of
//! silently diverging from the WL engine.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::ast::Node;
use crate::head::Head;

const TABLES_JSON: &str = include_str!("tables.json");

/// Every top-level key the generated file may contain. A generator that adds
/// or drops a table must update this list, so a stale snapshot cannot silently
/// lose a table (as happened with `$typstNoTrailingSpaceAfter`).
pub const TABLE_KEYS: &[&str] = &[
    "$typstDefaultCharMap",
    "$typstDefaultOperatorMap",
    "$typstGreekWords",
    "$typstMatchingDelimiter",
    "$typstSemanticDelimiterFunctions",
    "$typstMatrixDelimiterSpec",
    "$typstLeftDelimiters",
    "$typstRightDelimiters",
    "$typstInfixSpacedOps",
    "$typstNoLeadingSpaceBefore",
    "$typstNoTrailingSpaceAfter",
    "$typstTemplateExpansions",
    "$typstTableSourceHash",
];

pub struct Tables {
    char_map: HashMap<String, String>,
    operator_map: HashMap<String, String>,
    greek_words: HashMap<String, String>,
    matching_delimiter: HashMap<String, String>,
    semantic_delimiters: HashMap<String, String>,
    matrix_delimiters: HashMap<String, String>,
    left_delimiters: HashMap<String, String>,
    right_delimiters: HashMap<String, String>,
    infix_spaced: HashSet<String>,
    no_leading_space: HashSet<String>,
    no_trailing_space_after: HashSet<char>,
    /// `<tag>/<arity>` -> display boxes with `\u{f8f0}<n>\u{f8f1}` markers in
    /// the argument slots, as produced by the kernel's own
    /// `BoxForm`TemplateBoxToDisplayBoxes`.
    template_expansions: HashMap<String, Node>,
    /// MD5 of `Characters.wl` and `Boxes.wl` at generation time,
    /// ":"-separated — the staleness handle `scripts/build.nu` checks.
    source_hash: String,
}

static TABLES: OnceLock<Tables> = OnceLock::new();

fn tables() -> &'static Tables {
    TABLES.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(TABLES_JSON).expect("invalid generated TypstForm tables");
        let object = value
            .as_object()
            .expect("TypstForm tables must be an object");
        let mut present: Vec<&str> = object.keys().map(String::as_str).collect();
        present.sort_unstable();
        let mut expected = TABLE_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(
            present, expected,
            "generated tables.json does not match TABLE_KEYS; regenerate it"
        );
        let assoc = |name: &str| -> HashMap<String, String> {
            object[name]
                .as_object()
                .unwrap_or_else(|| panic!("missing table {name}"))
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_owned()))
                .collect()
        };
        let set = |name: &str| -> HashSet<String> {
            object[name]
                .as_array()
                .unwrap_or_else(|| panic!("missing table {name}"))
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_owned())
                .collect()
        };
        let template_expansions = object["$typstTemplateExpansions"]
            .as_object()
            .expect("missing table $typstTemplateExpansions")
            .iter()
            .map(|(key, value)| {
                let node = decode_node(value)
                    .unwrap_or_else(|| panic!("undecodable template expansion for {key}"));
                (key.clone(), node)
            })
            .collect();
        Tables {
            char_map: assoc("$typstDefaultCharMap"),
            operator_map: assoc("$typstDefaultOperatorMap"),
            greek_words: assoc("$typstGreekWords"),
            matching_delimiter: assoc("$typstMatchingDelimiter"),
            semantic_delimiters: assoc("$typstSemanticDelimiterFunctions"),
            matrix_delimiters: assoc("$typstMatrixDelimiterSpec"),
            left_delimiters: assoc("$typstLeftDelimiters"),
            right_delimiters: assoc("$typstRightDelimiters"),
            infix_spaced: set("$typstInfixSpacedOps"),
            no_leading_space: set("$typstNoLeadingSpaceBefore"),
            no_trailing_space_after: set("$typstNoTrailingSpaceAfter")
                .into_iter()
                .filter_map(|entry| entry.chars().next())
                .collect(),
            template_expansions,
            source_hash: object["$typstTableSourceHash"]
                .as_str()
                .expect("missing table $typstTableSourceHash")
                .to_owned(),
        }
    })
}

/// Decode one generated box tree.
///
/// The encoding is deliberately compact: a JSON string is a `Node::Str`, a
/// JSON number is a `Node::Num`, `["$sym", "Name"]` is a `Node::Sym`, and any
/// other array is `[head, args...]`.
fn decode_node(value: &serde_json::Value) -> Option<Node> {
    match value {
        serde_json::Value::String(text) => Some(Node::Str(text.clone())),
        serde_json::Value::Number(number) => Some(Node::Num(number.to_string())),
        serde_json::Value::Array(items) => {
            let head = items.first()?.as_str()?;
            if head == "$sym" {
                return Some(Node::Sym(items.get(1)?.as_str()?.to_owned()));
            }
            let args = items[1..]
                .iter()
                .map(decode_node)
                .collect::<Option<Vec<_>>>()?;
            Some(Node::Box {
                head: Head::from_name(head),
                args,
            })
        }
        _ => None,
    }
}

pub fn typst_char(c: &str) -> Option<&'static str> {
    tables().char_map.get(c).map(String::as_str)
}

pub fn operator(name: &str) -> Option<&'static str> {
    tables().operator_map.get(name).map(String::as_str)
}

pub fn greek_word(name: &str) -> Option<&'static str> {
    tables().greek_words.get(name).map(String::as_str)
}

pub fn matching_delimiter(open: &str) -> Option<&'static str> {
    tables().matching_delimiter.get(open).map(String::as_str)
}

pub fn semantic_delimiter(left: &str) -> Option<&'static str> {
    tables().semantic_delimiters.get(left).map(String::as_str)
}

pub fn matrix_delimiter(left: &str) -> Option<&'static str> {
    tables().matrix_delimiters.get(left).map(String::as_str)
}

pub fn left_delimiter(left: &str) -> Option<&'static str> {
    tables().left_delimiters.get(left).map(String::as_str)
}

pub fn right_delimiter(right: &str) -> Option<&'static str> {
    tables().right_delimiters.get(right).map(String::as_str)
}

pub fn infix_spaced(token: &str) -> bool {
    tables().infix_spaced.contains(token)
}

pub fn no_leading_space(token: &str) -> bool {
    tables().no_leading_space.contains(token)
}

pub fn no_trailing_space_after(c: char) -> bool {
    tables().no_trailing_space_after.contains(&c)
}

/// Display boxes for `TemplateBox[_, tag]` with `arity` arguments, or `None`
/// when the kernel has no such template. The returned tree still holds
/// `\u{f8f0}<n>\u{f8f1}` markers where the arguments belong.
pub fn template_expansion(tag: &str, arity: usize) -> Option<&'static Node> {
    tables().template_expansions.get(&format!("{tag}/{arity}"))
}

/// MD5 of `Characters.wl` and `Boxes.wl` (":"-separated) at the time
/// `tables.json` was generated.
pub fn source_hash() -> &'static str {
    &tables().source_hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tables_are_loaded() {
        assert_eq!(typst_char("α"), Some("alpha "));
        assert_eq!(operator("cos"), Some("cos"));
        assert_eq!(matching_delimiter("("), Some(")"));
        assert!(infix_spaced("="));
        assert_eq!(matrix_delimiter("("), Some("Automatic"));
    }
}
