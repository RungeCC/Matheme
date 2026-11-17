//! Mapping between Wolfram associations and [`matheme_world::Settings`].
//!
//! The bridge accepts a patch association, returns the effective settings, and
//! keeps all knowledge of WL value shapes in this module. `Null` resets a field
//! to its built-in default; omitting a key leaves it untouched.

use std::path::PathBuf;

use matheme_wl::{Expr, Rule, Symbol};
use matheme_world::settings::{Patch, Settings, Update};
use matheme_world::FontCounts;

/// A parsed patch plus any non-fatal complaints about individual keys.
pub struct Parsed {
    pub patch: Patch,
    pub warnings: Vec<String>,
}

/// Read a patch association.
pub fn parse(expr: &Expr) -> Result<Parsed, String> {
    let entries = expr
        .as_association()
        .ok_or_else(|| "settings must be an Association".to_owned())?;

    let mut parsed = Parsed {
        patch: Patch::default(),
        warnings: Vec::new(),
    };

    for entry in entries {
        let (key, value) = match entry.pair() {
            Ok(pair) => pair,
            Err(reason) => {
                parsed.warnings.push(reason);
                continue;
            }
        };
        let Some(name) = key.as_str() else {
            parsed.warn(format!(
                "settings key {} is not a String; entry skipped",
                key.raw()
            ));
            continue;
        };
        match name {
            "Reset" => match value.as_bool() {
                Some(reset) => parsed.patch.reset = reset,
                None => parsed.warn("Reset must be True or False"),
            },
            "FontPaths" => match path_list(value) {
                Ok(update) => parsed.patch.font_paths = update,
                Err(reason) => parsed.warn(format!("FontPaths {reason}")),
            },
            "SystemFonts" => match value.as_bool() {
                Some(enabled) => parsed.patch.system_fonts = Update::Set(enabled),
                None => parsed.warn("SystemFonts must be True or False"),
            },
            "EmbeddedFonts" => match value.as_bool() {
                Some(enabled) => parsed.patch.embedded_fonts = Update::Set(enabled),
                None => parsed.warn("EmbeddedFonts must be True or False"),
            },
            "PackagePath" => match optional_path(value) {
                Ok(update) => parsed.patch.package_data = update,
                Err(reason) => parsed.warn(format!("PackagePath {reason}")),
            },
            "PackageCache" => match optional_path(value) {
                Ok(update) => parsed.patch.package_cache = update,
                Err(reason) => parsed.warn(format!("PackageCache {reason}")),
            },
            "Proxy" => match optional_string(value) {
                Ok(update) => parsed.patch.proxy = update,
                Err(reason) => parsed.warn(format!("Proxy {reason}")),
            },
            "Downloads" => match value.as_bool() {
                Some(enabled) => parsed.patch.downloads = Update::Set(enabled),
                None => parsed.warn("Downloads must be True or False"),
            },
            other => parsed.warn(format!("unknown settings key: {other}")),
        }
    }

    Ok(parsed)
}

impl Parsed {
    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

fn path_list(value: &Expr) -> Result<Update<Vec<PathBuf>>, String> {
    if value.is_symbol(&Symbol::NULL) {
        return Ok(Update::Reset);
    }
    let items = value
        .as_list()
        .ok_or_else(|| "must be a List of strings or Null".to_owned())?;
    let mut paths = Vec::with_capacity(items.len());
    for item in &items {
        let path = item
            .as_str()
            .ok_or_else(|| "must contain only strings".to_owned())?;
        paths.push(PathBuf::from(path));
    }
    Ok(Update::Set(paths))
}

fn optional_path(value: &Expr) -> Result<Update<PathBuf>, String> {
    if value.is_symbol(&Symbol::NULL) {
        return Ok(Update::Reset);
    }
    value
        .as_str()
        .map(|path| Update::Set(PathBuf::from(path)))
        .ok_or_else(|| "must be a String or Null".to_owned())
}

fn optional_string(value: &Expr) -> Result<Update<String>, String> {
    if value.is_symbol(&Symbol::NULL) {
        return Ok(Update::Reset);
    }
    match value.as_str() {
        // An empty proxy means "no proxy", which is the same as resetting.
        Some("") => Ok(Update::Reset),
        Some(text) => Ok(Update::Set(text.to_owned())),
        None => Err("must be a String or Null".to_owned()),
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

    fn parse_settings(entries: impl IntoIterator<Item = Rule>) -> Parsed {
        parse(&Expr::association(entries)).expect("association")
    }

    #[test]
    fn booleans_and_paths_are_read() {
        let parsed = parse_settings([
            Rule::new("SystemFonts", false),
            Rule::new("PackagePath", "D:/typst/data"),
            Rule::new("Proxy", "http://127.0.0.1:2191"),
        ]);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.patch.system_fonts, Update::Set(false));
        assert_eq!(
            parsed.patch.package_data,
            Update::Set(PathBuf::from("D:/typst/data"))
        );
        assert!(matches!(parsed.patch.proxy, Update::Set(ref url) if url.ends_with("2191")));
    }

    #[test]
    fn null_resets_and_unknown_keys_warn() {
        let parsed = parse_settings([
            Rule::new("Proxy", Expr::null()),
            Rule::new("Mystery", 1_i64),
            Rule::new("SystemFonts", "yes"),
        ]);
        assert!(matches!(parsed.patch.proxy, Update::Reset));
        assert_eq!(parsed.warnings.len(), 2);
    }

    #[test]
    fn font_paths_require_strings() {
        let parsed = parse_settings([Rule::new("FontPaths", Expr::list([1_i64]))]);
        assert_eq!(parsed.warnings.len(), 1);
        assert!(matches!(parsed.patch.font_paths, Update::Keep));
    }
}
