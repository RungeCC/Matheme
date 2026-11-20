//! Host-agnostic Typst vector graphics extraction and Wolfram Language
//! `System\`Graphics` encoding backend.
//!
//! Exports [`extract`] (Typst `PagedDocument` -> host-neutral [`VectorExport`] IR),
//! [`wl`] ([`VectorPage`] -> `System\`Graphics` [`matheme_wl::Expr`]), and
//! [`export_graphics`] combining both stages.

pub mod extract;
mod winding;
pub mod wl;

pub use extract::{
    cache_generation, clear_glyph_cache, export_vector, DegradationWarning, EmbeddedImageFormat,
    GlyphInstances, GlyphKey, LineCapStyle, LineJoinStyle, Rgba, Segment, StrokeStyle, Subpath,
    UnitGlyphKey, VectorExport, VectorPage, VectorPrimitive, WindingRule,
};
pub use wl::{page_to_expr, pages_to_expr};

/// Combined Wolfram Language vector `Graphics` export plus any
/// fidelity-degradation warnings.
#[derive(Debug, Clone)]
pub struct GraphicsExport {
    pub graphics: matheme_wl::Expr,
    pub warnings: Vec<DegradationWarning>,
}

/// Extract selected pages of a [`typst_layout::PagedDocument`] and encode them
/// directly as a Wolfram Language list of `System\`Graphics` expressions.
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

/// Drop all cached unit glyph outlines and encoded Wolfram `Graphics`
/// expressions, advancing [`cache_generation`].
pub fn clear_caches() {
    extract::clear_glyph_cache();
    wl::clear_expr_cache();
}
