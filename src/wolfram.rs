//! Wolfram-specific discovery for the WSTP bridge.
//!
//! The world crate stays agnostic about Wolfram: this module asks the running
//! kernel for `$InstallationDirectory` and hands the resulting font directory to
//! `matheme_world::set_host_font_paths`. There is no filesystem guessing of
//! install roots, and no environment variables: the only source of truth is the
//! kernel itself.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use wolfram_library_link::{
    expr::{Expr as RawExpr, ExprKind, Symbol as RawSymbol},
    rtl,
    wstp::Link,
};

use matheme_wl::{Expr, Symbol};

static INSTALLATION_DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// Evaluate a WL expression on the bridge's WSTP link.
///
/// This mirrors `wolfram_library_link::try_evaluate`, except that the returned
/// expression is read with a symbol resolver: Wolfram 15 sends `ReturnPacket`
/// without a context, which the crate's plain `get_expr()` cannot parse.
fn evaluate(link: &mut Link, query: &RawExpr) -> Result<RawExpr, String> {
    link.put_eval_packet(query)
        .map_err(|error| format!("{error:?}"))?;

    let raw_link = unsafe { link.raw_link() };
    let processed = unsafe { rtl::processWSLINK(raw_link.cast()) };
    if processed == 0 {
        return Err(link
            .error_message()
            .unwrap_or_else(|| "unknown error processing WSTP link".into()));
    }

    let packet = link
        .get_expr_with_resolver(&mut |name: &str| Some(RawSymbol::new(&format!("System`{name}"))))
        .map_err(|error| error.to_string())?;
    match packet.kind() {
        ExprKind::Normal(return_packet)
            if return_packet.has_head(&RawSymbol::new(Symbol::RETURN_PACKET.as_str()))
                && return_packet.elements().len() == 1 =>
        {
            Ok(return_packet.elements()[0].clone())
        }
        other => Err(format!("expected ReturnPacket, got: {other}")),
    }
}

/// Ask the kernel once per process for `$InstallationDirectory`.
fn installation(link: &mut Link) -> &'static Result<PathBuf, String> {
    INSTALLATION_DIR.get_or_init(|| {
        let query = Expr::from(Symbol::INSTALLATION_DIRECTORY);
        let raw = evaluate(link, query.raw())?;
        let value = Expr::from_raw(raw);
        value
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| format!("expected a String, got: {}", value.raw()))
    })
}

fn bundled_font_dir(root: &Path) -> PathBuf {
    root.join("SystemFiles").join("Fonts").join("TrueType")
}

/// Directories holding Wolfram's bundled TrueType fonts.
///
/// The path comes from the running kernel's `$InstallationDirectory`, evaluated
/// over WSTP and cached for the process. Overriding it is a matter of patching
/// the font settings (`FontPaths`), not the environment.
pub fn font_dirs(link: &mut Link) -> Vec<PathBuf> {
    installation(link)
        .as_ref()
        .ok()
        .map(|root| vec![bundled_font_dir(root)])
        .unwrap_or_default()
}

/// Push the discovered directories into the Wolfram-agnostic world crate.
pub fn configure_world_fonts(link: &mut Link) {
    matheme_world::set_host_font_paths(font_dirs(link));
}
