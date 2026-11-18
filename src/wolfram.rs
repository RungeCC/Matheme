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

static INSTALLATION_DIR: OnceLock<PathBuf> = OnceLock::new();
static HOST_FONTS_SET: OnceLock<()> = OnceLock::new();

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

    let return_head = RawSymbol::new(Symbol::RETURN_PACKET.as_str());
    let message_head = RawSymbol::new("System`MessagePacket");
    let text_head = RawSymbol::new("System`TextPacket");

    // Drain any intermediate TextPacket/MessagePacket before ReturnPacket.
    for _ in 0..16 {
        let packet = link
            .get_expr_with_resolver(&mut |name: &str| {
                Some(RawSymbol::new(&format!("System`{name}")))
            })
            .map_err(|error| error.to_string())?;
        match packet.kind() {
            ExprKind::Normal(normal)
                if normal.has_head(&return_head) && normal.elements().len() == 1 =>
            {
                return Ok(normal.elements()[0].clone());
            }
            ExprKind::Normal(normal)
                if normal.has_head(&message_head) || normal.has_head(&text_head) =>
            {
                continue;
            }
            other => return Err(format!("expected ReturnPacket, got: {other}")),
        }
    }
    Err("exceeded intermediate WSTP packet limit before ReturnPacket".into())
}

/// Ask the kernel for `$InstallationDirectory`, caching only successful lookups
/// so a transient WSTP failure can be retried on a subsequent call.
fn installation(link: &mut Link) -> Option<&'static PathBuf> {
    if let Some(dir) = INSTALLATION_DIR.get() {
        return Some(dir);
    }
    let query = Expr::from(Symbol::INSTALLATION_DIRECTORY);
    let raw = evaluate(link, query.raw()).ok()?;
    let value = Expr::from_raw(raw);
    let dir = PathBuf::from(value.as_str()?);
    let _ = INSTALLATION_DIR.set(dir);
    INSTALLATION_DIR.get()
}

fn bundled_font_dir(root: &Path) -> PathBuf {
    root.join("SystemFiles").join("Fonts").join("TrueType")
}

/// Push the discovered directories into the Wolfram-agnostic world crate once
/// per process so steady-state calls avoid taking the settings write lock.
pub fn configure_world_fonts(link: &mut Link) {
    if HOST_FONTS_SET.get().is_some() {
        return;
    }
    if let Some(root) = installation(link) {
        matheme_world::set_host_font_paths(vec![bundled_font_dir(root)]);
        let _ = HOST_FONTS_SET.set(());
    }
}
