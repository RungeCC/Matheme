//! Wolfram Language expression ↔ Typst value bridge.
//!
//! This crate is deliberately independent of both the Typst *World* and the
//! WSTP transport: it builds result expressions with an [`Expr`] newtype using
//! ordinary `From`/`Into` conversions and `as_*` accessors, and maps WL values
//! onto Typst `sys.inputs` values. Both the WXF `ExprKind::ByteArray` and the
//! WSTP wire form `System\`ByteArray[{…}]` map to Typst `bytes`, so binary data
//! can be passed from a notebook without base64.

mod expr;
mod normal;
mod typst_value;

pub use expr::{Expr, Rule};
pub use typst_value::to_typst_value;
