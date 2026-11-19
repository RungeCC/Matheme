//! Process-wide, mutable configuration.
//!
//! Everything the compiler needs to know about its environment lives here:
//! where fonts come from, which package directories to use, and how network
//! access is configured. The caches read this state when they are (re)built, so
//! a single [`apply`] call reconfigures a running process.
//!
//! Configuration is deliberately *not* read from environment variables. They
//! cannot be observed reliably from a host language, and a stale value in the
//! process environment is impossible to tell apart from a deliberate one.
//! Callers patch [`Settings`] instead and can read the effective state back.

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::cache::{read_unpoisoned, write_unpoisoned};
use crate::fonts;
use crate::packages;

/// How a single field changes when a [`Patch`] is applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Update<T> {
    /// Keep the current value.
    #[default]
    Keep,
    /// Restore the built-in default (for optional paths: unset).
    Reset,
    /// Replace the value.
    Set(T),
}

impl<T: Clone + PartialEq> Update<T> {
    /// Apply to a plain field, using `default` for [`Update::Reset`].
    fn apply(&self, slot: &mut T, default: T) -> bool {
        match self {
            Update::Keep => false,
            Update::Reset => replace(slot, default),
            Update::Set(value) => replace(slot, value.clone()),
        }
    }

    /// Apply to an optional field; [`Update::Reset`] clears it.
    fn apply_optional(&self, slot: &mut Option<T>) -> bool {
        match self {
            Update::Keep => false,
            Update::Reset => replace(slot, None),
            Update::Set(value) => replace(slot, Some(value.clone())),
        }
    }
}

fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

/// The effective configuration of the compiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Extra font directories searched first, highest priority.
    pub font_paths: Vec<PathBuf>,
    /// Font directories contributed by the host application (e.g. fonts the
    /// application ships but does not register with the OS). Not part of the
    /// user-facing patch: the bridge owns this field.
    pub host_font_paths: Vec<PathBuf>,
    /// Whether OS fonts are scanned.
    pub system_fonts: bool,
    /// Whether Typst's embedded fonts (Libertinus, New Computer Modern,
    /// DejaVu Sans Mono) are used. They guarantee a sensible default rendering
    /// even on a machine with no usable system fonts.
    pub embedded_fonts: bool,
    /// Package data directory. `None` uses Typst's platform default.
    pub package_data: Option<PathBuf>,
    /// Package cache directory. `None` uses Typst's platform default.
    pub package_cache: Option<PathBuf>,
    /// Proxy URL for package downloads. `None` connects directly and ignores
    /// the process environment.
    pub proxy: Option<String>,
    /// Whether downloads are allowed without a per-call opt-in.
    pub downloads: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_paths: Vec::new(),
            host_font_paths: Vec::new(),
            system_fonts: true,
            embedded_fonts: true,
            package_data: None,
            package_cache: None,
            proxy: None,
            downloads: false,
        }
    }
}

/// A partial update of [`Settings`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    pub font_paths: Update<Vec<PathBuf>>,
    pub system_fonts: Update<bool>,
    pub embedded_fonts: Update<bool>,
    pub package_data: Update<PathBuf>,
    pub package_cache: Update<PathBuf>,
    pub proxy: Update<String>,
    pub downloads: Update<bool>,
    /// Restore every field to its default before applying the rest.
    pub reset: bool,
}

/// Which caches a patch invalidated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changes {
    pub fonts: bool,
    pub packages: bool,
    pub downloads: bool,
}

impl Patch {
    /// Merge the patch into `settings`, reporting which groups changed.
    pub fn apply_to(&self, settings: &mut Settings) -> Changes {
        let defaults = Settings::default();
        let before = settings.clone();
        if self.reset {
            // `host_font_paths` belongs to the host application, not to the
            // user-facing configuration, so a reset must not drop it.
            *settings = Settings {
                host_font_paths: settings.host_font_paths.clone(),
                ..defaults.clone()
            };
        }
        self.font_paths
            .apply(&mut settings.font_paths, defaults.font_paths);
        self.system_fonts
            .apply(&mut settings.system_fonts, defaults.system_fonts);
        self.embedded_fonts
            .apply(&mut settings.embedded_fonts, defaults.embedded_fonts);
        self.package_data.apply_optional(&mut settings.package_data);
        self.package_cache
            .apply_optional(&mut settings.package_cache);
        self.proxy.apply_optional(&mut settings.proxy);
        self.downloads
            .apply(&mut settings.downloads, defaults.downloads);

        Changes {
            fonts: settings.font_paths != before.font_paths
                || settings.system_fonts != before.system_fonts
                || settings.embedded_fonts != before.embedded_fonts,
            packages: settings.package_data != before.package_data
                || settings.package_cache != before.package_cache
                || settings.proxy != before.proxy,
            downloads: settings.downloads != before.downloads,
        }
    }
}

/// Result of applying a patch.
#[derive(Debug, Clone)]
pub struct Applied {
    pub settings: Settings,
    pub changes: Changes,
}

static SETTINGS: OnceLock<RwLock<Settings>> = OnceLock::new();

fn slot() -> &'static RwLock<Settings> {
    SETTINGS.get_or_init(|| RwLock::new(Settings::default()))
}

/// A snapshot of the current settings.
pub fn settings() -> Settings {
    read_unpoisoned(slot()).clone()
}

/// Apply a patch and invalidate the caches it affects.
pub fn apply(patch: &Patch) -> Applied {
    let (settings, changes) = {
        let mut guard = write_unpoisoned(slot());
        let changes = patch.apply_to(&mut guard);
        (guard.clone(), changes)
    };

    if changes.downloads {
        packages::set_configured_downloads(settings.downloads);
    }

    if changes.fonts {
        fonts::refresh();
    }

    if changes.packages {
        packages::rebuild();
    }

    Applied { settings, changes }
}

/// Set the host-provided font directories. Returns whether they changed.
///
/// This is the one field the host application owns; it is separate from
/// `font_paths` so a user patch cannot accidentally drop the host's fonts.
pub fn set_host_font_paths(paths: Vec<PathBuf>) -> bool {
    let changed = {
        let mut guard = write_unpoisoned(slot());
        replace(&mut guard.host_font_paths, paths)
    };
    if changed {
        fonts::refresh();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> Patch {
        Patch::default()
    }

    #[test]
    fn patch_updates_only_the_fields_it_sets() {
        let mut settings = Settings::default();
        let mut patch = patch();
        patch.system_fonts = Update::Set(false);
        patch.package_data = Update::Set(PathBuf::from("D:/typst/data"));

        let changes = patch.apply_to(&mut settings);
        assert!(changes.fonts);
        assert!(changes.packages);
        assert!(!changes.downloads);
        assert!(!settings.system_fonts);
        assert_eq!(settings.package_data, Some(PathBuf::from("D:/typst/data")));
        // Untouched fields keep their defaults.
        assert!(settings.embedded_fonts);
    }

    #[test]
    fn reset_restores_defaults_before_other_updates() {
        let mut settings = Settings {
            system_fonts: false,
            proxy: Some("http://127.0.0.1:1".into()),
            host_font_paths: vec![PathBuf::from("C:/wolfram/fonts")],
            ..Settings::default()
        };
        let mut patch = patch();
        patch.reset = true;
        patch.embedded_fonts = Update::Set(false);

        let changes = patch.apply_to(&mut settings);
        assert!(changes.fonts);
        assert!(settings.system_fonts);
        assert!(settings.proxy.is_none());
        assert!(!settings.embedded_fonts);
        // Host-provided directories survive a reset.
        assert_eq!(
            settings.host_font_paths,
            vec![PathBuf::from("C:/wolfram/fonts")]
        );
    }

    #[test]
    fn null_resets_optional_paths_to_the_platform_default() {
        let mut settings = Settings {
            package_cache: Some(PathBuf::from("D:/cache")),
            ..Settings::default()
        };
        let mut patch = patch();
        patch.package_cache = Update::Reset;
        patch.apply_to(&mut settings);
        assert!(settings.package_cache.is_none());
    }
}
