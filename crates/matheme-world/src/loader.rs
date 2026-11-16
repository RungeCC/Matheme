//! Resolves Typst file IDs to the in-memory main source, project files and
//! Typst Universe packages.

use std::path::PathBuf;
use std::sync::Arc;

use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
use typst::syntax::{FileId, VirtualRoot};
use typst_kit::files::{FileLoader, FsRoot};
use typst_kit::packages::SystemPackages;

use crate::packages::package_runtime;

pub struct MathemeLoader {
    pub main: FileId,
    pub main_bytes: Vec<u8>,
    pub project: FsRoot,
    pub packages: Arc<SystemPackages>,
}

impl MathemeLoader {
    pub fn new(main: FileId, source_text: String, base_path: PathBuf) -> Self {
        Self {
            main,
            main_bytes: source_text.into_bytes(),
            project: FsRoot::new(base_path),
            packages: Arc::clone(&package_runtime().packages),
        }
    }

    /// Resolve a file ID to a disk path, for human-readable diagnostic names.
    /// The in-memory main file has no disk path and returns `None`.
    pub fn resolve(&self, id: FileId) -> Option<PathBuf> {
        if id == self.main {
            return None;
        }
        match id.root() {
            VirtualRoot::Project => self.project.resolve(id.vpath()).ok(),
            VirtualRoot::Package(spec) => self
                .packages
                .obtain(spec)
                .ok()
                .and_then(|root| root.resolve(id.vpath()).ok()),
        }
    }
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
