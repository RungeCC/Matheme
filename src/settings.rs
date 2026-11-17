//! Mapping between Wolfram associations and [`matheme_world::Settings`].
//!
//! The bridge accepts a patch association and returns the effective settings.
//! `Null` resets a field to its built-in default; omitting a key leaves it
//! untouched. Argument shapes are owned by the Wolfram package, so a wrong
//! type is a programming error handled by the standard call pipeline.

use std::path::PathBuf;

use matheme_wl::{Entry, Expr, Rule, Symbol};
use matheme_world::settings::{Patch, Settings, Update};
use matheme_world::FontCounts;

use crate::call;

const FIELDS: &[&str] = &[
    "Reset",
    "FontPaths",
    "SystemFonts",
    "EmbeddedFonts",
    "PackagePath",
    "PackageCache",
    "Proxy",
    "Downloads",
];

/// Read a patch from association entries.
pub fn parse(entries: &[Entry]) -> Patch {
    let mut patch = Patch::default();
    for entry in entries {
        let (key, value) = call::pair(entry);
        let name = call::key(key);
        assert!(FIELDS.contains(&name), "unknown settings key: {name}");
        match name {
            "Reset" => patch.reset = call::boolean(value),
            "FontPaths" => patch.font_paths = path_list(value),
            "SystemFonts" => patch.system_fonts = Update::Set(call::boolean(value)),
            "EmbeddedFonts" => patch.embedded_fonts = Update::Set(call::boolean(value)),
            "PackagePath" => patch.package_data = optional_path(value),
            "PackageCache" => patch.package_cache = optional_path(value),
            "Proxy" => patch.proxy = optional_string(value),
            "Downloads" => patch.downloads = Update::Set(call::boolean(value)),
            _ => unreachable!("field is in FIELDS"),
        }
    }
    patch
}

fn path_list(value: &Expr) -> Update<Vec<PathBuf>> {
    if value.is_symbol(&Symbol::NULL) {
        return Update::Reset;
    }
    let items = value
        .as_list()
        .expect("FontPaths must be a List of strings or Null");
    Update::Set(
        items
            .iter()
            .map(|item| PathBuf::from(call::string(item)))
            .collect(),
    )
}

fn optional_path(value: &Expr) -> Update<PathBuf> {
    if value.is_symbol(&Symbol::NULL) {
        return Update::Reset;
    }
    Update::Set(PathBuf::from(call::string(value)))
}

fn optional_string(value: &Expr) -> Update<String> {
    if value.is_symbol(&Symbol::NULL) {
        return Update::Reset;
    }
    let text = call::string(value);
    // An empty proxy means "no proxy", which is the same as resetting.
    if text.is_empty() {
        Update::Reset
    } else {
        Update::Set(text)
    }
}

/// Render the effective settings.
pub fn expr(settings: &Settings) -> Expr {
    Expr::association([
        Rule::new("FontPaths", path_list_expr(&settings.font_paths)),
        Rule::new("SystemFonts", settings.system_fonts),
        Rule::new("EmbeddedFonts", settings.embedded_fonts),
        Rule::new(
            "PackagePath",
            optional_path_expr(settings.package_data.as_deref()),
        ),
        Rule::new(
            "PackageCache",
            optional_path_expr(settings.package_cache.as_deref()),
        ),
        Rule::new("Proxy", optional_str_expr(settings.proxy.as_deref())),
        Rule::new("Downloads", settings.downloads),
        // Host-provided directories are reported for transparency, but are not
        // part of the patch: the bridge owns them.
        Rule::new("HostFontPaths", path_list_expr(&settings.host_font_paths)),
    ])
}

/// Render how many faces each font source contributed.
pub fn font_counts_expr(counts: &FontCounts) -> Expr {
    Expr::association([
        Rule::new("Custom", counts.custom as i64),
        Rule::new("Host", counts.host as i64),
        Rule::new("Embedded", counts.embedded as i64),
        Rule::new("System", counts.system as i64),
        Rule::new("Total", counts.total() as i64),
    ])
}

fn path_list_expr(paths: &[PathBuf]) -> Expr {
    Expr::list(paths.iter().map(|path| path.display().to_string()))
}

fn optional_path_expr(path: Option<&std::path::Path>) -> Expr {
    match path {
        Some(path) => Expr::string(path.display().to_string()),
        None => Expr::null(),
    }
}

fn optional_str_expr(value: Option<&str>) -> Expr {
    match value {
        Some(value) => Expr::string(value),
        None => Expr::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(rules: impl IntoIterator<Item = Rule>) -> Vec<Entry> {
        Expr::association(rules)
            .as_association()
            .expect("association")
    }

    #[test]
    fn booleans_and_paths_are_read() {
        let patch = parse(&entries([
            Rule::new("SystemFonts", false),
            Rule::new("PackagePath", "D:/typst/data"),
            Rule::new("Proxy", "http://127.0.0.1:2191"),
        ]));
        assert_eq!(patch.system_fonts, Update::Set(false));
        assert_eq!(
            patch.package_data,
            Update::Set(PathBuf::from("D:/typst/data"))
        );
        assert!(matches!(patch.proxy, Update::Set(ref url) if url.ends_with("2191")));
    }

    #[test]
    fn null_resets_fields() {
        let patch = parse(&entries([
            Rule::new("Proxy", Expr::null()),
            Rule::new("FontPaths", Expr::null()),
            Rule::new("Reset", true),
        ]));
        assert!(matches!(patch.proxy, Update::Reset));
        assert!(matches!(patch.font_paths, Update::Reset));
        assert!(patch.reset);
    }

    #[test]
    #[should_panic(expected = "unknown settings key")]
    fn unknown_keys_are_not_silently_ignored() {
        parse(&entries([Rule::new("Mystery", 1_i64)]));
    }

    #[test]
    #[should_panic(expected = "expected True or False")]
    fn wrong_types_are_not_guessed() {
        parse(&entries([Rule::new("SystemFonts", "yes")]));
    }
}
