//! Criterion benchmarks comparing per-call `MathemeWorld` construction against
//! a persistent reusable `World` (migrated from `matheme-world/examples/bench.rs`).
//!
//! Run with:
//!   cargo bench -p matheme-bench --bench world_perf

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use matheme_world::{FontCache, MathemeWorld, evict_caches, font_cache};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Duration as TypstDuration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

/// Deliberately unique multi-page document so comemo cannot memoize across runs.
fn unique_document(id: usize) -> String {
    let sections = 6 + (id % 4);
    format!(
        r#"#set page(width: 420pt, height: 320pt, margin: 18pt)
#set text(size: 10pt)
= Benchmark document {id}

#for page in range({sections}) [
  == Section #page of run {id}

  This paragraph contains inline math $integral_0^1 x^2 dif x = 1/3$ and
  enough prose to force genuine line breaking, justification and paragraph
  layout on every single run rather than a trivial single-line document.

  #for i in range(24) [
    - Row #i / #page: value #(i * i + page * 7 + {id}) with $sum_(k=0)^i k^2$.
  ]

  #align(center)[Rendered uniquely for run {id}, section #page.]
  #pagebreak()
]
"#
    )
}

/// Persistent world reusing `Library` and `FontCache` while replacing the main
/// source on every compilation.
struct ReusableWorld {
    library: LazyHash<Library>,
    fonts: Arc<FontCache>,
    state: Mutex<State>,
}

struct State {
    main: FileId,
    sources: HashMap<FileId, Source>,
    next: usize,
}

impl ReusableWorld {
    fn new() -> Self {
        Self {
            library: LazyHash::new(Library::builder().with_inputs(Dict::new()).build()),
            fonts: font_cache(),
            state: Mutex::new(State {
                main: FileId::new(RootedPath::new(
                    VirtualRoot::Project,
                    VirtualPath::new("/main.typ").unwrap(),
                )),
                sources: HashMap::new(),
                next: 0,
            }),
        }
    }

    fn set_source(&self, text: String, unique: bool) {
        let mut state = self.state.lock().unwrap();
        let name = if unique {
            state.next += 1;
            format!("/input-{}.typ", state.next)
        } else {
            "/main.typ".to_string()
        };
        let id = FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(&name).unwrap(),
        ));
        let source = Source::new(id, text);
        state.main = id;
        state.sources.clear();
        state.sources.insert(id, source);
    }
}

impl World for ReusableWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }
    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }
    fn main(&self) -> FileId {
        self.state.lock().unwrap().main
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        self.state
            .lock()
            .unwrap()
            .sources
            .get(&id)
            .cloned()
            .ok_or_else(|| FileError::Other(Some("not found".into())))
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let source = self.source(id)?;
        Ok(Bytes::from_string(source.text().to_string()))
    }
    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }
    fn today(&self, _offset: Option<TypstDuration>) -> Option<Datetime> {
        None
    }
}

fn bench_world_overhead(c: &mut Criterion) {
    let _ = font_cache().len();
    let base = PathBuf::from(".");
    let heavy_src = unique_document(0);

    let mut group = c.benchmark_group("world_setup");
    group.warm_up_time(Duration::from_millis(400));
    group.measurement_time(Duration::from_secs(2));

    group.bench_function("library_builder_build", |b| {
        b.iter(|| black_box(Library::builder().with_inputs(Dict::new()).build()));
    });

    group.bench_function("matheme_world_new_heavy_src", |b| {
        b.iter(|| {
            black_box(MathemeWorld::new(
                heavy_src.clone(),
                Dict::new(),
                base.clone(),
            ))
        });
    });

    group.finish();
}

fn bench_world_compile_strategies(c: &mut Criterion) {
    let _ = font_cache().len();
    let base = PathBuf::from(".");

    let mut group = c.benchmark_group("world_compile");
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(20);

    let mut id_fresh = 0usize;
    group.bench_function("fresh_world_unique_doc", |b| {
        b.iter(|| {
            id_fresh += 1;
            let world = MathemeWorld::new(unique_document(id_fresh), Dict::new(), base.clone());
            black_box(typst::compile::<PagedDocument>(&world))
        });
    });

    let reuse_unique = ReusableWorld::new();
    let mut id_unique = 100_000usize;
    group.bench_function("reusable_world_unique_vpath", |b| {
        b.iter(|| {
            id_unique += 1;
            reuse_unique.set_source(unique_document(id_unique), true);
            black_box(typst::compile::<PagedDocument>(&reuse_unique))
        });
    });

    let reuse_fixed = ReusableWorld::new();
    let mut id_fixed = 200_000usize;
    group.bench_function("reusable_world_fixed_vpath", |b| {
        b.iter(|| {
            id_fixed += 1;
            reuse_fixed.set_source(unique_document(id_fixed), false);
            black_box(typst::compile::<PagedDocument>(&reuse_fixed))
        });
    });

    let same_src = unique_document(999_999);
    let reuse_same = ReusableWorld::new();
    group.bench_function("reusable_world_same_source_memoized", |b| {
        b.iter(|| {
            reuse_same.set_source(same_src.clone(), false);
            black_box(typst::compile::<PagedDocument>(&reuse_same))
        });
    });

    let reuse_evict = ReusableWorld::new();
    let mut id_evict = 300_000usize;
    group.bench_function("reusable_world_fixed_vpath_evict", |b| {
        b.iter(|| {
            id_evict += 1;
            reuse_evict.set_source(unique_document(id_evict), false);
            evict_caches(0);
            black_box(typst::compile::<PagedDocument>(&reuse_evict))
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_world_overhead,
    bench_world_compile_strategies
);
criterion_main!(benches);
