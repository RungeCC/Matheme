//! WSTP bridge between the Wolfram Language package and `matheme-world`.
//!
//! The crate exports one link function. [`call::serve`] reads a single
//! `<|"Action" -> name, "Arguments" -> <|…|>|>` envelope, dispatches it to an
//! [`call::Endpoint`], and writes back the uniform result association.

mod cache;
mod call;
mod compile;
mod diagnostics;
mod query;
mod settings;
mod version;
mod wolfram;

use matheme_wl::Entry;
use wolfram_library_link::{export, wstp::Link};

use cache::{ClearCache, Close, ReloadFonts, ReloadPackages, Warmup};
use call::{Answer, Endpoint};
use compile::Compile;
use query::{QueryFonts, QueryPackages};
use settings::{ResetSettings, SetSettings, Settings};
use version::Version;

#[export(wstp)]
fn matheme_call(link: &mut Link) {
    call::serve(link, dispatch);
}

/// The single source of truth for action names. Every entry must have a
/// matching `$displayNames` key in `Kernel/Matheme.wl`.
fn dispatch(link: &mut Link, action: &str, args: &[Entry<'_>]) -> Answer {
    macro_rules! endpoint {
        ($endpoint:ty) => {{
            debug_assert_eq!(<$endpoint as Endpoint>::NAME, action);
            call::invoke::<$endpoint>(link, args)
        }};
    }
    match action {
        "compile" => endpoint!(Compile),
        "version" => endpoint!(Version),
        "settings" => endpoint!(Settings),
        "setSettings" => endpoint!(SetSettings),
        "resetSettings" => endpoint!(ResetSettings),
        "warmup" => endpoint!(Warmup),
        "reloadFonts" => endpoint!(ReloadFonts),
        "reloadPackages" => endpoint!(ReloadPackages),
        "queryFonts" => endpoint!(QueryFonts),
        "queryPackages" => endpoint!(QueryPackages),
        "clearCache" => endpoint!(ClearCache),
        "close" => endpoint!(Close),
        other => Answer::Failure(call::error(format!("unknown action: {other}"))),
    }
}
