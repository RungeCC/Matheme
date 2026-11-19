//! Read-only queries over the process-wide metadata caches.
//!
//! Fonts and packages are separate query surfaces: each answers with its own
//! row shape, so neither needs a discriminating "target" argument. The
//! queryable surface is exactly what the caches hold — the font book and each
//! face's provenance/residency, the listing of locally installed packages, and
//! the Universe index once typst has fetched it. A query never loads font
//! bytes, writes to disk or reaches the network; the only way to populate the
//! Universe index is an explicit opt-in (or a compilation).

use typst::text::{FontFlags, FontInfo, FontStyle};

use crate::fonts;
use crate::packages;

pub use crate::packages::{PackageEntry, PackageOrigin, UniverseMetadata};

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

    /// Count all items yielded by `iter`, materializing (via `materialize`) only
    /// those that fall inside `[self.offset, self.offset + self.limit)`.
    fn collect_window<T, U>(
        &self,
        iter: impl Iterator<Item = T>,
        mut materialize: impl FnMut(T) -> U,
    ) -> (usize, Vec<U>) {
        let mut matched = 0usize;
        let mut out = self.limit.map(Vec::with_capacity).unwrap_or_default();
        for item in iter {
            let idx = matched;
            matched += 1;
            if idx < self.offset {
                continue;
            }
            if self.limit.is_some_and(|limit| out.len() >= limit) {
                continue;
            }
            out.push(materialize(item));
        }
        (matched, out)
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

/// Case-insensitive substring check without allocating on the ASCII fast path.
fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let needle_bytes = needle.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle_bytes.len())
            .any(|window| window.eq_ignore_ascii_case(needle_bytes));
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

impl FontFilter {
    fn matches_fields(&self, family: &str, origin: FontOrigin, resident: bool) -> bool {
        if let Some(needle) = &self.family {
            if !contains_case_insensitive(family, needle) {
                return false;
            }
        }
        if self.origin.is_some_and(|expected| expected != origin) {
            return false;
        }
        if self.resident.is_some_and(|expected| expected != resident) {
            return false;
        }
        true
    }

    #[cfg(test)]
    fn matches(&self, face: &FontFace) -> bool {
        self.matches_fields(&face.family, face.origin, face.resident)
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
            if !contains_case_insensitive(&entry.name, name) {
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

fn build_face(index: usize, info: &FontInfo, metadata: &crate::fonts::FaceMetadata) -> FontFace {
    FontFace {
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
    }
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

pub fn fonts(filter: &FontFilter, page: &Page) -> FontAnswer {
    let cache = fonts::font_cache();
    let total = cache.len();
    let resident = cache.resident_count();
    let book = cache.book();

    let matching = (0..total).filter_map(|index| {
        let info = book.info(index)?;
        if filter
            .character
            .is_some_and(|c| !info.coverage.contains(c as u32))
        {
            return None;
        }
        let metadata = cache.face(index)?;
        if !filter.matches_fields(&info.family, metadata.origin, metadata.resident()) {
            return None;
        }
        Some((index, info, metadata))
    });

    let (matched, faces) = page.collect_window(matching, |(index, info, metadata)| {
        build_face(index, info, metadata)
    });

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

    let matching = local
        .iter()
        .chain(universe.iter().flat_map(|entries| entries.iter()))
        .filter(|entry| filter.matches(entry));

    let (matched, entries) = page.collect_window(matching, Clone::clone);

    PackageAnswer {
        local_total,
        universe_total,
        universe_cached: universe.is_some(),
        matched,
        entries,
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
/// fresh filesystem data. When the package runtime is not yet initialized,
/// the subsequent `packages()` query initializes and scans it cleanly.
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

    #[test]
    fn collect_window_materializes_only_paged_items() {
        let mut materialized = 0usize;
        let page = Page {
            offset: 2,
            limit: Some(3),
        };
        let (matched, items) = page.collect_window(0..10, |n| {
            materialized += 1;
            n * 10
        });
        assert_eq!(matched, 10);
        assert_eq!(items, vec![20, 30, 40]);
        assert_eq!(materialized, 3);
    }
}
