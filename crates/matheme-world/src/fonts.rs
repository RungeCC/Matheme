//! Process-wide font cache.
//!
//! Discovery is expensive on Windows, so the font *book* (metadata) is built
//! once and shared by every compilation. Font *data* is loaded lazily:
//! typst's `Font` holds the entire font file, and a multi-face `.ttc` is held
//! once per face (e.g. a 16-face `SourceHanSans.ttc` costs 16 full copies —
//! several GiB), so eagerly materializing every face is not viable. The
//! `FontStore` keeps one lazy slot per face and only reads a file when a
//! document actually selects that font.
//!
//! The collection is rebuilt when the configuration changes
//! ([`refresh`]) or when the user explicitly asks for a rescan
//! ([`reload_fonts`]); [`reset`] drops it entirely (used by the host-facing
//! close path).
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
use typst_kit::fonts::{self, FontStore};

use crate::settings;

/// Face counts per source plus the lazily-backed font store.
///
/// Cloning an `Arc<FontCache>` is cheap; the heavy font data only appears
/// inside the store's slots as documents request faces.
pub struct FontCache {
    store: FontStore,
    pub counts: FontCounts,
}

impl FontCache {
    /// Metadata index, aligned with [`FontCache::font`]'s `index` parameter.
    pub fn book(&self) -> &LazyHash<FontBook> {
        self.store.book()
    }

    /// The font at `index`, loading its file on first use. Faces touched by
    /// earlier compilations stay resident; untouched faces cost nothing.
    pub fn font(&self, index: usize) -> Option<Font> {
        self.store.font(index)
    }

    /// Total number of registered faces.
    pub fn len(&self) -> usize {
        self.counts.total()
    }
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
fn extend<T: fonts::FontSource>(
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

    FontCache { store, counts }
}

/// `None` means "dropped, rescan on next use" (see [`reset`]).
static FONTS: OnceLock<RwLock<Option<Arc<FontCache>>>> = OnceLock::new();

fn fonts_slot() -> &'static RwLock<Option<Arc<FontCache>>> {
    FONTS.get_or_init(|| RwLock::new(None))
}

/// Get the current font snapshot without holding the lock across a
/// compilation. This also triggers the lazy font scan (metadata only — no
/// font data is loaded) on first use.
pub fn font_cache() -> Arc<FontCache> {
    if let Some(cache) = fonts_slot()
        .read()
        .expect("font cache lock poisoned")
        .clone()
    {
        return cache;
    }
    let cache = Arc::new(scan());
    let mut slot = fonts_slot().write().expect("font cache lock poisoned");
    // A concurrent `reset()` may have cleared the slot meanwhile; whichever
    // scan is newer wins, both are consistent with some settings snapshot.
    if slot.is_none() {
        *slot = Some(Arc::clone(&cache));
    }
    cache
}

/// Counts of the current snapshot, triggering the scan if needed.
pub fn font_counts() -> FontCounts {
    font_cache().counts
}

/// Counts of the current snapshot, without forcing a scan.
pub fn cached_counts() -> Option<FontCounts> {
    FONTS
        .get()
        .and_then(|slot| slot.read().expect("font cache lock poisoned").clone())
        .map(|cache| cache.counts)
}

/// Replace the cache with a fresh scan and evict memoized entries that could
/// keep the previous generation's fonts alive.
fn install(next: Arc<FontCache>) {
    *fonts_slot().write().expect("font cache lock poisoned") = Some(next);
    // comemo's global cache may hold `Font` clones (e.g. memoized
    // `Font::instantiate` results) pinning the old font data.
    comemo::evict(0);
}

/// Rescan with the current settings. Returns whether the cache was live.
pub(crate) fn refresh() -> bool {
    let Some(slot) = FONTS.get() else {
        // Not initialized yet: the next `font_cache()` picks up the new
        // settings on its own.
        return false;
    };
    if slot.read().expect("font cache lock poisoned").is_none() {
        return false;
    }
    install(Arc::new(scan()));
    true
}

/// Drop the cache entirely; the next use rescans from the settings. Also
/// evicts memoized results so dropped fonts are actually released.
pub fn reset() {
    *fonts_slot().write().expect("font cache lock poisoned") = None;
    comemo::evict(0);
}

/// Eagerly build the font book and return the number of faces found. Only
/// metadata is parsed; font files load lazily on first use.
pub fn warmup_fonts() -> usize {
    font_cache().len()
}

/// Rescan fonts (e.g. after installing a font) and return the new face count.
pub fn reload_fonts() -> usize {
    let next = Arc::new(scan());
    let count = next.len();
    install(next);
    count
}
