//! Process-wide font cache.
//!
//! Discovery is expensive on Windows, so the font collection is built once and
//! shared by every compilation. It is rebuilt when the configuration changes
//! ([`refresh`]) or when the user explicitly asks for a rescan
//! ([`reload_fonts`]).
//!
//! Sources are added in priority order — earlier fonts win when families tie:
//!
//! 1. `font_paths`: user-provided directories (highest priority);
//! 2. `host_font_paths`: fonts shipped by the host application (e.g. Wolfram's
//!    bundled fonts, which are not registered with the OS);
//! 3. Typst's embedded fonts, so a machine with no usable system fonts still
//!    renders sensibly;
//! 4. OS fonts.

use std::sync::{Arc, OnceLock, RwLock};

use typst::text::{Font, FontBook, FontInfo};
use typst::utils::LazyHash;
use typst_kit::fonts::{self, FontSource, FontStore};

use crate::settings;

/// Immutable snapshot of the font collection.
pub struct FontCache {
    pub book: LazyHash<FontBook>,
    pub fonts: Vec<Font>,
    pub counts: FontCounts,
}

/// How many faces each source contributed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FontCounts {
    pub custom: usize,
    pub host: usize,
    pub embedded: usize,
    pub system: usize,
}

impl FontCounts {
    pub fn total(&self) -> usize {
        self.custom + self.host + self.embedded + self.system
    }
}

/// Add all `entries` to the store and report how many were added.
fn extend<T: FontSource>(
    store: &mut FontStore,
    entries: impl IntoIterator<Item = (T, FontInfo)>,
) -> usize {
    let entries: Vec<_> = entries.into_iter().collect();
    let count = entries.len();
    store.extend(entries);
    count
}

fn scan() -> FontCache {
    let settings = settings::settings();
    let mut store = FontStore::new();
    let mut counts = FontCounts::default();

    for dir in &settings.font_paths {
        counts.custom += extend(&mut store, fonts::scan(dir));
    }
    for dir in &settings.host_font_paths {
        counts.host += extend(&mut store, fonts::scan(dir));
    }
    if settings.embedded_fonts {
        counts.embedded += extend(&mut store, fonts::embedded());
    }
    if settings.system_fonts {
        counts.system += extend(&mut store, fonts::system());
    }

    // FontStore keeps book indices and font indices aligned.
    let fonts: Vec<Font> = (0..).map_while(|i| store.font(i)).collect();

    FontCache {
        book: store.book().clone(),
        fonts,
        counts,
    }
}

static FONTS: OnceLock<RwLock<Arc<FontCache>>> = OnceLock::new();

fn fonts_slot() -> &'static RwLock<Arc<FontCache>> {
    FONTS.get_or_init(|| RwLock::new(Arc::new(scan())))
}

/// Get the current font snapshot without holding the lock across a compilation.
/// This also triggers the lazy font scan on first use.
pub fn font_cache() -> Arc<FontCache> {
    fonts_slot()
        .read()
        .expect("font cache lock poisoned")
        .clone()
}

/// Counts of the current snapshot, triggering the scan if needed.
pub fn font_counts() -> FontCounts {
    font_cache().counts
}

/// Counts of the current snapshot, without forcing a scan.
pub fn cached_counts() -> Option<FontCounts> {
    FONTS
        .get()
        .map(|slot| slot.read().expect("font cache lock poisoned").counts)
}

/// Rescan with the current settings. Returns whether the cache was live.
pub(crate) fn refresh() -> bool {
    let Some(slot) = FONTS.get() else {
        // Not initialized yet: the next `font_cache()` picks up the new
        // settings on its own.
        return false;
    };
    let next = Arc::new(scan());
    *slot.write().expect("font cache lock poisoned") = next;
    true
}

/// Eagerly initialize the font cache and return the number of faces found.
pub fn warmup_fonts() -> usize {
    font_cache().fonts.len()
}

/// Rescan fonts (e.g. after installing a font) and return the new face count.
pub fn reload_fonts() -> usize {
    let next = Arc::new(scan());
    let count = next.fonts.len();
    *fonts_slot().write().expect("font cache lock poisoned") = next;
    count
}
