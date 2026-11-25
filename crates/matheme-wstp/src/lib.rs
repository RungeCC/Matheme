//! Shared WSTP link wrapper, persistent [`KernelSession`], WSTP protocol
//! symbols, and Wolfram installation helpers for both the LibraryLink bridge
//! (`matheme`) and the standalone CLI (`matheme-cli`).
//!
//! Keeping all WSTP transport code in this crate preserves `matheme-wl`'s
//! guarantee of having zero WSTP / native-library dependency.

mod error;
pub mod install;
pub mod kernel;
mod link;
pub(crate) mod protocol;

pub use error::WstpError;
pub use kernel::{KernelError, KernelSession};
pub use link::{EvalOptions, EvalOutput, WstpLink, resolve_unqualified_symbol};
pub use wstp::Link;

pub use install::wolfram_truetype_font_dir;
