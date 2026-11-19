//! Mapping between Wolfram associations and [`matheme_world::Settings`].
//!
//! The bridge accepts a patch association and returns the effective settings.
//! `Null` resets a field to its built-in default; omitting a key leaves it
//! untouched.

use std::path::PathBuf;

use matheme_wl::{assoc, rule, Expr, Symbol};
use matheme_world::settings::{Patch, Settings as WorldSettings, Update};
use matheme_world::{self as world, FontCounts};

use crate::args::{FromExpr, Resettable, SetOnly};
use crate::call::{Answer, Outcome};
use crate::wl_args;

/// Proxy field value: `Null` or empty string resets the proxy, otherwise sets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyUpdate(pub Update<String>);

impl Default for ProxyUpdate {
    fn default() -> Self {
        Self(Update::Keep)
    }
}

impl FromExpr for ProxyUpdate {
    fn from_expr(expr: &Expr) -> Result<Self, String> {
        if expr.is_symbol(&Symbol::NULL) {
            return Ok(Self(Update::Reset));
        }
        let text = String::from_expr(expr)?;
        if text.is_empty() {
            Ok(Self(Update::Reset))
        } else {
            world::packages::validate_proxy(&text)?;
            Ok(Self(Update::Set(text)))
        }
    }
}

wl_args! {
    #[action = "setSettings"]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SettingsPatchArgs {
        "FontPaths" => pub font_paths: Resettable<Vec<PathBuf>> = Resettable::default(),
        "SystemFonts" => pub system_fonts: SetOnly<bool> = SetOnly::default(),
        "EmbeddedFonts" => pub embedded_fonts: SetOnly<bool> = SetOnly::default(),
        "PackagePath" => pub package_data: Resettable<PathBuf> = Resettable::default(),
        "PackageCache" => pub package_cache: Resettable<PathBuf> = Resettable::default(),
        "Proxy" => pub proxy: ProxyUpdate = ProxyUpdate::default(),
        "Downloads" => pub downloads: SetOnly<bool> = SetOnly::default(),
    }
}

impl SettingsPatchArgs {
    pub fn into_patch(self) -> Patch {
        Patch {
            font_paths: self.font_paths.0,
            system_fonts: self.system_fonts.0,
            embedded_fonts: self.embedded_fonts.0,
            package_data: self.package_data.0,
            package_cache: self.package_cache.0,
            proxy: self.proxy.0,
            downloads: self.downloads.0,
            reset: false,
        }
    }
}

/// A patch that restores every user-facing field to its default.
pub fn reset_patch() -> Patch {
    Patch {
        reset: true,
        ..Patch::default()
    }
}

/// Render the effective settings.
pub fn expr(settings: &WorldSettings) -> Expr {
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

// ---------------------------------------------------------------------------
// Settings / SetSettings / ResetSettings handlers
// ---------------------------------------------------------------------------

pub fn run_get() -> Answer {
    Outcome::new([rule!("Settings" -> expr(&world::settings()))]).into()
}

pub fn run_set(args: SettingsPatchArgs) -> Answer {
    applied_outcome(world::apply(&args.into_patch())).into()
}

pub fn run_reset() -> Answer {
    applied_outcome(world::apply(&reset_patch())).into()
}

fn applied_outcome(applied: world::Applied) -> Outcome {
    Outcome::new([
        rule!(
            "Changed" -> assoc! {
                "Fonts" -> applied.changes.fonts,
                "Packages" -> applied.changes.packages,
                "Downloads" -> applied.changes.downloads,
            }
        ),
        rule!("Settings" -> expr(&applied.settings)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::FromArgs;
    use crate::call::with_entries;
    use matheme_wl::Rule;

    fn parse(entries: &[matheme_wl::Entry<'_>]) -> Result<Patch, crate::call::Failure> {
        SettingsPatchArgs::parse_args(entries).map(SettingsPatchArgs::into_patch)
    }

    #[test]
    fn booleans_and_paths_are_read() {
        let patch = with_entries(
            [
                Rule::new("SystemFonts", false),
                Rule::new("PackagePath", "D:/typst/data"),
                Rule::new("Proxy", "http://127.0.0.1:2191"),
            ],
            parse,
        )
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
        let patch = with_entries(
            [
                Rule::new("Proxy", Expr::null()),
                Rule::new("FontPaths", Expr::null()),
            ],
            parse,
        )
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
        let err = with_entries([Rule::new("Mystery", 1_i64)], parse).expect_err("unknown key");
        assert_eq!(err.tag, "Call");
        assert!(
            err.message.contains("unknown setSettings key: Mystery"),
            "{}",
            err.message
        );
    }

    #[test]
    fn wrong_types_return_a_failure_with_field_context() {
        let err = with_entries([Rule::new("SystemFonts", "yes")], parse).expect_err("wrong type");
        assert_eq!(err.tag, "Call");
        assert!(
            err.message.contains("setSettings.SystemFonts: expected True or False"),
            "{}",
            err.message
        );
    }

    #[test]
    fn malformed_proxy_returns_a_failure() {
        let err = with_entries([Rule::new("Proxy", "://not-a-valid-proxy")], parse)
            .expect_err("malformed proxy");
        assert_eq!(err.tag, "Call");
        assert!(
            err.message.contains("setSettings.Proxy: invalid proxy"),
            "{}",
            err.message
        );
    }
}
