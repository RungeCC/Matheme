//! Cache lifecycle endpoints: `ClearMathemeCache`, `MathemeWarmup`,
//! `MathemeReloadFonts`, `MathemeReloadPackages`, and `MathemeClose`.

use matheme_wl::{Expr, Rule, Symbol, rule, rules};
use matheme_world::{self as world, CacheSelection};

use crate::args::schema;
use crate::call::{Answer, Outcome};
use crate::diagnostics::DiagnosticSource;
use crate::settings;
use crate::wl_args;
use matheme_wl::decode::{self, Decoder};
use serde::Deserialize;

pub(crate) fn package_entries(status: &world::PackageStatus) -> Vec<Rule> {
    rules! [
        "Packages" -> status.count as i64,
        "PackageData" -> status.data.as_deref(),
        "PackageCache" -> status.cache.as_deref(),
        "DownloadsAllowed" -> status.downloads,
        "DownloadsActive" -> status.downloads_active,
        "Proxy" -> status.proxy.as_deref(),
        "ProxyError" -> status.proxy_error.as_deref(),
    ]
}

/// Font counts, optionally forcing a scan for introspection calls.
fn font_counts_entry(scan: bool) -> Rule {
    let counts = if scan {
        Some(world::font_counts())
    } else {
        world::cached_counts()
    };
    rule!(
        "FontCounts" -> counts
            .map(|counts| settings::font_counts_expr(&counts))
            .unwrap_or_else(Expr::null)
    )
}

// ---------------------------------------------------------------------------
// ClearMathemeCache
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum CacheTarget {
    All,
    Fonts,
    Packages,
    Memoization,
}

fn cache_targets() -> impl Decoder<Value = CacheSelection> {
    let target = schema::enumeration(|name| schema::parse_name(name, "cache target"));
    decode::symbol(Symbol::ALL)
        .map(|()| CacheSelection::all())
        .or(decode::list(target).map(|targets| {
            let mut selection = CacheSelection::default();
            for target in targets {
                match target {
                    CacheTarget::All => return CacheSelection::all(),
                    CacheTarget::Fonts => selection.fonts = true,
                    CacheTarget::Packages => selection.packages = true,
                    CacheTarget::Memoization => selection.memoization = true,
                }
            }
            selection
        }))
}

wl_args! {
    #[action = "clearCache"]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ClearCacheArgs {
        "Targets" => pub targets: CacheSelection [cache_targets()] = CacheSelection::all(),
    }
}

pub fn run_clear(input: ClearCacheArgs) -> Answer {
    let cleared = world::clear_caches(input.targets);
    // Eagerly release cached glyph outlines / WL expressions when Fonts are
    // explicitly cleared (ordinary font reloads auto-invalidate via font_generation).
    if input.targets.fonts {
        matheme_graphics::clear_caches();
    }
    Outcome::new([rule!(
        "Cleared" -> Expr::list(cleared.into_iter().map(world::CacheTarget::as_str))
    )])
    .into()
}

// ---------------------------------------------------------------------------
// MathemeWarmup
// ---------------------------------------------------------------------------

wl_args! {
    #[action = "warmup"]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WarmupArgs {
        "Fonts" => pub fonts: bool [decode::boolean()] = true,
        "Packages" => pub packages: bool [decode::boolean()] = true,
        "PackageIndex" => pub package_index: bool [decode::boolean()] = true,
    }
}

pub fn run_warmup(input: WarmupArgs) -> Answer {
    let mut outcome = Outcome::new(Vec::new());

    if input.fonts {
        let start = std::time::Instant::now();
        let count = world::warmup_fonts();
        outcome.extend([
            rule!("Fonts" -> count as i64),
            rule!("FontsMs" -> start.elapsed().as_millis() as i64),
            font_counts_entry(false),
        ]);
    }

    if input.packages {
        let start = std::time::Instant::now();
        let status = world::warmup_packages();
        outcome.extend(package_entries(&status));
        outcome.extend([rule!("PackagesMs" -> start.elapsed().as_millis() as i64)]);
    }

    if input.package_index {
        let start = std::time::Instant::now();
        match world::prefetch_universe_index() {
            Ok(count) => outcome.extend([rule!("PackageIndex" -> count as i64)]),
            Err(err) => {
                outcome.extend([rule!("PackageIndex" -> Expr::null())]);
                outcome.warn(
                    DiagnosticSource::Package,
                    format!("failed to prefetch Universe index: {err}"),
                );
            }
        }
        outcome.extend([rule!(
            "PackageIndexMs" -> start.elapsed().as_millis() as i64
        )]);
    }

    outcome.into()
}

// ---------------------------------------------------------------------------
// MathemeReloadFonts / MathemeReloadPackages / MathemeClose
// ---------------------------------------------------------------------------

pub fn run_reload_fonts() -> Answer {
    // `world::reload_fonts()` advances `font_generation()`, which automatically
    // invalidates `matheme-graphics` glyph & expression caches on next export.
    let count = world::reload_fonts();
    Outcome::new([rule!("Fonts" -> count as i64), font_counts_entry(false)]).into()
}

pub fn run_reload_packages() -> Answer {
    Outcome::new(package_entries(&world::reload_packages())).into()
}

pub fn run_close() -> Answer {
    let released = world::clear_caches(CacheSelection::all());
    matheme_graphics::clear_caches();
    Outcome::new([rule!(
        "Released" -> Expr::list(released.into_iter().map(world::CacheTarget::as_str))
    )])
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::FromArgs;
    use crate::call::with_entries;

    #[test]
    fn clear_cache_parses_targets_and_rejects_unknown_keys_after_all() {
        assert_eq!(
            ClearCacheArgs::parse_args(&[]).unwrap().targets,
            CacheSelection::all()
        );
        with_entries([Rule::new("Targets", Symbol::ALL)], |args| {
            assert_eq!(
                ClearCacheArgs::parse_args(args).unwrap().targets,
                CacheSelection::all()
            );
        });
        // Regression test: `Targets -> All` must not short-circuit before
        // validating remaining keys.
        with_entries(
            [Rule::new("Targets", Symbol::ALL), Rule::new("Bogus", 1_i64)],
            |args| {
                let err = ClearCacheArgs::parse_args(args).expect_err("unknown key");
                assert!(
                    err.message.contains("unknown clearCache key: Bogus"),
                    "{}",
                    err.message
                );
            },
        );
        with_entries(
            [Rule::new("Targets", Expr::list(["Fonts", "Memoization"]))],
            |args| {
                assert_eq!(
                    ClearCacheArgs::parse_args(args).unwrap().targets,
                    CacheSelection {
                        fonts: true,
                        packages: false,
                        memoization: true,
                    }
                );
            },
        );
        with_entries([Rule::new("Targets", Expr::list(["Bogus"]))], |args| {
            assert!(ClearCacheArgs::parse_args(args).is_err());
        });
    }

    #[test]
    fn warmup_checks_key_name_before_value_type() {
        // Regression test: an unknown key with a non-boolean value must report
        // the unknown key rather than a boolean type mismatch.
        with_entries([Rule::new("Unknown", "not_a_bool")], |args| {
            let err = WarmupArgs::parse_args(args).expect_err("unknown key");
            assert!(
                err.message.contains("unknown warmup key: Unknown"),
                "{}",
                err.message
            );
        });
    }
}
