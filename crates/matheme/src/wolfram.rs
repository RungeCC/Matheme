//! Wolfram-specific discovery and runtime library metadata.
//!
//! The world crate stays agnostic about Wolfram: this module asks the running
//! kernel for `$InstallationDirectory` and hands the resulting font directory to
//! `matheme_world::set_host_font_paths`, and inspects the loaded LibraryLink /
//! WSTP runtime for [`crate::version::Version`].

#[cfg(not(test))]
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::OnceLock;

use wolfram_library_link::{
    rtl,
    wstp::{sys as wstp_sys, Link},
};

use matheme_wl::Expr;
use matheme_wstp::{install, EvalOptions, WstpError, WstpLink};

static INSTALLATION_DIR: OnceLock<PathBuf> = OnceLock::new();
static HOST_FONTS_SET: OnceLock<()> = OnceLock::new();

/// Runtime metadata for the loaded Matheme DLL, LibraryLink and WSTP layer.
#[derive(Debug, Clone)]
pub struct DllInfo {
    pub target: String,
    pub library_link_compiled: i64,
    pub library_link_runtime: Option<i64>,
    pub wstp_protocol: i64,
    pub wstp_compiled_interface: i64,
    pub wstp_compiled_revision: i64,
    pub wstp_runtime: Option<(i64, i64, i64)>,
}

/// Query runtime version numbers from LibraryLink and WSTP. Returns compile-time
/// metadata with `None` runtime fields when called outside a Wolfram Kernel
/// (e.g. unit tests).
pub fn dll_info() -> DllInfo {
    #[cfg(test)]
    let library_data: Option<wolfram_library_link::sys::WolframLibraryData> = None;
    #[cfg(not(test))]
    let library_data = std::panic::catch_unwind(wolfram_library_link::get_library_data).ok();
    #[cfg(test)]
    let library_link_runtime: Option<i64> = library_data.map(|_| 0);
    #[cfg(not(test))]
    let library_link_runtime = library_data.map(|data| unsafe { (*data.raw_library_data).VersionNumber });
    #[cfg(test)]
    let wstp_runtime: Option<(i64, i64, i64)> = None;
    #[cfg(not(test))]
    let wstp_runtime = library_data.and_then(|data| unsafe {
        let env = rtl::getWSLINKEnvironment(data.raw_library_data);
        if env.is_null() {
            return None;
        }
        let mut interface: c_int = 0;
        let mut revision: c_int = 0;
        let mut build: c_int = 0;
        wstp_sys::WSVersionNumbers(env.cast(), &mut interface, &mut revision, &mut build);
        Some((i64::from(interface), i64::from(revision), i64::from(build)))
    });

    DllInfo {
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        library_link_compiled: i64::from(wolfram_library_link::sys::WolframLibraryVersion),
        library_link_runtime,
        wstp_protocol: i64::from(wstp_sys::WSVERSION),
        wstp_compiled_interface: i64::from(wstp_sys::WSINTERFACE),
        wstp_compiled_revision: i64::from(wstp_sys::WSREVISION),
        wstp_runtime,
    }
}

/// Evaluate a WL expression on the bridge's WSTP link via [`WstpLink::eval_with`].
///
/// This mirrors `wolfram_library_link::try_evaluate`, except that the returned
/// expression is read with a context-resolving reader and unified timeout/resync
/// handling inside [`WstpLink::eval_with`].
fn evaluate(link: &mut Link, query: &Expr) -> Result<Expr, WstpError> {
    WstpLink::new(link)
        .eval_with(query, EvalOptions::library_link(), |raw_link| {
            let processed = unsafe { rtl::processWSLINK(raw_link.raw_link().cast()) };
            if processed == 0 {
                Err(WstpError::transport(raw_link.error_message().unwrap_or_else(
                    || "unknown error processing WSTP link".into(),
                )))
            } else {
                Ok(())
            }
        })
        .map(|out| out.expr)
}

/// Ask the kernel for `$InstallationDirectory`, caching only successful lookups
/// so a transient WSTP failure can be retried on a subsequent call.
fn installation(link: &mut Link) -> Result<&'static PathBuf, String> {
    if let Some(dir) = INSTALLATION_DIR.get() {
        return Ok(dir);
    }
    let query = Expr::from(install::INSTALLATION_DIRECTORY);
    let value = evaluate(link, &query)
        .map_err(|err| format!("failed to query $InstallationDirectory for host fonts: {err}"))?;
    let dir_str = value
        .as_str()
        .ok_or_else(|| format!("$InstallationDirectory returned non-string expression: {value}"))?;
    let _ = INSTALLATION_DIR.set(PathBuf::from(dir_str));
    INSTALLATION_DIR
        .get()
        .ok_or_else(|| "failed to cache $InstallationDirectory".to_owned())
}

/// Push the discovered directories into the Wolfram-agnostic world crate once
/// per process so steady-state calls avoid taking the settings write lock.
///
/// Returns an optional warning message if host font discovery failed so callers
/// can surface it as a diagnostic instead of silently degrading fonts.
pub fn configure_world_fonts(link: &mut Link) -> Option<String> {
    if HOST_FONTS_SET.get().is_some() {
        return None;
    }
    match installation(link) {
        Ok(root) => {
            matheme_world::set_host_font_paths(vec![matheme_wstp::wolfram_truetype_font_dir(root)]);
            let _ = HOST_FONTS_SET.set(());
            None
        }
        Err(warning) => Some(warning),
    }
}
