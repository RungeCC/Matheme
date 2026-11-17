//! Typst Universe packages: process-wide loader, download policy and status.
//!
//! The loader is configured from [`crate::settings`]: package directories,
//! proxy and the download policy all live there. A per-call opt-in
//! (`AllowDownloads`) can additionally open the gate for one compilation
//! without changing the stored configuration.

use std::any::Any;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

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

struct GatedDownloader {
    agent: ureq::Agent,
    gate: Arc<DownloadGate>,
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
        Ok((size, Box::new(response.into_reader())))
    }
}

/// Process-wide package loader. Kept across calls so that the Universe index
/// and downloaded package files are reused; `FileStore` is per-call instead.
pub struct PackageRuntime {
    pub packages: Arc<SystemPackages>,
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

    let downloader = GatedDownloader {
        agent: agent.build(),
        gate: Arc::clone(&gate),
    };

    let data = match &settings.package_data {
        Some(path) => Some(FsPackages::new(path.as_path())),
        None => FsPackages::system_data(),
    };
    let cache = match &settings.package_cache {
        Some(path) => Some(FsPackages::new(path.as_path())),
        None => FsPackages::system_cache(),
    };

    PackageRuntime {
        packages: Arc::new(SystemPackages::from_parts(
            data,
            cache,
            UniversePackages::new(downloader),
        )),
        gate,
        proxy: settings.proxy.clone(),
        proxy_error,
    }
}

static PACKAGES: OnceLock<RwLock<Arc<PackageRuntime>>> = OnceLock::new();

fn package_slot() -> &'static RwLock<Arc<PackageRuntime>> {
    PACKAGES.get_or_init(|| RwLock::new(Arc::new(build(&settings::settings()))))
}

pub(crate) fn package_runtime() -> Arc<PackageRuntime> {
    package_slot()
        .read()
        .expect("package cache lock poisoned")
        .clone()
}

/// Allow downloads for the current call only. The bridge resets this on every
/// entry point, so a single `AllowDownloads -> True` does not leak.
pub fn set_call_downloads(allowed: bool) {
    package_runtime().gate.set_call(allowed);
}

/// Mirror a changed `Downloads` setting into the live loader, if it exists.
pub(crate) fn set_configured_downloads(allowed: bool) -> bool {
    let Some(slot) = PACKAGES.get() else {
        return false;
    };
    slot.read()
        .expect("package cache lock poisoned")
        .gate
        .set_configured(allowed);
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
    *package_slot().write().expect("package cache lock poisoned") = next;
    package_status()
}

/// Rebuild the loader after the package configuration changed. Returns whether
/// the loader was live; when it is not, the next access builds it from the new
/// settings anyway.
pub(crate) fn rebuild() -> bool {
    let Some(slot) = PACKAGES.get() else {
        return false;
    };
    let next = Arc::new(build(&settings::settings()));
    *slot.write().expect("package cache lock poisoned") = next;
    true
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

fn count_packages(path: &Path) -> usize {
    let Some(namespaces) = std::fs::read_dir(path).ok() else {
        return 0;
    };
    let mut total = 0;
    for namespace in namespaces.flatten() {
        let Some(packages) = std::fs::read_dir(namespace.path()).ok() else {
            continue;
        };
        for package in packages.flatten() {
            if let Ok(versions) = std::fs::read_dir(package.path()) {
                total += versions
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        name.starts_with(|c: char| c.is_ascii_digit()) && entry.path().is_dir()
                    })
                    .count();
            }
        }
    }
    total
}

fn count_in(path: &Option<String>) -> usize {
    path.as_ref()
        .map(|path| count_packages(Path::new(path)))
        .unwrap_or(0)
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
        count: count_in(&data) + count_in(&cache),
        downloads: settings::settings().downloads,
        downloads_active: runtime.gate.allowed(),
        data,
        cache,
        proxy: runtime.proxy.clone(),
        proxy_error: runtime.proxy_error.clone(),
    }
}
