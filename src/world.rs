use std::sync::{Arc, OnceLock, RwLock};

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::FontBook;
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::{system, FontStore};

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

/// A per-call World: source and inputs live only for this compilation.
pub struct MathemeWorld {
    library: LazyHash<Library>,
    main: FileId,
    source_text: String,
    main_source: Source,
    fonts: Arc<FontCache>,
}

impl MathemeWorld {
    pub fn new(source_text: String, inputs: Dict) -> Self {
        let main = FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("/main.typ").expect("valid virtual path"),
        ));
        let main_source = Source::new(main, source_text.clone());

        Self {
            library: LazyHash::new(Library::builder().with_inputs(inputs).build()),
            main,
            source_text,
            main_source,
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

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main {
            Ok(self.main_source.clone())
        } else {
            Err(FileError::Other(None))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main {
            Ok(Bytes::new(self.source_text.as_bytes().to_vec()))
        } else {
            Err(FileError::Other(None))
        }
    }

    fn font(&self, index: usize) -> Option<typst::text::Font> {
        self.fonts.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}
