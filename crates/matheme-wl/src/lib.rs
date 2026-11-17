//! Wolfram Language expression ↔ Typst value bridge.
//!
//! This crate is deliberately independent of both the Typst *World* and the
//! WSTP transport. It exposes wrappers instead of raw expression types:
//!
//! - [`Expr`] for owned values, with `From` constructors and `as_*` accessors;
//! - [`Normal`] for compound `head[args…]` expressions;
//! - [`Entry`] / [`Rule`] for association elements, keeping `Rule` and
//!   `RuleDelayed` apart;
//! - [`Symbol`] for the `System`` symbols the bridge cares about.
//!
//! Both the WXF `ExprKind::ByteArray` and the WSTP wire form
//! `System`ByteArray[{…}]` map to Typst `bytes`, so binary data can be passed
//! from a notebook without base64.

mod entry;
mod expr;
mod normal;
mod symbol;
mod typst_value;

pub use entry::{Entry, Rule};
pub use expr::Expr;
pub use normal::Normal;
pub use symbol::Symbol;
pub use typst_value::to_typst_value;
