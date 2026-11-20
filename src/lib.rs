//! WSTP bridge between the Wolfram Language package and `matheme-world`.
//!
//! The crate exports one link function. [`call::serve`] reads a single
//! `<|"Action" -> name, "Arguments" -> <|…|>|>` envelope, parses it into an
//! [`Action`] via the declarative [`wl_actions!`] schema, and writes back the
//! uniform result association.

mod args;
mod cache;
mod call;
mod compile;
mod diagnostics;
mod graphics;
mod query;
mod settings;
mod version;
mod wolfram;

use wolfram_library_link::{export, wstp::Link};

#[export(wstp)]
fn matheme_call(link: &mut Link) {
    call::serve(link);
}

// The single source of truth for action names, host-font requirements,
// argument schemas, and handlers. Every entry must have a matching
// `$displayNames` key in `Kernel/Matheme.wl`.
wl_actions! {
    pub enum Action {
        "compile" => Compile(compile::CompileArgs) => compile::run,
        "version" [no_host_fonts] => Version => version::run,
        "settings" [no_host_fonts] => Settings => settings::run_get,
        "setSettings" => SetSettings(settings::SettingsPatchArgs) => settings::run_set,
        "resetSettings" => ResetSettings => settings::run_reset,
        "warmup" => Warmup(cache::WarmupArgs) => cache::run_warmup,
        "reloadFonts" => ReloadFonts => cache::run_reload_fonts,
        "reloadPackages" [no_host_fonts] => ReloadPackages => cache::run_reload_packages,
        "queryFonts" => QueryFonts(query::FontQueryArgs) => query::run_fonts,
        "queryPackages" [no_host_fonts] => QueryPackages(query::PackageQueryArgs) => query::run_packages,
        "clearCache" [no_host_fonts] => ClearCache(cache::ClearCacheArgs) => cache::run_clear,
        "close" [no_host_fonts] => Close => cache::run_close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_action_returns_call_failure() {
        let err = Action::parse("nonexistent", &[])
            .err()
            .expect("unknown action");
        assert_eq!(err.tag, "Call");
        assert!(err.message.contains("unknown action"), "{}", err.message);
    }

    #[test]
    fn unit_actions_reject_unexpected_arguments() {
        call::with_entries([matheme_wl::Rule::new("Extra", 1_i64)], |args| {
            let err = Action::parse("version", args)
                .err()
                .expect("unexpected args");
            assert_eq!(err.tag, "Call");
            assert!(
                err.message.contains("takes no arguments"),
                "{}",
                err.message
            );
        });
    }

    #[test]
    fn host_font_flags_match_expectations() {
        assert!(!Action::parse("version", &[]).unwrap().needs_host_fonts());
        assert!(!Action::parse("settings", &[]).unwrap().needs_host_fonts());
        assert!(!Action::parse("close", &[]).unwrap().needs_host_fonts());
        assert!(Action::parse("warmup", &[]).unwrap().needs_host_fonts());
        assert!(Action::parse("queryFonts", &[]).unwrap().needs_host_fonts());
        assert!(!Action::parse("queryPackages", &[]).unwrap().needs_host_fonts());
    }
}
