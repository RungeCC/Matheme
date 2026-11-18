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
//! Alongside the book we keep a small per-face [`FaceMetadata`] record: where
//! the face came from, its file/collection index and whether its bytes are
//! currently resident. That record is the whole queryable surface of this
//! cache ([`crate::query`]); reading it never loads a font.
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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use typst::text::{Font, FontBook, FontInfo};
use typst::utils::LazyHash;
use typst_kit::fonts::{self, FontPath, FontSource, FontStore};

use crate::cache::LazyCache;
use crate::query::FontOrigin;
use crate::settings;

/// Per-face provenance. Kept next to the font book so queries can answer
/// "where did this face come from" and "is it in memory" without reading a
/// single font file.
pub(crate) struct FaceMetadata {
    pub origin: FontOrigin,
    /// `None` for embedded fonts, whose bytes live in the store from the start.
    pub path: Option<PathBuf>,
    pub collection_index: u32,
    resident: Arc<AtomicBool>,
}

impl FaceMetadata {
    fn file(origin: FontOrigin, path: &FontPath) -> Self {
        Self {
            origin,
            path: Some(path.path.clone()),
            collection_index: path.index,
            resident: Arc::new(AtomicBool::new(false)),
        }
    }

    fn embedded(collection_index: u32) -> Self {
        Self {
            origin: FontOrigin::Embedded,
            path: None,
            collection_index,
            // Embedded fonts are `Font` values held by the source itself.
            resident: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Whether this face's bytes are currently held in memory.
    pub fn resident(&self) -> bool {
        self.resident.load(Ordering::Relaxed)
    }
}

/// A `FontSource` that records when the store first loads a face.
///
/// `FontStore` does not expose its lazy slots' state, but it does let us supply
/// the source, so the flag is set from inside `load` — exactly once per face.
struct Tracked<S> {
    inner: S,
    resident: Arc<AtomicBool>,
}

impl<S: FontSource> FontSource for Tracked<S> {
    fn load(&self) -> Option<Font> {
        self.resident.store(true, Ordering::Relaxed);
        self.inner.load()
    }
}

/// Face counts per source plus the lazily-backed font store.
///
/// Cloning an `Arc<FontCache>` is cheap; the heavy font data only appears
/// inside the store's slots as documents request faces.
pub struct FontCache {
    store: FontStore,
    faces: Vec<FaceMetadata>,
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

    /// Whether no faces are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Provenance of the face at `index`, if the index is registered.
    pub(crate) fn face(&self, index: usize) -> Option<&FaceMetadata> {
        self.faces.get(index)
    }

    /// How many faces currently hold their bytes in memory.
    pub(crate) fn resident_count(&self) -> usize {
        self.faces.iter().filter(|face| face.resident()).count()
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

fn add_files(
    store: &mut FontStore,
    faces: &mut Vec<FaceMetadata>,
    entries: impl IntoIterator<Item = (FontPath, FontInfo)>,
    origin: FontOrigin,
) -> usize {
    let mut count = 0;
    for (path, info) in entries {
        let metadata = FaceMetadata::file(origin, &path);
        let resident = Arc::clone(&metadata.resident);
        faces.push(metadata);
        store.push((
            Tracked {
                inner: path,
                resident,
            },
            info,
        ));
        count += 1;
    }
    count
}

fn add_embedded(
    store: &mut FontStore,
    faces: &mut Vec<FaceMetadata>,
    entries: impl IntoIterator<Item = (Font, FontInfo)>,
) -> usize {
    let mut count = 0;
    for (font, info) in entries {
        let metadata = FaceMetadata::embedded(font.index());
        let resident = Arc::clone(&metadata.resident);
        faces.push(metadata);
        store.push((
            Tracked {
                inner: font,
                resident,
            },
            info,
        ));
        count += 1;
    }
    count
}

fn scan() -> FontCache {
    let settings = settings::settings();
    let mut store = FontStore::new();
    let mut faces = Vec::new();
    let mut counts = FontCounts::default();

    for dir in &settings.font_paths {
        counts.custom += add_files(&mut store, &mut faces, fonts::scan(dir), FontOrigin::Custom);
    }
    for dir in &settings.host_font_paths {
        counts.host += add_files(&mut store, &mut faces, fonts::scan(dir), FontOrigin::Host);
    }
    if settings.embedded_fonts {
        counts.embedded += add_embedded(&mut store, &mut faces, fonts::embedded());
    }
    if settings.system_fonts {
        counts.system += add_files(&mut store, &mut faces, fonts::system(), FontOrigin::System);
    }

    FontCache {
        store,
        faces,
        counts,
    }
}

/// `None` means "dropped, rescan on next use" (see [`reset`]).
static FONTS: LazyCache<FontCache> = LazyCache::new();

/// Get the current font snapshot without holding the lock across a
/// compilation. This also triggers the lazy font scan (metadata only — no
/// font data is loaded) on first use.
pub fn font_cache() -> Arc<FontCache> {
    FONTS.get_or_init(scan)
}

/// The live snapshot, if one exists. Unlike [`font_cache`] this never scans,
/// so cache-state queries stay read-only.
pub(crate) fn cached() -> Option<Arc<FontCache>> {
    FONTS.cached()
}

/// Counts of the current snapshot, triggering the scan if needed.
pub fn font_counts() -> FontCounts {
    font_cache().counts
}

/// Counts of the current snapshot, without forcing a scan.
pub fn cached_counts() -> Option<FontCounts> {
    cached().map(|cache| cache.counts)
}

/// Replace the cache with a fresh scan and evict memoized entries that could
/// keep the previous generation's fonts alive.
fn install() -> Arc<FontCache> {
    let next = FONTS.install(scan());
    // comemo's global cache may hold `Font` clones (e.g. memoized
    // `Font::instantiate` results) pinning the old font data.
    comemo::evict(0);
    next
}

/// Rescan with the current settings. Returns whether the cache was live.
pub(crate) fn refresh() -> bool {
    if FONTS.refresh_if_live(scan).is_some() {
        comemo::evict(0);
        true
    } else {
        false
    }
}

/// Drop the cache entirely; the next use rescans from the settings. Also
/// evicts memoized results so dropped fonts are actually released.
pub fn reset() {
    FONTS.reset();
    comemo::evict(0);
}

/// Eagerly build the font book and return the number of faces found. Only
/// metadata is parsed; font files load lazily on first use.
pub fn warmup_fonts() -> usize {
    font_cache().len()
}

/// Rescan fonts (e.g. after installing a font) and return the new face count.
pub fn reload_fonts() -> usize {
    install().len()
}
