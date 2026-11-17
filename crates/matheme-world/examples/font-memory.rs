//! Memory probe: what does eagerly materializing the whole font collection
//! actually cost?
//!
//! Run with:
//!   cargo run --release -p matheme-world --example font-memory -- [extra dirs...]
//!
//! typst's `Font` holds the *entire* font file (`Bytes`), and
//! `typst_kit::fonts::FontPath::load` does one `fs::read` per *face*, so a
//! multi-face `.ttc` ends up resident once per face. `FontStore` itself is
//! lazy — nothing is resident until `font(index)` is called — but eagerly
//! flattening the store into a `Vec<Font>` (as the matheme font cache used to)
//! forces every file into memory.
//!
//! This example reports, per source: face counts, the resident bytes implied
//! by eager loading, how much of that is duplicate copies of multi-face
//! collections, and the top offenders.

use std::collections::HashMap;
use std::path::PathBuf;

use typst_kit::fonts::{self, FontPath, FontStore};

fn gib(bytes: u64) -> String {
    format!("{:.3} GiB", bytes as f64 / 1073741824.0)
}

/// Resident bytes implied by one `fs::read` per face.
fn report_paths(label: &str, entries: &[(FontPath, typst::text::FontInfo)]) {
    let mut by_file: HashMap<PathBuf, u64> = HashMap::new();
    let mut missing = 0usize;
    for (path, _info) in entries {
        match std::fs::metadata(&path.path) {
            Ok(meta) => *by_file.entry(path.path.clone()).or_default() += meta.len(),
            Err(_) => missing += 1,
        }
    }

    let total: u64 = by_file.values().sum();
    let unique: u64 = by_file
        .keys()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    let faces = entries.len();
    let multi = by_file.iter().filter(|(_, b)| **b > 0).count();

    println!("{label}: {faces} faces in {} files ({missing} unreadable)", by_file.len());
    println!("  eager resident bytes: {}", gib(total));
    println!("  on-disk bytes:         {} (x{:.2} duplication)", gib(unique), total as f64 / unique.max(1) as f64);

    let mut top: Vec<_> = by_file.into_iter().collect();
    top.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
    for (path, bytes) in top.iter().take(6) {
        println!("    {:>12}  {}", gib(*bytes), path.display());
    }
    let _ = multi;
}

fn main() {
    let extra: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    for dir in &extra {
        let entries: Vec<_> = fonts::scan(dir).collect();
        println!("[scanned {}]", dir.display());
        report_paths("host/custom", &entries);
    }

    let entries: Vec<_> = fonts::system().collect();
    report_paths("system", &entries);

    // Embedded fonts are already-parsed `Font` values: report their data size.
    let embedded: Vec<_> = fonts::embedded().collect();
    let bytes: usize = embedded.iter().map(|(f, _)| f.data().len()).sum();
    println!("embedded: {} faces, {} resident bytes", embedded.len(), gib(bytes as u64));

    // Control: registering faces without calling `font()` keeps memory flat.
    // Load exactly one face and show that only that file is resident.
    let mut store = FontStore::new();
    let entries: Vec<_> = fonts::system().collect();
    let count = entries.len();
    store.extend(entries);
    let face = store.font(0).expect("first system font");
    println!(
        "lazy control: {} faces registered, 1 face loaded = {} resident",
        count,
        gib(face.data().len() as u64)
    );
}
