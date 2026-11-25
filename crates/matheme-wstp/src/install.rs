//! Wolfram installation path conventions shared by discovery and the bridge.

use std::path::{Path, PathBuf};

use matheme_wl::Symbol;

/// ``System`$InstallationDirectory``.
pub const INSTALLATION_DIRECTORY: Symbol = Symbol::from_static("System`$InstallationDirectory");

/// Standard subdirectory under a Wolfram installation containing bundled
/// TrueType fonts.
pub fn wolfram_truetype_font_dir(install_dir: &Path) -> PathBuf {
    install_dir
        .join("SystemFiles")
        .join("Fonts")
        .join("TrueType")
}
