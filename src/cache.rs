//! Cache lifecycle endpoints: `ClearMathemeCache`, `MathemeWarmup`,
//! `MathemeReloadFonts`, `MathemeReloadPackages`, and `MathemeClose`.

use matheme_wl::{rule, Entry, Expr, Rule, Symbol};
use matheme_world::{self as world, CacheSelection};

use crate::call::{self, Answer, Endpoint, Failure, Outcome};
use crate::settings;

pub(crate) fn package_entries(status: &world::PackageStatus) -> Vec<Rule> {
    vec![
        rule!("Packages" -> status.count as i64),
        rule!("PackageData" -> status.data.as_deref()),
        rule!("PackageCache" -> status.cache.as_deref()),
        rule!("DownloadsAllowed" -> status.downloads),
        rule!("DownloadsActive" -> status.downloads_active),
        rule!("Proxy" -> status.proxy.as_deref()),
        rule!("ProxyError" -> status.proxy_error.as_deref()),
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

pub struct ClearCache;

impl Endpoint for ClearCache {
    const NAME: &'static str = "clearCache";
    const HOST_FONTS: bool = false;
    type Input = CacheSelection;

    fn parse(args: &[Entry<'_>]) -> Result<Self::Input, Failure> {
        let mut selection = CacheSelection::default();
        let mut saw_targets = false;

        for entry in args {
            let (key, value) = call::pair(entry)?;
            match call::key(key)? {
                "Targets" => {
                    saw_targets = true;
                    if value.is_symbol(&Symbol::ALL) {
                        return Ok(CacheSelection::all());
                    }
                    let items = value
                        .as_list()
                        .ok_or_else(|| call::error("Targets must be All or a List of strings"))?;
                    for item in items {
                        let name = call::string(item)?;
                        if name.eq_ignore_ascii_case("all") {
                            selection = CacheSelection::all();
                        } else if name.eq_ignore_ascii_case("fonts") {
                            selection.fonts = true;
                        } else if name.eq_ignore_ascii_case("packages") {
                            selection.packages = true;
                        } else if name.eq_ignore_ascii_case("memoization") {
                            selection.memoization = true;
                        } else {
                            return Err(call::error(format!(
                                "unknown cache target {name:?}; expected Fonts, Packages, Memoization, or All"
                            )));
                        }
                    }
                }
                other => return Err(call::error(format!("unknown clearCache key: {other}"))),
            }
        }

        if !saw_targets {
            return Ok(CacheSelection::all());
        }
        Ok(selection)
    }

    fn invoke(selection: Self::Input) -> Answer {
        let cleared = world::clear_caches(selection);
        Outcome::new([rule!("Cleared" -> Expr::list(cleared))]).into()
    }
}

// ---------------------------------------------------------------------------
// MathemeWarmup
// ---------------------------------------------------------------------------

pub struct Warmup;

pub struct WarmupArgs {
    fonts: bool,
    packages: bool,
    package_index: bool,
}

impl Endpoint for Warmup {
    const NAME: &'static str = "warmup";
    type Input = WarmupArgs;

    fn parse(args: &[Entry<'_>]) -> Result<Self::Input, Failure> {
        let mut input = WarmupArgs {
            fonts: true,
            packages: true,
            package_index: true,
        };
        for entry in args {
            let (key, value) = call::pair(entry)?;
            let enabled = call::boolean(value)?;
            match call::key(key)? {
                "Fonts" => input.fonts = enabled,
                "Packages" => input.packages = enabled,
                "PackageIndex" => input.package_index = enabled,
                other => return Err(call::error(format!("unknown warmup key: {other}"))),
            }
        }
        Ok(input)
    }

    fn invoke(input: Self::Input) -> Answer {
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
                Ok(version) => outcome.extend([rule!("PackageIndex" -> version)]),
                Err(err) => {
                    outcome.extend([rule!("PackageIndex" -> Expr::null())]);
                    outcome.warn(format!("failed to prefetch Universe index: {err}"));
                }
            }
            outcome.extend([rule!(
                "PackageIndexMs" -> start.elapsed().as_millis() as i64
            )]);
        }

        outcome.into()
    }
}

// ---------------------------------------------------------------------------
// MathemeReloadFonts / MathemeReloadPackages / MathemeClose
// ---------------------------------------------------------------------------

pub struct ReloadFonts;

impl Endpoint for ReloadFonts {
    const NAME: &'static str = "reloadFonts";
    type Input = ();

    fn parse(args: &[Entry<'_>]) -> Result<(), Failure> {
        call::ensure_empty(args, Self::NAME)
    }

    fn invoke((): ()) -> Answer {
        let count = world::reload_fonts();
        Outcome::new([rule!("Fonts" -> count as i64), font_counts_entry(false)]).into()
    }
}

pub struct ReloadPackages;

impl Endpoint for ReloadPackages {
    const NAME: &'static str = "reloadPackages";
    const HOST_FONTS: bool = false;
    type Input = ();

    fn parse(args: &[Entry<'_>]) -> Result<(), Failure> {
        call::ensure_empty(args, Self::NAME)
    }

    fn invoke((): ()) -> Answer {
        Outcome::new(package_entries(&world::reload_packages())).into()
    }
}

pub struct Close;

impl Endpoint for Close {
    const NAME: &'static str = "close";
    const HOST_FONTS: bool = false;
    type Input = ();

    fn parse(args: &[Entry<'_>]) -> Result<(), Failure> {
        call::ensure_empty(args, Self::NAME)
    }

    fn invoke((): ()) -> Answer {
        let released = world::clear_caches(CacheSelection::all());
        Outcome::new([rule!("Released" -> Expr::list(released))]).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::with_entries;

    #[test]
    fn clear_cache_parses_targets() {
        assert_eq!(ClearCache::parse(&[]).unwrap(), CacheSelection::all());
        with_entries([Rule::new("Targets", Symbol::ALL)], |args| {
            assert_eq!(ClearCache::parse(args).unwrap(), CacheSelection::all());
        });
        with_entries(
            [Rule::new("Targets", Expr::list(["Fonts", "Memoization"]))],
            |args| {
                assert_eq!(
                    ClearCache::parse(args).unwrap(),
                    CacheSelection {
                        fonts: true,
                        packages: false,
                        memoization: true,
                    }
                );
            },
        );
        with_entries([Rule::new("Targets", Expr::list(["Bogus"]))], |args| {
            assert!(ClearCache::parse(args).is_err());
        });
    }
}
