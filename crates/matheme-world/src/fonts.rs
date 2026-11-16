//! Process-wide system font cache.
//!
//! Font discovery is expensive on Windows, so it is done once and the result
//! is shared by every compilation. `reload_fonts` swaps the snapshot after the
//! user installs new fonts.
//!
//! Besides the operating system's fonts we also add the TrueType fonts that
//! Wolfram bundles under `$InstallationDirectory/SystemFiles/Fonts/TrueType`
//! (Mathematica Sans, Mathematica Mono, Alegreya, Source Sans, …). They are
//! not registered as system fonts, so `fontdb` cannot find them on its own.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst_kit::fonts::{system, FontStore};

/// Immutable snapshot of the system font collection.
pub struct FontCache {
    pub book: LazyHash<FontBook>,
    pub fonts: Vec<Font>,
}

fn push_unique(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if dir.is_dir() && !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

fn installation_font_dir(root: &Path) -> PathBuf {
    root.join("SystemFiles").join("Fonts").join("TrueType")
}

/// Compare versioned installation directory names numerically, so `15.0`
/// sorts after `9.0` (a plain string sort would get this wrong).
fn version_key(name: &OsStr) -> Vec<u64> {
    name.to_string_lossy()
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn scan_installation_root(dirs: &mut Vec<PathBuf>, root: &Path) {
    // Unversioned layout, e.g. the Contents directory of the macOS app bundle.
    push_unique(dirs, installation_font_dir(root));

    // Versioned layout, e.g. C:\Program Files\Wolfram Research\Wolfram\15.0.
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut versions: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    versions.sort_by(|a, b| {
        let empty = OsStr::new("");
        version_key(b.file_name().unwrap_or(empty)).cmp(&version_key(a.file_name().unwrap_or(empty)))
    });
    for version in versions {
        let candidate = installation_font_dir(&version);
        if candidate.is_dir() {
            // One (the newest) installation per root is plenty.
            push_unique(dirs, candidate);
            break;
        }
    }
}

fn installation_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        let Some(base) = std::env::var_os(key) else {
            continue;
        };
        let vendor = PathBuf::from(base).join("Wolfram Research");
        roots.push(vendor.join("Wolfram"));
        roots.push(vendor.join("Mathematica"));
    }
    if cfg!(target_os = "macos") {
        roots.push(PathBuf::from("/Applications/Wolfram Mathematica.app/Contents"));
        roots.push(PathBuf::from("/Applications/Mathematica.app/Contents"));
    }
    if cfg!(target_os = "linux") {
        for base in ["/usr/local/Wolfram", "/opt/Wolfram"] {
            roots.push(PathBuf::from(base).join("Wolfram"));
            roots.push(PathBuf::from(base).join("Mathematica"));
        }
    }
    roots
}

/// Directories holding Wolfram's bundled TrueType fonts.
///
/// Resolution order:
/// 1. `MATHEME_WOLFRAM_FONTS` — an explicit `env::split_paths` list. Set it to
///    an empty value to disable bundled fonts entirely.
/// 2. `MATHEME_WOLFRAM_DIR` / `WOLFRAM_INSTALLATION_DIRECTORY` — an
///    installation root; `<root>/SystemFiles/Fonts/TrueType` is used. The WL
///    package sets the first one from `$InstallationDirectory` on load.
/// 3. Well-known installation roots, picking the newest versioned directory.
pub fn wolfram_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(paths) = std::env::var_os("MATHEME_WOLFRAM_FONTS") {
        for path in std::env::split_paths(&paths) {
            if !path.as_os_str().is_empty() {
                push_unique(&mut dirs, path);
            }
        }
        // An explicit list is authoritative; don't add discovered directories.
        return dirs;
    }

    for key in ["MATHEME_WOLFRAM_DIR", "WOLFRAM_INSTALLATION_DIRECTORY"] {
        if let Some(root) = std::env::var_os(key) {
            if !root.is_empty() {
                push_unique(&mut dirs, installation_font_dir(Path::new(&root)));
            }
        }
    }
    for root in installation_roots() {
        scan_installation_root(&mut dirs, &root);
    }
    dirs
}

/// Number of font files in the bundled directories (cheap status reporting;
/// the real count of faces loaded into the book is `warmup_fonts()`).
pub fn wolfram_font_file_count() -> usize {
    wolfram_font_dirs()
        .iter()
        .map(|dir| {
            std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| {
                            entry
                                .path()
                                .extension()
                                .and_then(OsStr::to_str)
                                .is_some_and(|ext| {
                                    matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc")
                                })
                        })
                        .count()
                })
                .unwrap_or(0)
        })
        .sum()
}

fn scan_fonts() -> FontCache {
    let mut store = FontStore::new();

    // Wolfram's fonts go first: families such as "Mathematica Sans" exist
    // nowhere else, and when two fonts tie in FontBook the earlier one wins.
    for dir in wolfram_font_dirs() {
        store.extend(typst_kit::fonts::scan(&dir));
    }
    store.extend(system());

    // FontStore keeps book indices and font indices aligned.
    let fonts: Vec<Font> = (0..).map_while(|i| store.font(i)).collect();

    FontCache {
        book: store.book().clone(),
        fonts,
    }
}

static FONTS: OnceLock<RwLock<Arc<FontCache>>> = OnceLock::new();

fn fonts_slot() -> &'static RwLock<Arc<FontCache>> {
    FONTS.get_or_init(|| RwLock::new(Arc::new(scan_fonts())))
}

/// Get the current font snapshot without holding the lock across a compilation.
/// This also triggers the lazy font scan on first use.
pub fn font_cache() -> Arc<FontCache> {
    fonts_slot()
        .read()
        .expect("font cache lock poisoned")
        .clone()
}

/// Eagerly initialize the font cache and return the number of fonts found.
pub fn warmup_fonts() -> usize {
    font_cache().fonts.len()
}

/// Rescan system fonts and return the new font count.
pub fn reload_fonts() -> usize {
    let next = Arc::new(scan_fonts());
    let count = next.fonts.len();
    *fonts_slot()
        .write()
        .expect("font cache lock poisoned") = next;
    count
}
