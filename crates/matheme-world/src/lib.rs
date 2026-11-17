//! Typst world, fonts and package loading. This crate knows nothing about
//! Wolfram Language; the WSTP bridge lives in the root `matheme` crate.

pub mod fonts;
pub mod loader;
pub mod packages;
pub mod world;

pub use fonts::{
    extra_font_dirs, font_cache, reload_fonts, set_extra_font_dirs, warmup_fonts, FontCache,
};
pub use packages::{
    package_status, prefetch_universe_index, reload_packages, set_downloads_allowed,
    warmup_packages, PackageStatus,
};
pub use world::{MathemeWorld, MAIN_SOURCE_NAME};

// Re-exported so the WSTP bridge can render diagnostics with the same
// codespan-based formatter that typst-cli uses.
pub use typst_kit::diagnostics::{
    emit as emit_diagnostics, termcolor, DiagnosticFormat, DiagnosticWorld,
};
