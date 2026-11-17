//! Per-compilation Typst world.

use std::path::PathBuf;

use typst::diag::{FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::diagnostics::DiagnosticWorld;
use typst_kit::files::FileStore;

use crate::fonts::{font_cache, FontCache};
use crate::loader::MathemeLoader;

/// Name shown for the in-memory source string passed from Wolfram.
pub const MAIN_SOURCE_NAME: &str = "<typst source>";

/// A per-call World: source, inputs and the file cache live only for this
/// compilation; fonts and the package loader are shared process-wide.
pub struct MathemeWorld {
    library: LazyHash<Library>,
    main: FileId,
    files: FileStore<MathemeLoader>,
    fonts: std::sync::Arc<FontCache>,
}

impl MathemeWorld {
    pub fn new(source_text: String, inputs: Dict, base_path: PathBuf) -> Self {
        let main = FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("/main.typ").expect("valid virtual path"),
        ));
        let loader = MathemeLoader::new(main, source_text, base_path);

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
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

/// File naming for diagnostics, following `typst-cli`'s strategy:
/// project files are relative to the project root, packages are shown as
/// `@namespace/name:version/path`, and the in-memory main file gets a stable
/// synthetic name instead of a meaningless virtual path.
impl DiagnosticWorld for MathemeWorld {
    fn name(&self, id: FileId) -> String {
        if id == self.main {
            return MAIN_SOURCE_NAME.into();
        }

        match id.root() {
            VirtualRoot::Project => self
                .files
                .loader()
                .resolve(id)
                .and_then(|path| {
                    path.strip_prefix(self.files.loader().project.path())
                        .ok()
                        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                })
                .unwrap_or_else(|| id.vpath().get_without_slash().into()),
            VirtualRoot::Package(spec) => format!("{spec}{}", id.vpath().get_with_slash()),
        }
    }
}
