//! Typst vector graphics extraction IR and optional Wolfram Language
//! ``System`Graphics`` encoding backend.
//!
//! - [`extract`] is host-agnostic: it walks a Typst `PagedDocument` and emits
//!   [`VectorExport`] / [`VectorPage`] IR without depending on Wolfram Language.
//! - [`wl`] (behind the `wl` Cargo feature, enabled by default) encodes
//!   [`VectorPage`]s into ``System`Graphics`` [`matheme_wl::Expr`]s.

pub mod extract;
mod lru;
mod winding;
#[cfg(feature = "wl")]
pub mod wl;

pub use extract::{
    cache_generation, clear_glyph_cache, export_vector, DegradationWarning, EmbeddedImageFormat,
    GlyphInstances, GlyphKey, LineCapStyle, LineJoinStyle, Rgba, Segment, StrokeStyle, Subpath,
    UnitGlyphKey, VectorExport, VectorPage, VectorPrimitive, WindingRule,
};
#[cfg(feature = "wl")]
pub use wl::{page_to_expr, pages_to_expr};

/// Combined Wolfram Language vector `Graphics` export plus any
/// fidelity-degradation warnings.
#[cfg(feature = "wl")]
#[derive(Debug, Clone)]
pub struct GraphicsExport {
    pub graphics: matheme_wl::Expr,
    pub warnings: Vec<DegradationWarning>,
}

/// Extract selected pages of a [`typst_layout::PagedDocument`] and encode them
/// directly as a Wolfram Language list of ``System`Graphics`` expressions.
#[cfg(feature = "wl")]
pub fn export_graphics(
    document: &typst_layout::PagedDocument,
    pages: Option<&typst::layout::PageRanges>,
    font_generation: u64,
) -> GraphicsExport {
    let export = export_vector(document, pages, font_generation);
    GraphicsExport {
        graphics: wl::pages_to_expr(&export.pages),
        warnings: export.warnings,
    }
}

/// Drop all cached unit/scaled glyph outlines and (when the `wl` feature is
/// enabled) encoded Wolfram `Graphics` expressions, advancing
/// [`cache_generation`].
pub fn clear_caches() {
    extract::clear_glyph_cache();
    #[cfg(feature = "wl")]
    wl::clear_expr_cache();
}
