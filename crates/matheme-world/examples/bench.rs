//! Micro-benchmark comparing per-call World construction against a persistent
//! World with a fresh, uniquely-named virtual file on every call.
//!
//! Run with: cargo run --release -p matheme-world --example bench
//!
//! Every document is made unique (a monotonically increasing id is embedded in
//! the source) so comemo's memoization cannot make later iterations
//! artificially cheap. This measures cold-cache work, which is the relevant
//! case for Wolfram calls that compile different source each time.

use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use matheme_world::{font_cache, FontCache, MathemeWorld};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Duration as TypstDuration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

const ITERATIONS: usize = 60;
const WARMUP: usize = 10;

/// A deliberately heavy document: many pages, math and layout work, so that
/// per-call timings are large relative to clock noise. An embedded id makes
/// every document's content unique, preventing comemo from memoizing the main
/// source across iterations.
fn document(id: usize) -> String {
    let sections = 6 + (id % 4); // varying page count makes stale results detectable
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

/// A reusable world: the expensive, stable pieces (Library and font book) are
/// built once; each call installs a fresh source with a configurable file name.
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

    /// Install a new source. `unique` selects a fresh virtual path per call
    /// (which grows Typst's global path interner) versus reusing one path.
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

fn time<F: FnMut(usize)>(label: &str, ids: &[usize], mut f: F) -> Duration {
    for &id in &ids[..WARMUP] {
        f(id);
    }
    let start = Instant::now();
    for &id in &ids[WARMUP..] {
        f(id);
    }
    let elapsed = start.elapsed();
    let per = elapsed / (ids.len() - WARMUP) as u32;
    println!(
        "{label:<34} total {:>8.2?}   per call {:>7.3?}",
        elapsed.as_secs_f64(),
        per.as_secs_f64()
    );
    elapsed
}

/// Measure a zero-argument closure repeated N times, for very cheap operations.
fn time_many<F: FnMut()>(label: &str, n: usize, mut f: F) {
    let start = Instant::now();
    for _ in 0..n {
        f();
    }
    let elapsed = start.elapsed();
    println!(
        "{label:<34} total {:>8.2?}   per call {:>9.4?}  (n={n})",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / n as f64
    );
}

fn main() {
    // Font scan is process-wide and cached; do it once before timing, exactly
    // as MathemeWarmup[] would in the real application.
    println!("fonts in cache: {}", font_cache().len());

    // Sanity check: confirm the heavy document actually compiles to multiple
    // pages, so that all timings below measure a successful compilation and
    // not a fast error path. Also verify that a persistent world with a fixed
    // file name does NOT return stale content (a comemo correctness trap).
    {
        let world = MathemeWorld::new(document(1), Dict::new(), PathBuf::from("."));
        let result = typst::compile::<PagedDocument>(&world);
        match result.output {
            Ok(doc) => println!("sanity: compiled OK, pages = {}", doc.pages().len()),
            Err(errors) => println!("sanity: FAILED with {} diagnostics", errors.len()),
        }
    }
    {
        // Feed two structurally DIFFERENT documents (different page counts)
        // into the same persistent world with a FIXED name. If comemo returns
        // stale cached content, the second result keeps the first's page count.
        let world = ReusableWorld::new();
        let short = "= Short\n#pagebreak()\none".to_string();
        let long = "= Long\n#pagebreak()\na\n#pagebreak()\nb\n#pagebreak()\nc".to_string();
        let mut fingerprints = Vec::new();
        for text in [&short, &long] {
            world.set_source(text.clone(), false);
            let result = typst::compile::<PagedDocument>(&world);
            if let Ok(doc) = result.output {
                fingerprints.push(doc.pages().len());
            } else {
                fingerprints.push(0);
            }
        }
        println!("sanity: fixed-name world page counts (expect [2, 4]) = {fingerprints:?}");
    }

    let ids: Vec<usize> = (0..ITERATIONS + WARMUP).collect();
    let base = PathBuf::from(".");

    // 1. Isolated cost of rebuilding the standard library's global scope.
    time_many("Library::builder().build() only", 2000, || {
        let lib = Library::builder().with_inputs(Dict::new()).build();
        black_box(lib);
    });

    // 2. Isolated cost of MathemeWorld::new (library build + loader + source).
    let heavy_doc = document(0);
    time_many("MathemeWorld::new only (heavy src)", 500, || {
        let world = MathemeWorld::new(heavy_doc.clone(), Dict::new(), base.clone());
        black_box(world);
    });

    let fresh_pages: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
    let unique_pages: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
    let fixed_pages: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());

    // 3. Current strategy: a brand new World for every compilation.
    let fresh = time("compile / fresh World (current)", &ids, |id| {
        let world = MathemeWorld::new(document(id), Dict::new(), base.clone());
        let result = typst::compile::<PagedDocument>(&world);
        if let Ok(doc) = &result.output {
            fresh_pages.lock().unwrap().push((id, doc.pages().len()));
        }
        black_box(result);
    });

    // 4. Persistent World, one unique virtual file name per call.
    let reuse_unique = {
        let world = ReusableWorld::new();
        time("compile / reuse World + unique name", &ids, |id| {
            world.set_source(document(id), true);
            let result = typst::compile::<PagedDocument>(&world);
            if let Ok(doc) = &result.output {
                unique_pages.lock().unwrap().push((id, doc.pages().len()));
            }
            black_box(result);
        })
    };

    // 5. Persistent World, reusing a single fixed file name.
    let reuse_fixed = {
        let world = ReusableWorld::new();
        time("compile / reuse World + fixed name", &ids, |id| {
            world.set_source(document(id), false);
            let result = typst::compile::<PagedDocument>(&world);
            if let Ok(doc) = &result.output {
                fixed_pages.lock().unwrap().push((id, doc.pages().len()));
            }
            black_box(result);
        })
    };

    // 6. Persistent World compiling the SAME document repeatedly: this is
    //    where comemo's memoization shines and shows the fixed-name speedup's
    //    true source (identical input cache hit, not the World being cheaper).
    let same = document(9999);
    let reuse_same = {
        let world = ReusableWorld::new();
        time_many("compile / reuse World + SAME source", 200, || {
            world.set_source(same.clone(), false);
            let result = typst::compile::<PagedDocument>(&world);
            black_box(result);
        })
    };
    let _ = reuse_same;

    // 7. Fixed-name World but evict comemo's memo cache before every compile.
    //    If this returns to ~6ms, the fixed-name speedup is purely memoization.
    let reuse_fixed_evict = {
        let world = ReusableWorld::new();
        time("compile / reuse fixed + evict cache", &ids, |id| {
            world.set_source(document(id), false);
            comemo::evict(0);
            let result = typst::compile::<PagedDocument>(&world);
            black_box(result);
        })
    };

    println!("\n--- output correctness (page counts must match fresh World) ---");
    let fp = fresh_pages.lock().unwrap();
    let up = unique_pages.lock().unwrap();
    let fx = fixed_pages.lock().unwrap();
    let unique_equal = *fp == *up;
    let fixed_equal = *fp == *fx;
    println!(
        "fresh (id,pages) first 8        : {:?}",
        &fp[..fp.len().min(8)]
    );
    println!(
        "unique (id,pages) first 8       : {:?}",
        &up[..up.len().min(8)]
    );
    println!(
        "fixed (id,pages) first 8        : {:?}",
        &fx[..fx.len().min(8)]
    );
    println!("unique-name output == fresh     : {unique_equal}");
    println!("fixed-name  output == fresh     : {fixed_equal}");
    println!("\n--- why fixed-name is fast ---");
    println!(
        "reuse fixed + evict per call: {:.3?}/call (vs fresh ~5ms)",
        reuse_fixed_evict.as_secs_f64() / (ids.len() - WARMUP) as f64
    );

    println!("\n--- ratios (lower is better) ---");
    let f = fresh.as_secs_f64();
    println!(
        "reuse unique / fresh : {:.3}x  ({:+.1}% vs fresh)",
        reuse_unique.as_secs_f64() / f,
        (reuse_unique.as_secs_f64() / f - 1.0) * 100.0
    );
    println!(
        "reuse fixed  / fresh : {:.3}x  ({:+.1}% vs fresh)",
        reuse_fixed.as_secs_f64() / f,
        (reuse_fixed.as_secs_f64() / f - 1.0) * 100.0
    );
}
