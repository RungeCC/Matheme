//! Wolfram Language expression ↔ Typst value bridge.
//!
//! This crate is deliberately independent of both the Typst *World* and the
//! WSTP transport. It exposes zero-copy wrappers over raw expressions:
//!
//! - [`Expr`] for owned values (`#[repr(transparent)]`), with `From` / `TryFrom`
//!   conversions, borrowed `as_*` accessors, and [`Expr::to_typst`] /
//!   [`Expr::from_typst`];
//! - [`Normal`] for compound `head[args…]` expressions;
//! - [`Entry`] / [`Rule`] for association elements, keeping `Rule` and
//!   `RuleDelayed` apart;
//! - [`Symbol`] for `System`` symbols.

mod convert;
mod entry;
mod expr;
mod macros;
mod normal;
mod symbol;
#[cfg(feature = "wstp")]
#[path = "wstp_helpers.rs"]
pub mod wstp;

pub use convert::{from_typst, to_typst};
pub use entry::{Entry, Rule};
pub use expr::Expr;
pub use normal::Normal;
pub use symbol::Symbol;

/// Standard subdirectory under a Wolfram installation containing bundled TrueType fonts.
pub fn wolfram_truetype_font_dir(install_dir: &std::path::Path) -> std::path::PathBuf {
    install_dir
        .join("SystemFiles")
        .join("Fonts")
        .join("TrueType")
}
