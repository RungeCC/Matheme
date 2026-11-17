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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;
use typst::syntax::package::VersionlessPackageSpec;
use typst_kit::downloader::Downloader;
use typst_kit::packages::{FsPackages, SystemPackages, UniversePackages};

use crate::settings::{self, Settings};

/// Two independent switches: the configured policy and a per-call opt-in.
struct DownloadGate {
    configured: AtomicBool,
    call: AtomicBool,
}

impl DownloadGate {
    fn new(configured: bool) -> Self {
        Self {
            configured: AtomicBool::new(configured),
            call: AtomicBool::new(false),
        }
    }

    fn allowed(&self) -> bool {
        self.configured.load(Ordering::Relaxed) || self.call.load(Ordering::Relaxed)
    }

    fn set_configured(&self, allowed: bool) {
        self.configured.store(allowed, Ordering::Relaxed);
    }

    fn set_call(&self, allowed: bool) {
        self.call.store(allowed, Ordering::Relaxed);
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

fn field(entry: &Value, key: &str) -> Option<String> {
    entry.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn field_list(entry: &Value, key: &str) -> Vec<String> {
    entry
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn universe_entry(entry: &Value) -> Option<PackageEntry> {
    Some(PackageEntry {
        namespace: UniversePackages::NAMESPACE.to_owned(),
        name: field(entry, "name")?,
        version: field(entry, "version")?,
        origin: PackageOrigin::Universe,
        path: None,
        metadata: Some(UniverseMetadata {
            description: field(entry, "description"),
            authors: field_list(entry, "authors"),
            license: field(entry, "license"),
            keywords: field_list(entry, "keywords"),
            categories: field_list(entry, "categories"),
            disciplines: field_list(entry, "disciplines"),
            entrypoint: field(entry, "entrypoint"),
            repository: field(entry, "repository"),
            homepage: field(entry, "homepage"),
            compiler: field(entry, "compiler"),
            updated_at: entry.get("updatedAt").and_then(Value::as_i64),
            template: entry.get("template").is_some(),
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
        if let Some(entries) = self
            .local
            .read()
            .expect("package metadata poisoned")
            .clone()
        {
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
        *self.local.write().expect("package metadata poisoned") = Some(Arc::clone(&entries));
        entries
    }

    /// Drop the local listing; the next access rescans the directories.
    pub fn invalidate_local(&self) {
        *self.local.write().expect("package metadata poisoned") = None;
    }

    /// Rescan the local directories immediately.
    pub fn refresh_local(&self) {
        self.invalidate_local();
        drop(self.local());
    }

    /// Universe index entries, if the index has been fetched in this loader
    /// generation. A query never triggers a download on its own.
    pub fn universe(&self) -> Option<Arc<Vec<PackageEntry>>> {
        self.universe
            .read()
            .expect("package metadata poisoned")
            .clone()
    }

    /// Entry counts that are already cached, without forcing either a scan or
    /// a network request (`None` means "not cached yet").
    pub fn cached_counts(&self) -> (Option<usize>, Option<usize>) {
        (
            self.local
                .read()
                .expect("package metadata poisoned")
                .as_ref()
                .map(|entries| entries.len()),
            self.universe
                .read()
                .expect("package metadata poisoned")
                .as_ref()
                .map(|entries| entries.len()),
        )
    }

    fn set_universe(&self, bytes: &[u8]) {
        let Ok(values) = serde_json::from_slice::<Vec<Value>>(bytes) else {
            return;
        };
        let entries: Vec<_> = values.iter().filter_map(universe_entry).collect();
        *self.universe.write().expect("package metadata poisoned") = Some(Arc::new(entries));
    }
}

struct GatedDownloader {
    agent: ureq::Agent,
    gate: Arc<DownloadGate>,
    metadata: Arc<MetadataCache>,
}

fn is_universe_index(url: &str) -> bool {
    url.ends_with("/preview/index.json")
}

impl Downloader for GatedDownloader {
    fn stream(&self, _key: &dyn Any, url: &str) -> io::Result<(Option<usize>, Box<dyn Read>)> {
        if !self.gate.allowed() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "package downloads are disabled (set Downloads -> True or AllowDownloads -> True)",
            ));
        }

        let response = self.agent.get(url).call().map_err(|err| match err {
            ureq::Error::Status(404, _) => io::Error::new(io::ErrorKind::NotFound, err),
            err => io::Error::other(err),
        })?;
        let size = response
            .header("Content-Length")
            .and_then(|value| value.parse().ok());
        let mut reader = response.into_reader();

        if is_universe_index(url) {
            // typst caches the index inside `UniversePackages`; buffer the very
            // same response to keep a parsed copy for queries. Re-serving the
            // bytes means this is not a second request.
            let mut bytes = Vec::with_capacity(size.unwrap_or(0));
            reader.read_to_end(&mut bytes)?;
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
    if let Ok(tls) = ureq::native_tls::TlsConnector::new() {
        agent = agent.tls_connector(Arc::new(tls));
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

static PACKAGES: OnceLock<RwLock<Option<Arc<PackageRuntime>>>> = OnceLock::new();

fn package_slot() -> &'static RwLock<Option<Arc<PackageRuntime>>> {
    PACKAGES.get_or_init(|| RwLock::new(None))
}

pub(crate) fn package_runtime() -> Arc<PackageRuntime> {
    if let Some(runtime) = package_slot()
        .read()
        .expect("package cache lock poisoned")
        .clone()
    {
        return runtime;
    }
    let runtime = Arc::new(build(&settings::settings()));
    let mut slot = package_slot().write().expect("package cache lock poisoned");
    if slot.is_none() {
        *slot = Some(Arc::clone(&runtime));
    }
    runtime
}

/// The live loader, if one exists. Unlike [`package_runtime`] this never
/// builds one, so cache-state queries stay read-only.
pub(crate) fn cached() -> Option<Arc<PackageRuntime>> {
    PACKAGES
        .get()
        .and_then(|slot| slot.read().expect("package cache lock poisoned").clone())
}

/// Allow downloads for the current call only. The bridge resets this on every
/// entry point, so a single `AllowDownloads -> True` does not leak.
///
/// Clearing the flag does not build the loader: introspection calls must stay
/// read-only with respect to the caches.
pub fn set_call_downloads(allowed: bool) {
    if !allowed {
        if let Some(runtime) = cached() {
            runtime.gate.set_call(false);
        }
        return;
    }
    package_runtime().gate.set_call(true);
}

/// Mirror a changed `Downloads` setting into the live loader, if it exists.
pub(crate) fn set_configured_downloads(allowed: bool) -> bool {
    let Some(slot) = PACKAGES.get() else {
        return false;
    };
    let Some(runtime) = slot.read().expect("package cache lock poisoned").clone() else {
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
/// opened temporarily, so this works even when downloads are configured off.
///
/// Returns the resolved version of a well-known package on success, or the
/// error message on failure (e.g. no network / proxy misconfigured).
pub fn prefetch_universe_index() -> Result<String, String> {
    let runtime = package_runtime();
    let spec: VersionlessPackageSpec = "@preview/cetz"
        .parse()
        .expect("valid versionless package spec");
    runtime.gate.set_call(true);
    let result = runtime
        .packages
        .universe()
        .latest_version(&spec)
        .map(|version| version.to_string())
        .map_err(|err| err.to_string());
    runtime.gate.set_call(false);
    result
}

/// Rebuild the loader unconditionally and report its status.
pub fn reload_packages() -> PackageStatus {
    let next = Arc::new(build(&settings::settings()));
    *package_slot().write().expect("package cache lock poisoned") = Some(next);
    package_status()
}

/// Rebuild the loader after the package configuration changed. Returns whether
/// the loader was live; when it is not, the next access builds it from the new
/// settings anyway.
pub(crate) fn rebuild() -> bool {
    let Some(slot) = PACKAGES.get() else {
        return false;
    };
    if slot.read().expect("package cache lock poisoned").is_none() {
        return false;
    }
    let next = Arc::new(build(&settings::settings()));
    *slot.write().expect("package cache lock poisoned") = Some(next);
    true
}

/// Drop the loader (including the cached Universe index); the next use
/// rebuilds it from the settings.
pub fn reset() {
    *package_slot().write().expect("package cache lock poisoned") = None;
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
}
