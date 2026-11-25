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
//! - [`Symbol`] for `` System` `` symbols.

mod convert;
pub mod decode;
mod entry;
mod expr;
mod macros;
mod normal;
mod symbol;

pub use convert::{from_typst, to_typst};
pub use entry::{Entry, Rule};
pub use expr::Expr;
pub use normal::Normal;
pub use symbol::{Symbol, fundamental, graphics};
