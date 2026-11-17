//! Wolfram-specific discovery for the WSTP bridge.
//!
//! The world crate stays agnostic about Wolfram: this module asks the running
//! kernel for `$InstallationDirectory` and hands the resulting font directory
//! to `matheme_world::set_extra_font_dirs`. No filesystem guessing of install
//! roots happens in the world crate.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use wolfram_library_link::{
    expr::{Expr as RawExpr, ExprKind, Symbol},
    rtl,
    wstp::Link,
};

use matheme_wl::Expr;

const FONT_DIRS_VAR: &str = "MATHEME_WOLFRAM_FONTS";
const INSTALL_DIR_VAR: &str = "MATHEME_WOLFRAM_DIR";

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
        .get_expr_with_resolver(&mut |name: &str| Some(Symbol::new(&format!("System`{name}"))))
        .map_err(|error| error.to_string())?;
    match packet.kind() {
        ExprKind::Normal(return_packet)
            if return_packet.has_head(&Symbol::new("System`ReturnPacket"))
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
        let query = Expr::symbol("System`$InstallationDirectory");
        let raw = evaluate(link, query.raw())?;
        let value = Expr::from_raw(raw);
        value
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| format!("expected a String, got: {}", value.raw()))
    })
}

/// Why kernel-based font-directory discovery failed, if it did.
pub fn installation_error(link: &mut Link) -> Option<&'static str> {
    installation(link).as_ref().err().map(String::as_str)
}

fn bundled_font_dir(root: &Path) -> PathBuf {
    root.join("SystemFiles").join("Fonts").join("TrueType")
}

/// Directories holding Wolfram's bundled TrueType fonts.
///
/// 1. `MATHEME_WOLFRAM_FONTS` overrides with an explicit path list; an empty
///    value disables bundled fonts.
/// 2. `MATHEME_WOLFRAM_DIR` overrides the installation root.
/// 3. Otherwise the running kernel's `$InstallationDirectory` is evaluated
///    over WSTP (cached for the process).
pub fn font_dirs(link: &mut Link) -> Vec<PathBuf> {
    if let Some(paths) = std::env::var_os(FONT_DIRS_VAR) {
        return std::env::split_paths(&paths)
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
    }
    if let Some(root) = std::env::var_os(INSTALL_DIR_VAR) {
        if !root.is_empty() {
            return vec![bundled_font_dir(Path::new(&root))];
        }
    }
    installation(link)
        .as_ref()
        .ok()
        .map(|root| vec![bundled_font_dir(root)])
        .unwrap_or_default()
}

/// Push the discovered directories into the Wolfram-agnostic world crate.
pub fn configure_world_fonts(link: &mut Link) {
    matheme_world::set_extra_font_dirs(font_dirs(link));
}

/// Number of font files in the bundled directories (status reporting only;
/// the real number of faces loaded is returned by `warmup_fonts()`).
pub fn font_file_count(dirs: &[PathBuf]) -> usize {
    dirs.iter()
        .map(|dir| {
            std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| {
                            entry
                                .path()
                                .extension()
                                .and_then(OsStr::to_str)
                                .is_some_and(|ext| {
                                    matches!(
                                        ext.to_ascii_lowercase().as_str(),
                                        "ttf" | "otf" | "ttc" | "otc"
                                    )
                                })
                        })
                        .count()
                })
                .unwrap_or(0)
        })
        .sum()
}
