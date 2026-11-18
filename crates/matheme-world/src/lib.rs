//! Typst world, fonts, package loading and configuration. This crate knows
//! nothing about Wolfram Language; the WSTP bridge lives in the root `matheme`
//! crate and only supplies host-specific paths (e.g. Wolfram's bundled fonts)
//! through [`settings::set_host_font_paths`].

mod cache;
pub mod fonts;
mod loader;
pub mod packages;
pub mod query;
pub mod settings;
pub mod world;

pub use fonts::{
    cached_counts, font_cache, font_counts, reload_fonts, reset as reset_fonts, warmup_fonts,
    FontCache, FontCounts,
};
pub use packages::{
    package_status, prefetch_universe_index, reload_packages, reset as reset_packages,
    set_call_downloads, warmup_packages, PackageStatus,
};
pub use settings::{
    apply, set_host_font_paths, settings, Applied, Changes, Patch, Settings, Update,
};
pub use world::{datetime_from_unix, MathemeWorld, WorldDate, MAIN_SOURCE_NAME};

/// Which process-wide caches to drop in [`clear_caches`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheSelection {
    pub fonts: bool,
    pub packages: bool,
    pub memoization: bool,
}

impl CacheSelection {
    pub const fn all() -> Self {
        Self {
            fonts: true,
            packages: true,
            memoization: true,
        }
    }

    pub const fn is_empty(&self) -> bool {
        !self.fonts && !self.packages && !self.memoization
    }
}

/// Drop the selected process-wide caches and return the names of the caches
/// cleared.
pub fn clear_caches(selection: CacheSelection) -> Vec<&'static str> {
    let mut cleared = Vec::with_capacity(3);
    if selection.fonts {
        reset_fonts();
        cleared.push("Fonts");
    }
    if selection.packages {
        reset_packages();
        cleared.push("Packages");
    }
    if selection.memoization {
        comemo::evict(0);
        cleared.push("Memoization");
    }
    cleared
}

/// Drop every process-wide cache: the font book and its lazily loaded faces,
/// the package loader (including the Universe index) and typst's memoized
/// compilation results (comemo). The next compilation rebuilds what it needs
/// from the current settings.
///
/// This is the host-facing "give the memory back" entry point; it is cheap to
/// call and safe to interleave with compilations.
pub fn reset_caches() {
    clear_caches(CacheSelection::all());
}

/// Evict memoized compilation results older than `max_age` generations so
/// long-lived sessions do not retain every historical compilation in memory.
pub fn evict_caches(max_age: usize) {
    comemo::evict(max_age);
}

// Re-exported so the WSTP bridge can render diagnostics with the same
// codespan-based formatter that typst-cli uses.
pub use typst_kit::diagnostics::{
    emit as emit_diagnostics, termcolor, DiagnosticFormat, DiagnosticWorld,
};
