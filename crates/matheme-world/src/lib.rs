//! Typst world, fonts and package loading. This crate knows nothing about
//! Wolfram Language; the WSTP bridge lives in the root `matheme` crate.

pub mod fonts;
pub mod loader;
pub mod packages;
pub mod world;

pub use fonts::{font_cache, reload_fonts, FontCache};
pub use packages::{package_status, reload_packages, set_downloads_allowed, PackageStatus};
pub use world::{MathemeWorld, MAIN_SOURCE_NAME};

// Re-exported so the WSTP bridge can render diagnostics with the same
// codespan-based formatter that typst-cli uses.
pub use typst_kit::diagnostics::{
    emit as emit_diagnostics, termcolor, DiagnosticFormat, DiagnosticWorld,
};
