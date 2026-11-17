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
    cached_counts, font_cache, font_counts, reload_fonts, warmup_fonts, FontCache, FontCounts,
};
pub use packages::{
    package_status, prefetch_universe_index, reload_packages, set_call_downloads, warmup_packages,
    PackageStatus,
};
pub use settings::{
    apply, set_host_font_paths, settings, Applied, Changes, Patch, Settings, Update,
};
pub use world::{MathemeWorld, MAIN_SOURCE_NAME};

// Re-exported so the WSTP bridge can render diagnostics with the same
// codespan-based formatter that typst-cli uses.
pub use typst_kit::diagnostics::{
    emit as emit_diagnostics, termcolor, DiagnosticFormat, DiagnosticWorld,
};
