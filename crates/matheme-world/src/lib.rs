use std::any::Any;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Duration};
use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
use typst::text::FontBook;
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::downloader::Downloader;
use typst_kit::files::{FileLoader, FileStore, FsRoot};
use typst_kit::fonts::{system, FontStore};
use typst_kit::packages::SystemPackages;

/// Immutable snapshot of the system font collection.
pub struct FontCache {
    pub book: LazyHash<FontBook>,
    pub fonts: Vec<typst::text::Font>,
}

fn scan_fonts() -> FontCache {
    let mut store = FontStore::new();
    store.extend(system());

    // FontStore keeps book indices and font indices aligned.
    let fonts: Vec<typst::text::Font> = (0..)
        .map_while(|i| store.font(i))
        .collect();

    FontCache {
        book: store.book().clone(),
        fonts,
    }
}

static FONTS: OnceLock<RwLock<Arc<FontCache>>> = OnceLock::new();

fn fonts_slot() -> &'static RwLock<Arc<FontCache>> {
    FONTS.get_or_init(|| RwLock::new(Arc::new(scan_fonts())))
}

pub fn font_cache() -> Arc<FontCache> {
    fonts_slot().read().expect("font cache lock poisoned").clone()
}

pub fn reload_fonts() -> usize {
    let next = Arc::new(scan_fonts());
    let count = next.fonts.len();
    *fonts_slot().write().expect("font cache lock poisoned") = next;
    count
}

/// WSTP downloads are opt-in: offline by default, and the gate is shared by
/// the process-wide package loader so that `AllowDownloads` is per-call.
struct GatedDownloader {
    agent: ureq::Agent,
    allowed: Arc<AtomicBool>,
}

impl Downloader for GatedDownloader {
    fn stream(&self, _key: &dyn Any, url: &str) -> io::Result<(Option<usize>, Box<dyn Read>)> {
        if !self.allowed.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "package downloads are disabled (set AllowDownloads -> True)",
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
    allowed: Arc<AtomicBool>,
}

fn build_package_runtime(allow_downloads: bool) -> PackageRuntime {
    let allowed = Arc::new(AtomicBool::new(allow_downloads));
    let mut agent = ureq::AgentBuilder::new().user_agent("matheme/0.1.0");
    // ureq's `native-tls` feature only adds an adapter; the connector must be
    // installed explicitly (Windows then uses schannel, no extra DLL).
    if let Ok(tls) = ureq::native_tls::TlsConnector::new() {
        agent = agent.tls_connector(Arc::new(tls));
    }
    // MATHEME_PROXY overrides the (sometimes broken) environment proxy.
    if let Ok(proxy) = std::env::var("MATHEME_PROXY") {
        if let Ok(proxy) = ureq::Proxy::new(proxy) {
            agent = agent.proxy(proxy);
        }
    }
    let downloader = GatedDownloader {
        agent: agent.build(),
        allowed: Arc::clone(&allowed),
    };
    PackageRuntime {
        packages: Arc::new(SystemPackages::new(downloader)),
        allowed,
    }
}

static PACKAGES: OnceLock<RwLock<Arc<PackageRuntime>>> = OnceLock::new();

fn package_slot() -> &'static RwLock<Arc<PackageRuntime>> {
    PACKAGES.get_or_init(|| RwLock::new(Arc::new(build_package_runtime(false))))
}

fn package_runtime() -> Arc<PackageRuntime> {
    package_slot()
        .read()
        .expect("package cache lock poisoned")
        .clone()
}

/// Set the download gate for the current call. Wolfram calls are serialized on
/// the kernel main thread, so a process-wide flag is sufficient.
pub fn set_downloads_allowed(allowed: bool) {
    package_runtime()
        .allowed
        .store(allowed, Ordering::Relaxed);
}

/// Rebuild the loader (drops the cached Universe index) while preserving the
/// current download gate.
pub fn reload_packages() -> PackageStatus {
    let allowed = package_runtime().allowed.load(Ordering::Relaxed);
    *package_slot()
        .write()
        .expect("package cache lock poisoned") = Arc::new(build_package_runtime(allowed));
    package_status()
}

pub struct PackageStatus {
    pub data: Option<String>,
    pub cache: Option<String>,
    pub downloads: bool,
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

pub fn package_status() -> PackageStatus {
    let runtime = package_runtime();
    let data = runtime.packages.data().map(|p| p.path().display().to_string());
    let cache = runtime.packages.cache().map(|p| p.path().display().to_string());
    let count = data
        .as_ref()
        .map(|p| count_packages(Path::new(p)))
        .unwrap_or(0)
        + cache
            .as_ref()
            .map(|p| count_packages(Path::new(p)))
            .unwrap_or(0);
    PackageStatus {
        data,
        cache,
        downloads: runtime.allowed.load(Ordering::Relaxed),
        count,
    }
}

/// Serves the in-memory main file, project files under `BasePath`, and Typst
/// packages from the standard data/cache directories (and Universe when the
/// download gate is open).
struct MathemeLoader {
    main: FileId,
    main_bytes: Vec<u8>,
    project: FsRoot,
    packages: Arc<SystemPackages>,
}

impl FileLoader for MathemeLoader {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main {
            return Ok(Bytes::new(self.main_bytes.clone()));
        }
        match id.root() {
            VirtualRoot::Project => self.project.load(id.vpath()),
            VirtualRoot::Package(spec) => {
                let root = self.packages.obtain(spec).map_err(FileError::Package)?;
                root.load(id.vpath())
            }
        }
    }
}

/// A per-call World: source, inputs and the file cache live only for this
/// compilation; fonts and the package loader are shared process-wide.
pub struct MathemeWorld {
    library: LazyHash<Library>,
    main: FileId,
    files: FileStore<MathemeLoader>,
    fonts: Arc<FontCache>,
}

impl MathemeWorld {
    pub fn new(source_text: String, inputs: Dict, base_path: PathBuf) -> Self {
        let main = FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("/main.typ").expect("valid virtual path"),
        ));
        let runtime = package_runtime();
        let loader = MathemeLoader {
            main,
            main_bytes: source_text.into_bytes(),
            project: FsRoot::new(base_path),
            packages: Arc::clone(&runtime.packages),
        };

        Self {
            library: LazyHash::new(Library::builder().with_inputs(inputs).build()),
            main,
            files: FileStore::new(loader),
            fonts: font_cache(),
        }
    }
}

impl World for MathemeWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.fonts.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<typst::syntax::Source> {
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<typst::text::Font> {
        self.fonts.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}
