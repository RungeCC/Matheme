//! Read-only queries over the process-wide metadata caches.
//!
//! The queryable surface is exactly what the caches hold: the font book and
//! each face's provenance/residency, the listing of locally installed
//! packages, and the Universe index once typst has fetched it. A query never
//! loads font bytes, writes to disk or reaches the network; the only way to
//! populate the Universe index is an explicit warmup (or a compilation).

use typst::text::{FontFlags, FontInfo, FontStyle};

use crate::fonts::{self, FontCache, FontCounts};
use crate::packages;

pub use crate::packages::{PackageEntry, PackageOrigin, UniverseMetadata};

/// Which cache a query inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Just the state of the caches, without listing entries.
    Caches,
    Fonts,
    Packages,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Caches => "Caches",
            Self::Fonts => "Fonts",
            Self::Packages => "Packages",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("caches") {
            Some(Self::Caches)
        } else if name.eq_ignore_ascii_case("fonts") {
            Some(Self::Fonts)
        } else if name.eq_ignore_ascii_case("packages") {
            Some(Self::Packages)
        } else {
            None
        }
    }
}

/// Where a registered face came from. Also the priority order in which the
/// sources are added to the font book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontOrigin {
    Custom,
    Host,
    Embedded,
    System,
}

impl FontOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::Host => "Host",
            Self::Embedded => "Embedded",
            Self::System => "System",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [
            (Self::Custom, "Custom"),
            (Self::Host, "Host"),
            (Self::Embedded, "Embedded"),
            (Self::System, "System"),
        ]
        .into_iter()
        .find(|(_, label)| name.eq_ignore_ascii_case(label))
        .map(|(origin, _)| origin)
    }
}

/// Restrict package results to one storage location. `Local` covers both the
/// data and the cache directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageOriginFilter {
    Local,
    Data,
    Cache,
    Universe,
}

impl PackageOriginFilter {
    pub fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("local") {
            Some(Self::Local)
        } else if name.eq_ignore_ascii_case("data") {
            Some(Self::Data)
        } else if name.eq_ignore_ascii_case("cache") {
            Some(Self::Cache)
        } else if name.eq_ignore_ascii_case("universe") {
            Some(Self::Universe)
        } else {
            None
        }
    }

    fn matches(self, origin: PackageOrigin) -> bool {
        match self {
            Self::Local => origin != PackageOrigin::Universe,
            Self::Data => origin == PackageOrigin::Data,
            Self::Cache => origin == PackageOrigin::Cache,
            Self::Universe => origin == PackageOrigin::Universe,
        }
    }
}

/// Paging applied after filtering. `limit == None` means "all".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub offset: usize,
    pub limit: Option<usize>,
}

impl Page {
    pub fn slice<T>(&self, items: &mut Vec<T>) {
        let start = self.offset.min(items.len());
        items.drain(..start);
        if let Some(limit) = self.limit {
            items.truncate(limit);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FontFilter {
    /// Case-insensitive substring of the family name.
    pub family: Option<String>,
    pub origin: Option<FontOrigin>,
    pub resident: Option<bool>,
    /// Only faces whose coverage contains this codepoint.
    pub character: Option<char>,
}

impl FontFilter {
    fn matches(&self, face: &FontFace) -> bool {
        if let Some(family) = &self.family {
            if !face.family.to_lowercase().contains(&family.to_lowercase()) {
                return false;
            }
        }
        if self.origin.is_some_and(|origin| origin != face.origin) {
            return false;
        }
        if self
            .resident
            .is_some_and(|resident| resident != face.resident)
        {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct PackageFilter {
    /// Case-insensitive substring of the package name.
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub origin: Option<PackageOriginFilter>,
    pub version: Option<String>,
}

impl PackageFilter {
    fn matches(&self, entry: &packages::PackageEntry) -> bool {
        if let Some(name) = &self.name {
            if !entry.name.to_lowercase().contains(&name.to_lowercase()) {
                return false;
            }
        }
        if let Some(namespace) = &self.namespace {
            if !entry.namespace.eq_ignore_ascii_case(namespace) {
                return false;
            }
        }
        if let Some(origin) = self.origin {
            if !origin.matches(entry.origin) {
                return false;
            }
        }
        if let Some(version) = &self.version {
            if entry.version != *version {
                return false;
            }
        }
        true
    }
}

/// One row of the font book, joined with its cached provenance.
#[derive(Debug, Clone)]
pub struct FontFace {
    /// Index into the font book (and `World::font`).
    pub index: usize,
    pub family: String,
    pub style: &'static str,
    pub weight: u16,
    pub stretch: f64,
    pub flags: Vec<&'static str>,
    pub origin: FontOrigin,
    pub path: Option<String>,
    pub collection_index: u32,
    /// Whether the face's bytes are currently held in memory.
    pub resident: bool,
}

fn style_name(style: FontStyle) -> &'static str {
    match style {
        FontStyle::Normal => "Normal",
        FontStyle::Italic => "Italic",
        FontStyle::Oblique => "Oblique",
    }
}

fn flag_names(info: &FontInfo) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if info.flags.contains(FontFlags::MONOSPACE) {
        flags.push("Monospace");
    }
    if info.flags.contains(FontFlags::SERIF) {
        flags.push("Serif");
    }
    if info.flags.contains(FontFlags::MATH) {
        flags.push("Math");
    }
    if info.flags.contains(FontFlags::VARIABLE) {
        flags.push("Variable");
    }
    flags
}

fn collect_faces(cache: &FontCache, character: Option<char>) -> Vec<FontFace> {
    let book = cache.book();
    (0..cache.len())
        .filter_map(|index| {
            let info = book.info(index)?;
            if character.is_some_and(|c| !info.coverage.contains(c as u32)) {
                return None;
            }
            let metadata = cache.face(index)?;
            Some(FontFace {
                index,
                family: info.family.clone(),
                style: style_name(info.variant.style),
                weight: info.variant.weight.to_number(),
                stretch: info.variant.stretch.to_ratio().get(),
                flags: flag_names(info),
                origin: metadata.origin,
                path: metadata
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                collection_index: metadata.collection_index,
                resident: metadata.resident(),
            })
        })
        .collect()
}

pub struct FontAnswer {
    /// Faces in the cache, before filtering.
    pub total: usize,
    pub matched: usize,
    pub resident: usize,
    pub faces: Vec<FontFace>,
}

pub struct PackageAnswer {
    pub local_total: usize,
    pub universe_total: usize,
    pub universe_cached: bool,
    pub matched: usize,
    pub entries: Vec<packages::PackageEntry>,
}

impl PackageAnswer {
    pub fn total(&self) -> usize {
        self.local_total + self.universe_total
    }
}

/// State of the caches without initializing anything.
#[derive(Debug, Clone)]
pub struct CacheState {
    pub fonts_cached: bool,
    pub font_counts: Option<FontCounts>,
    pub resident: usize,
    pub packages_cached: bool,
    /// `None` when the local listing has not been built yet.
    pub local_packages: Option<usize>,
    pub universe_cached: bool,
    /// `None` when the Universe index has not been fetched yet.
    pub universe_packages: Option<usize>,
}

pub fn fonts(filter: &FontFilter, page: &Page) -> FontAnswer {
    let cache = fonts::font_cache();
    let total = cache.len();
    let resident = cache.resident_count();
    let mut faces = collect_faces(&cache, filter.character);
    faces.retain(|face| filter.matches(face));
    let matched = faces.len();
    page.slice(&mut faces);
    FontAnswer {
        total,
        matched,
        resident,
        faces,
    }
}

pub fn packages(filter: &PackageFilter, page: &Page) -> PackageAnswer {
    let runtime = packages::package_runtime();
    let local = runtime.metadata.local();
    let universe = runtime.metadata.universe();
    let local_total = local.len();
    let universe_total = universe.as_ref().map_or(0, |entries| entries.len());

    let mut entries: Vec<packages::PackageEntry> = Vec::with_capacity(local_total + universe_total);
    entries.extend(local.iter().cloned());
    if let Some(universe) = &universe {
        entries.extend(universe.iter().cloned());
    }
    entries.retain(|entry| filter.matches(entry));
    let matched = entries.len();
    page.slice(&mut entries);

    PackageAnswer {
        local_total,
        universe_total,
        universe_cached: universe.is_some(),
        matched,
        entries,
    }
}

pub fn caches() -> CacheState {
    let font_cache = fonts::cached();
    let runtime = packages::cached();
    let (local_packages, universe_packages) = runtime
        .as_ref()
        .map(|runtime| runtime.metadata.cached_counts())
        .unwrap_or((None, None));
    CacheState {
        fonts_cached: font_cache.is_some(),
        font_counts: font_cache.as_ref().map(|cache| cache.counts),
        resident: font_cache
            .as_ref()
            .map_or(0, |cache| cache.resident_count()),
        packages_cached: runtime.is_some(),
        local_packages,
        universe_cached: universe_packages.is_some(),
        universe_packages,
    }
}

/// Whether the Universe index has already been fetched in this loader
/// generation.
pub fn universe_cached() -> bool {
    packages::cached()
        .and_then(|runtime| runtime.metadata.universe())
        .is_some()
}

/// Rescan the local package directories when the caller explicitly asks for
/// fresh filesystem data.
pub fn refresh_local() {
    if let Some(runtime) = packages::cached() {
        runtime.metadata.refresh_local();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, origin: FontOrigin, resident: bool) -> FontFace {
        FontFace {
            index: 0,
            family: family.to_owned(),
            style: "Normal",
            weight: 400,
            stretch: 1.0,
            flags: vec![],
            origin,
            path: None,
            collection_index: 0,
            resident,
        }
    }

    fn entry(name: &str, origin: PackageOrigin) -> PackageEntry {
        PackageEntry {
            namespace: "preview".to_owned(),
            name: name.to_owned(),
            version: "0.1.0".to_owned(),
            origin,
            path: None,
            metadata: None,
        }
    }

    #[test]
    fn page_slices_matched_rows() {
        let mut items = vec![0, 1, 2, 3, 4];
        let page = Page {
            offset: 2,
            limit: Some(2),
        };
        page.slice(&mut items);
        assert_eq!(items, vec![2, 3]);

        let mut items = vec![0, 1];
        Page {
            offset: 99,
            limit: None,
        }
        .slice(&mut items);
        assert!(items.is_empty());
    }

    #[test]
    fn origins_parse_case_insensitively() {
        assert_eq!(FontOrigin::parse("embedded"), Some(FontOrigin::Embedded));
        assert_eq!(FontOrigin::parse("System"), Some(FontOrigin::System));
        assert_eq!(FontOrigin::parse("bogus"), None);
        assert_eq!(
            PackageOriginFilter::parse("LOCAL"),
            Some(PackageOriginFilter::Local)
        );
    }

    #[test]
    fn font_filters_combine() {
        let filter = FontFilter {
            family: Some("libertinus".to_owned()),
            origin: Some(FontOrigin::Embedded),
            resident: Some(true),
            character: None,
        };
        assert!(filter.matches(&face("Libertinus Serif", FontOrigin::Embedded, true)));
        assert!(!filter.matches(&face("Libertinus Serif", FontOrigin::System, true)));
        assert!(!filter.matches(&face("Libertinus Serif", FontOrigin::Embedded, false)));
        assert!(!filter.matches(&face("DejaVu Sans", FontOrigin::Embedded, true)));
    }

    #[test]
    fn package_filters_combine() {
        let filter = PackageFilter {
            name: Some("cetz".to_owned()),
            namespace: Some("preview".to_owned()),
            origin: Some(PackageOriginFilter::Local),
            version: None,
        };
        assert!(filter.matches(&entry("cetz", PackageOrigin::Data)));
        assert!(!filter.matches(&entry("cetz", PackageOrigin::Universe)));
        assert!(!filter.matches(&entry("other", PackageOrigin::Data)));
    }
}
