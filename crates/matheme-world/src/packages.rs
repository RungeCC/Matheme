//! Typst Universe packages: process-wide loader, download policy and status.
//!
//! The loader is configured from [`crate::settings`]: package directories,
//! proxy and the download policy all live there. A per-call opt-in
//! (`AllowDownloads`) can additionally open the gate for one compilation
//! without changing the stored configuration.
//!
//! The two metadata sets that matter to callers are cached here as well: the
//! listing of locally installed packages, and the Universe index (captured
//! when typst downloads it). [`crate::query`] is a read-only view over them.

use std::any::Any;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use serde::Deserialize;
use serde_json::Value;
use typst::syntax::package::VersionlessPackageSpec;
use typst_kit::downloader::Downloader;
use typst_kit::packages::{FsPackages, SystemPackages, UniversePackages};

use crate::cache::{read_unpoisoned, write_unpoisoned, LazyCache};
use crate::settings::{self, Settings};

/// Upper bound when buffering `/preview/index.json` in memory (32 MiB).
const MAX_UNIVERSE_INDEX_BYTES: u64 = 32 * 1024 * 1024;

/// Two independent switches: the configured policy and a reference-counted
/// per-call opt-in, plus a flag recording whether a download was blocked.
struct DownloadGate {
    configured: AtomicBool,
    call_depth: AtomicUsize,
    blocked: AtomicBool,
}

impl DownloadGate {
    fn new(configured: bool) -> Self {
        Self {
            configured: AtomicBool::new(configured),
            call_depth: AtomicUsize::new(0),
            blocked: AtomicBool::new(false),
        }
    }

    fn allowed(&self) -> bool {
        self.configured.load(Ordering::Acquire) || self.call_depth.load(Ordering::Acquire) > 0
    }

    fn set_configured(&self, allowed: bool) {
        self.configured.store(allowed, Ordering::Release);
    }

    fn acquire_call(&self) {
        self.call_depth.fetch_add(1, Ordering::AcqRel);
    }

    fn release_call(&self) {
        let prev = self.call_depth.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "DownloadGate::release_call underflow");
    }

    fn clear_blocked(&self) {
        self.blocked.store(false, Ordering::Release);
    }

    fn mark_blocked(&self) {
        self.blocked.store(true, Ordering::Release);
    }

    fn was_blocked(&self) -> bool {
        self.blocked.load(Ordering::Acquire)
    }
}

/// RAII scope that activates a per-call download opt-in (when `allowed` is
/// true) and tracks whether a package download was rejected while active.
pub struct CallDownloadScope {
    gate: Arc<DownloadGate>,
    allowed: bool,
}

impl CallDownloadScope {
    /// Returns `true` if `GatedDownloader` rejected a download because
    /// downloads were disabled during this call.
    pub fn download_blocked(&self) -> bool {
        self.gate.was_blocked()
    }
}

impl Drop for CallDownloadScope {
    fn drop(&mut self) {
        if self.allowed {
            self.gate.release_call();
        }
    }
}

/// Where a package entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageOrigin {
    /// A package in the data directory (user-installed).
    Data,
    /// A package in the cache directory (downloaded).
    Cache,
    /// An entry of the Typst Universe index.
    Universe,
}

impl PackageOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "Data",
            Self::Cache => "Cache",
            Self::Universe => "Universe",
        }
    }
}

/// Fields of a Universe index entry. The index is intentionally left as JSON
/// by typst (its format is not stable), so unknown fields are simply ignored.
#[derive(Debug, Clone, Default)]
pub struct UniverseMetadata {
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub disciplines: Vec<String>,
    pub entrypoint: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub compiler: Option<String>,
    pub updated_at: Option<i64>,
    pub template: bool,
}

/// One row of the cached package metadata.
#[derive(Debug, Clone)]
pub struct PackageEntry {
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub origin: PackageOrigin,
    /// Directory of a local package; `None` for Universe index entries.
    pub path: Option<PathBuf>,
    /// Extra fields from the Universe index; `None` for local packages.
    pub metadata: Option<UniverseMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUniverseEntry {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    license: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    disciplines: Vec<String>,
    entrypoint: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    compiler: Option<String>,
    updated_at: Option<i64>,
    #[serde(default)]
    template: Option<Value>,
}

fn universe_entry(entry: &Value) -> Option<PackageEntry> {
    let raw: RawUniverseEntry = serde_json::from_value(entry.clone()).ok()?;
    let template = raw
        .template
        .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false));
    Some(PackageEntry {
        namespace: UniversePackages::NAMESPACE.to_owned(),
        name: raw.name?,
        version: raw.version?,
        origin: PackageOrigin::Universe,
        path: None,
        metadata: Some(UniverseMetadata {
            description: raw.description,
            authors: raw.authors,
            license: raw.license,
            keywords: raw.keywords,
            categories: raw.categories,
            disciplines: raw.disciplines,
            entrypoint: raw.entrypoint,
            repository: raw.repository,
            homepage: raw.homepage,
            compiler: raw.compiler,
            updated_at: raw.updated_at,
            template,
        }),
    })
}

/// List the local package tree `namespace/name/version`.
fn scan_local(root: &Path, origin: PackageOrigin) -> Vec<PackageEntry> {
    let Some(namespaces) = std::fs::read_dir(root).ok() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for namespace in namespaces.flatten() {
        if !namespace.path().is_dir() {
            continue;
        }
        let Some(packages) = std::fs::read_dir(namespace.path()).ok() else {
            continue;
        };
        for package in packages.flatten() {
            let Some(versions) = std::fs::read_dir(package.path()).ok() else {
                continue;
            };
            for version in versions.flatten() {
                let name = version.file_name().to_string_lossy().into_owned();
                if !name.starts_with(|c: char| c.is_ascii_digit()) || !version.path().is_dir() {
                    continue;
                }
                entries.push(PackageEntry {
                    namespace: namespace.file_name().to_string_lossy().into_owned(),
                    name: package.file_name().to_string_lossy().into_owned(),
                    version: name,
                    origin,
                    path: Some(version.path()),
                    metadata: None,
                });
            }
        }
    }
    entries
}

/// Cached, lazily built package metadata.
///
/// `local` is a filesystem listing that is invalidated whenever a package
/// archive is downloaded; `universe` is filled by the downloader when typst
/// fetches the index, so the query layer never needs a second network call.
#[derive(Debug)]
pub struct MetadataCache {
    data_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    local: RwLock<Option<Arc<Vec<PackageEntry>>>>,
    universe: RwLock<Option<Arc<Vec<PackageEntry>>>>,
}

impl MetadataCache {
    fn new(data_dir: Option<PathBuf>, cache_dir: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            cache_dir,
            local: RwLock::new(None),
            universe: RwLock::new(None),
        }
    }

    /// Locally installed packages, scanning the directories on first use.
    pub fn local(&self) -> Arc<Vec<PackageEntry>> {
        if let Some(entries) = read_unpoisoned(&self.local).clone() {
            return entries;
        }
        let mut entries = Vec::new();
        if let Some(dir) = &self.data_dir {
            entries.extend(scan_local(dir, PackageOrigin::Data));
        }
        if let Some(dir) = &self.cache_dir {
            entries.extend(scan_local(dir, PackageOrigin::Cache));
        }
        entries.sort_by(|a, b| {
            (&a.namespace, &a.name, &a.version).cmp(&(&b.namespace, &b.name, &b.version))
        });
        let entries = Arc::new(entries);
        *write_unpoisoned(&self.local) = Some(Arc::clone(&entries));
        entries
    }

    /// Drop the local listing; the next access rescans the directories.
    pub fn invalidate_local(&self) {
        *write_unpoisoned(&self.local) = None;
    }

    /// Rescan the local directories immediately.
    pub fn refresh_local(&self) {
        self.invalidate_local();
        drop(self.local());
    }

    /// Universe index entries, if the index has been fetched in this loader
    /// generation. A query never triggers a download on its own.
    pub fn universe(&self) -> Option<Arc<Vec<PackageEntry>>> {
        read_unpoisoned(&self.universe).clone()
    }

    /// Entry counts that are already cached, without forcing either a scan or
    /// a network request (`None` means "not cached yet").
    pub fn cached_counts(&self) -> (Option<usize>, Option<usize>) {
        (
            read_unpoisoned(&self.local)
                .as_ref()
                .map(|entries| entries.len()),
            read_unpoisoned(&self.universe)
                .as_ref()
                .map(|entries| entries.len()),
        )
    }

    fn set_universe(&self, bytes: &[u8]) {
        let Ok(values) = serde_json::from_slice::<Vec<Value>>(bytes) else {
            return;
        };
        let entries: Vec<_> = values.iter().filter_map(universe_entry).collect();
        *write_unpoisoned(&self.universe) = Some(Arc::new(entries));
    }
}

struct GatedDownloader {
    agent: ureq::Agent,
    gate: Arc<DownloadGate>,
    metadata: Arc<MetadataCache>,
    tls_error: Option<String>,
}

fn is_universe_index(url: &str) -> bool {
    url.ends_with("/preview/index.json")
}

impl Downloader for GatedDownloader {
    fn stream(&self, _key: &dyn Any, url: &str) -> io::Result<(Option<usize>, Box<dyn Read>)> {
        if !self.gate.allowed() {
            self.gate.mark_blocked();
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "package downloads are disabled",
            ));
        }

        if let Some(err) = &self.tls_error {
            if url.starts_with("https://") {
                return Err(io::Error::other(err.clone()));
            }
        }

        let response = self.agent.get(url).call().map_err(|err| match err {
            ureq::Error::Status(404, _) => io::Error::new(io::ErrorKind::NotFound, err),
            err => io::Error::other(err),
        })?;
        let size = response
            .header("Content-Length")
            .and_then(|value| value.parse().ok());
        let reader = response.into_reader();

        if is_universe_index(url) {
            // typst caches the index inside `UniversePackages`; buffer the very
            // same response to keep a parsed copy for queries. Re-serving the
            // bytes means this is not a second request.
            let mut bytes = Vec::with_capacity(size.unwrap_or(0).min(MAX_UNIVERSE_INDEX_BYTES as usize));
            reader
                .take(MAX_UNIVERSE_INDEX_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_UNIVERSE_INDEX_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Universe index exceeds maximum supported size",
                ));
            }
            self.metadata.set_universe(&bytes);
            return Ok((Some(bytes.len()), Box::new(Cursor::new(bytes))));
        }

        // A package archive is about to be stored in the cache directory.
        if url.ends_with(".tar.gz") {
            self.metadata.invalidate_local();
        }
        Ok((size, reader))
    }
}

/// Process-wide package loader. Kept across calls so that the Universe index
/// and downloaded package files are reused; `FileStore` is per-call instead.
pub struct PackageRuntime {
    pub packages: Arc<SystemPackages>,
    pub metadata: Arc<MetadataCache>,
    gate: Arc<DownloadGate>,
    proxy: Option<String>,
    proxy_error: Option<String>,
}

fn build(settings: &Settings) -> PackageRuntime {
    let gate = Arc::new(DownloadGate::new(settings.downloads));

    let mut agent = ureq::AgentBuilder::new()
        .user_agent(concat!("matheme/", env!("CARGO_PKG_VERSION")))
        // The process environment is not a configuration channel here: the
        // proxy is either explicit in `Settings` or there is none.
        .try_proxy_from_env(false);
    let mut proxy_error = None;
    if let Some(proxy) = &settings.proxy {
        match ureq::Proxy::new(proxy) {
            Ok(parsed) => agent = agent.proxy(parsed),
            Err(err) => proxy_error = Some(format!("invalid proxy {proxy:?}: {err}")),
        }
    }
    // ureq's `native-tls` feature only adds an adapter; the connector must be
    // installed explicitly (Windows then uses schannel, no extra DLL).
    let mut tls_error = None;
    match ureq::native_tls::TlsConnector::new() {
        Ok(tls) => agent = agent.tls_connector(Arc::new(tls)),
        Err(err) => tls_error = Some(format!("failed to initialize native TLS connector: {err}")),
    }

    let data = match &settings.package_data {
        Some(path) => Some(FsPackages::new(path.as_path())),
        None => FsPackages::system_data(),
    };
    let cache = match &settings.package_cache {
        Some(path) => Some(FsPackages::new(path.as_path())),
        None => FsPackages::system_cache(),
    };

    let metadata = Arc::new(MetadataCache::new(
        data.as_ref().map(|p| p.path().to_owned()),
        cache.as_ref().map(|p| p.path().to_owned()),
    ));
    let downloader = GatedDownloader {
        agent: agent.build(),
        gate: Arc::clone(&gate),
        metadata: Arc::clone(&metadata),
        tls_error,
    };

    PackageRuntime {
        packages: Arc::new(SystemPackages::from_parts(
            data,
            cache,
            UniversePackages::new(downloader),
        )),
        metadata,
        gate,
        proxy: settings.proxy.clone(),
        proxy_error,
    }
}

/// Validate a proxy URL without constructing the full runtime.
pub fn validate_proxy(proxy: &str) -> Result<(), String> {
    ureq::Proxy::new(proxy).map(|_| ()).map_err(|err| format!("invalid proxy {proxy:?}: {err}"))
}

static PACKAGES: LazyCache<PackageRuntime> = LazyCache::new();

pub(crate) fn package_runtime() -> Arc<PackageRuntime> {
    PACKAGES.get_or_init(|| build(&settings::settings()))
}

/// The live loader, if one exists. Unlike [`package_runtime`] this never
/// builds one, so cache-state queries stay read-only.
pub(crate) fn cached() -> Option<Arc<PackageRuntime>> {
    PACKAGES.cached()
}

/// Enter a scoped per-call download policy. When `allowed` is `true`, downloads
/// are permitted until the returned [`CallDownloadScope`] is dropped (including
/// on early returns or panics), so a per-call opt-in never leaks to later calls.
pub fn enter_call_downloads(allowed: bool) -> CallDownloadScope {
    let runtime = package_runtime();
    runtime.gate.clear_blocked();
    if allowed {
        runtime.gate.acquire_call();
    }
    CallDownloadScope {
        gate: Arc::clone(&runtime.gate),
        allowed,
    }
}

/// Mirror a changed `Downloads` setting into the live loader, if it exists.
pub(crate) fn set_configured_downloads(allowed: bool) -> bool {
    let Some(runtime) = cached() else {
        return false;
    };
    runtime.gate.set_configured(allowed);
    true
}

/// Initialize the process-wide package loader and report its status without
/// compiling anything. This is the eager counterpart of the lazy `OnceLock`
/// initialization that otherwise happens on the first `MathemeWorld::new`.
pub fn warmup_packages() -> PackageStatus {
    package_status()
}

/// Eagerly fetch and cache the Universe index (the `OnceCell` inside
/// `UniversePackages`) without compiling a document. The per-call gate is
/// opened temporarily via an RAII guard, so this works even when downloads are
/// configured off.
///
/// `typst-kit::UniversePackages` exposes no standalone index-fetch API, so we
/// probe `@preview/cetz` to populate its internal index cell.
pub fn prefetch_universe_index() -> Result<String, String> {
    let runtime = package_runtime();
    let spec: VersionlessPackageSpec = "@preview/cetz"
        .parse()
        .expect("valid versionless package spec");
    let _scope = enter_call_downloads(true);
    runtime
        .packages
        .universe()
        .latest_version(&spec)
        .map(|version| version.to_string())
        .map_err(|err| err.to_string())
}

/// Rebuild the loader unconditionally and report its status.
pub fn reload_packages() -> PackageStatus {
    PACKAGES.install(build(&settings::settings()));
    package_status()
}

/// Rebuild the loader after the package configuration changed. Returns whether
/// the loader was live; when it is not, the next access builds it from the new
/// settings anyway.
pub(crate) fn rebuild() -> bool {
    PACKAGES
        .refresh_if_live(|| build(&settings::settings()))
        .is_some()
}

/// Drop the loader (including the cached Universe index); the next use
/// rebuilds it from the settings.
pub fn reset() {
    PACKAGES.reset();
}

pub struct PackageStatus {
    pub data: Option<String>,
    pub cache: Option<String>,
    /// Configured download policy.
    pub downloads: bool,
    /// Whether downloads are allowed for this call (policy or opt-in).
    pub downloads_active: bool,
    pub proxy: Option<String>,
    pub proxy_error: Option<String>,
    pub count: usize,
}

pub fn package_status() -> PackageStatus {
    let runtime = package_runtime();
    let data = runtime
        .packages
        .data()
        .map(|p| p.path().display().to_string());
    let cache = runtime
        .packages
        .cache()
        .map(|p| p.path().display().to_string());
    PackageStatus {
        count: runtime.metadata.local().len(),
        downloads: settings::settings().downloads,
        downloads_active: runtime.gate.allowed(),
        data,
        cache,
        proxy: runtime.proxy.clone(),
        proxy_error: runtime.proxy_error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_entry_reads_index_fields() {
        let value = serde_json::json!({
            "name": "cetz",
            "version": "0.4.0",
            "description": "Draw with Typst",
            "authors": ["A", "B"],
            "license": "MIT",
            "keywords": ["drawing"],
            "template": {"path": "template"},
            "updatedAt": 1704708827
        });
        let entry = universe_entry(&value).expect("entry");
        assert_eq!(entry.namespace, "preview");
        assert_eq!(entry.name, "cetz");
        assert_eq!(entry.origin, PackageOrigin::Universe);
        let metadata = entry.metadata.expect("metadata");
        assert_eq!(metadata.description.as_deref(), Some("Draw with Typst"));
        assert_eq!(metadata.authors, vec!["A", "B"]);
        assert_eq!(metadata.updated_at, Some(1704708827));
        assert!(metadata.template);
        assert!(metadata.keywords.contains(&"drawing".to_owned()));
    }

    #[test]
    fn incomplete_index_entries_are_skipped() {
        assert!(universe_entry(&serde_json::json!({"name": "no-version"})).is_none());
        assert!(universe_entry(&serde_json::json!({"version": "0.1.0"})).is_none());
    }

    #[test]
    fn metadata_cache_starts_empty_and_refreshes() {
        let cache = MetadataCache::new(None, None);
        assert_eq!(cache.cached_counts(), (None, None));
        cache.refresh_local();
        assert_eq!(cache.cached_counts(), (Some(0), None));
        cache.set_universe(br#"[{"name":"cetz","version":"0.4.0"}]"#);
        assert_eq!(cache.cached_counts(), (Some(0), Some(1)));
    }

    #[test]
    fn call_download_scope_resets_on_drop_and_tracks_blocked_downloads() {
        let gate = Arc::new(DownloadGate::new(false));
        assert!(!gate.allowed());

        {
            gate.acquire_call();
            let _scope = CallDownloadScope {
                gate: Arc::clone(&gate),
                allowed: true,
            };
            assert!(gate.allowed());
        }
        assert!(!gate.allowed(), "per-call opt-in must reset on drop");

        let downloader = GatedDownloader {
            agent: ureq::AgentBuilder::new().build(),
            gate: Arc::clone(&gate),
            metadata: Arc::new(MetadataCache::new(None, None)),
            tls_error: None,
        };
        let scope = CallDownloadScope {
            gate: Arc::clone(&gate),
            allowed: false,
        };
        assert!(!scope.download_blocked());
        let err = downloader
            .stream(&(), "https://packages.typst.org/preview/index.json")
            .err()
            .expect("downloads disabled should error");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(scope.download_blocked());
    }
}
