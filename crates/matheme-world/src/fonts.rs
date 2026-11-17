//! Process-wide system font cache.
//!
//! Font discovery is expensive on Windows, so it is done once and the result
//! is shared by every compilation. `reload_fonts` swaps the snapshot after the
//! user installs new fonts.
//!
//! Callers may inject additional font directories (e.g. fonts shipped by an
//! application that are not registered as OS fonts) through
//! [`set_extra_font_dirs`]. This crate deliberately knows nothing about
//! Wolfram; discovery of Wolfram's installation is the bridge's job.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst_kit::fonts::{system, FontStore};

/// Immutable snapshot of the system font collection.
pub struct FontCache {
    pub book: LazyHash<FontBook>,
    pub fonts: Vec<Font>,
}

static EXTRA_DIRS: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();

fn extra_dirs_slot() -> &'static RwLock<Vec<PathBuf>> {
    EXTRA_DIRS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Replace the additional font directories scanned before the system fonts.
/// FontBook resolves family-name ties in insertion order, so injected fonts
/// take precedence when a family is present in both places.
pub fn set_extra_font_dirs(dirs: impl IntoIterator<Item = PathBuf>) {
    *extra_dirs_slot()
        .write()
        .expect("extra font dirs lock poisoned") = dirs.into_iter().collect();
}

/// Current snapshot of the injected font directories.
pub fn extra_font_dirs() -> Vec<PathBuf> {
    extra_dirs_slot()
        .read()
        .expect("extra font dirs lock poisoned")
        .clone()
}

fn scan_fonts() -> FontCache {
    let mut store = FontStore::new();

    // Injected fonts go first: families such as "Mathematica Sans" exist
    // nowhere else, and when two fonts tie in FontBook the earlier one wins.
    for dir in extra_font_dirs() {
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
