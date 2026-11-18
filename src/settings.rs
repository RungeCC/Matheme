//! Mapping between Wolfram associations and [`matheme_world::Settings`].
//!
//! The bridge accepts a patch association and returns the effective settings.
//! `Null` resets a field to its built-in default; omitting a key leaves it
//! untouched.

use std::path::PathBuf;

use matheme_wl::{assoc, Entry, Expr, Symbol};
use matheme_world::settings::{Patch, Settings, Update};
use matheme_world::FontCounts;

use crate::call::{self, Failure};

const FIELDS: &[&str] = &[
    "FontPaths",
    "SystemFonts",
    "EmbeddedFonts",
    "PackagePath",
    "PackageCache",
    "Proxy",
    "Downloads",
];

/// Read a patch from association entries.
pub fn parse(entries: &[Entry]) -> Result<Patch, Failure> {
    let mut patch = Patch::default();
    for entry in entries {
        let (key, value) = call::pair(entry)?;
        let name = call::key(key)?;
        if !FIELDS.contains(&name) {
            return Err(call::error(format!("unknown settings key: {name}")));
        }
        match name {
            "FontPaths" => patch.font_paths = path_list(value)?,
            "SystemFonts" => patch.system_fonts = Update::Set(call::boolean(value)?),
            "EmbeddedFonts" => patch.embedded_fonts = Update::Set(call::boolean(value)?),
            "PackagePath" => patch.package_data = optional_path(value)?,
            "PackageCache" => patch.package_cache = optional_path(value)?,
            "Proxy" => patch.proxy = optional_string(value)?,
            "Downloads" => patch.downloads = Update::Set(call::boolean(value)?),
            _ => unreachable!("field is in FIELDS"),
        }
    }
    Ok(patch)
}

/// A patch that restores every user-facing field to its default.
pub fn reset_patch() -> Patch {
    Patch {
        reset: true,
        ..Patch::default()
    }
}

fn path_list(value: &Expr) -> Result<Update<Vec<PathBuf>>, Failure> {
    if value.is_symbol(&Symbol::NULL) {
        return Ok(Update::Reset);
    }
    let items = value
        .as_list()
        .ok_or_else(|| call::error("FontPaths must be a List of strings or Null"))?;
    items
        .iter()
        .map(|item| call::string(item).map(PathBuf::from))
        .collect::<Result<Vec<_>, _>>()
        .map(Update::Set)
}

fn optional_path(value: &Expr) -> Result<Update<PathBuf>, Failure> {
    if value.is_symbol(&Symbol::NULL) {
        return Ok(Update::Reset);
    }
    Ok(Update::Set(PathBuf::from(call::string(value)?)))
}

fn optional_string(value: &Expr) -> Result<Update<String>, Failure> {
    if value.is_symbol(&Symbol::NULL) {
        return Ok(Update::Reset);
    }
    let text = call::string(value)?;
    // An empty proxy means "no proxy", which is the same as resetting.
    if text.is_empty() {
        Ok(Update::Reset)
    } else {
        Ok(Update::Set(text))
    }
}

/// Render the effective settings.
pub fn expr(settings: &Settings) -> Expr {
    assoc! {
        "FontPaths" -> path_list_expr(&settings.font_paths),
        "SystemFonts" -> settings.system_fonts,
        "EmbeddedFonts" -> settings.embedded_fonts,
        "PackagePath" -> settings.package_data.as_deref().map(path_string),
        "PackageCache" -> settings.package_cache.as_deref().map(path_string),
        "Proxy" -> settings.proxy.as_deref(),
        "Downloads" -> settings.downloads,
        // Host-provided directories are reported for transparency, but are not
        // part of the patch: the bridge owns them.
        "HostFontPaths" -> path_list_expr(&settings.host_font_paths),
    }
}

/// Render how many faces each font source contributed.
pub fn font_counts_expr(counts: &FontCounts) -> Expr {
    assoc! {
        "Custom" -> counts.custom as i64,
        "Host" -> counts.host as i64,
        "Embedded" -> counts.embedded as i64,
        "System" -> counts.system as i64,
        "Total" -> counts.total() as i64,
    }
}

fn path_string(path: &std::path::Path) -> String {
    path.display().to_string()
}

fn path_list_expr(paths: &[PathBuf]) -> Expr {
    Expr::list(paths.iter().map(|path| path_string(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::entries;
    use matheme_wl::Rule;

    #[test]
    fn booleans_and_paths_are_read() {
        let patch = parse(&entries([
            Rule::new("SystemFonts", false),
            Rule::new("PackagePath", "D:/typst/data"),
            Rule::new("Proxy", "http://127.0.0.1:2191"),
        ]))
        .expect("valid settings patch");
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
        ]))
        .expect("valid null reset");
        assert!(matches!(patch.proxy, Update::Reset));
        assert!(matches!(patch.font_paths, Update::Reset));
        assert!(!patch.reset);
    }

    #[test]
    fn reset_is_a_separate_patch() {
        assert!(reset_patch().reset);
    }

    #[test]
    fn unknown_keys_return_a_failure() {
        let err = parse(&entries([Rule::new("Mystery", 1_i64)])).expect_err("unknown key");
        assert_eq!(err.tag, "Call");
        assert!(err.message.contains("unknown settings key"), "{}", err.message);
    }

    #[test]
    fn wrong_types_return_a_failure() {
        let err =
            parse(&entries([Rule::new("SystemFonts", "yes")])).expect_err("wrong type");
        assert_eq!(err.tag, "Call");
        assert!(
            err.message.contains("expected True or False"),
            "{}",
            err.message
        );
    }
}
