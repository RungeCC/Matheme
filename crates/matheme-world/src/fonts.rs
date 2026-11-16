//! Process-wide system font cache.
//!
//! Font discovery is expensive on Windows, so it is done once and the result
//! is shared by every compilation. `reload_fonts` swaps the snapshot after the
//! user installs new fonts.

use std::sync::{Arc, OnceLock, RwLock};

use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst_kit::fonts::{system, FontStore};

/// Immutable snapshot of the system font collection.
pub struct FontCache {
    pub book: LazyHash<FontBook>,
    pub fonts: Vec<Font>,
}

fn scan_fonts() -> FontCache {
    let mut store = FontStore::new();
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
