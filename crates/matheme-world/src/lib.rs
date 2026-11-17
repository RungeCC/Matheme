//! Typst world, fonts, package loading and configuration. This crate knows
//! nothing about Wolfram Language; the WSTP bridge lives in the root `matheme`
//! crate and only supplies host-specific paths (e.g. Wolfram's bundled fonts)
//! through [`settings::set_host_font_paths`].

pub mod fonts;
pub mod loader;
pub mod packages;
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
pub use world::{MathemeWorld, MAIN_SOURCE_NAME};

/// Drop every process-wide cache: the font book and its lazily loaded faces,
/// the package loader (including the Universe index) and typst's memoized
/// compilation results (comemo). The next compilation rebuilds what it needs
/// from the current settings.
///
/// This is the host-facing "give the memory back" entry point; it is cheap to
/// call and safe to interleave with compilations.
pub fn reset_caches() {
    reset_fonts();
    reset_packages();
    comemo::evict(0);
}

// Re-exported so the WSTP bridge can render diagnostics with the same
// codespan-based formatter that typst-cli uses.
pub use typst_kit::diagnostics::{
    emit as emit_diagnostics, termcolor, DiagnosticFormat, DiagnosticWorld,
};
