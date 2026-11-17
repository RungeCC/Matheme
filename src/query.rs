//! Wolfram expression shapes for the cache query API.
//!
//! `MathemeQuery` is a read-only view over the metadata the library already
//! caches. This module only translates WL values into [`matheme_world::query`]
//! filters and renders the answers back; the world crate owns the data and all
//! filtering logic.

use matheme_wl::{Entry, Expr, Rule, Symbol};
use matheme_world as world;
use world::query::{
    CacheState, FontAnswer, FontFace, FontFilter, FontOrigin, PackageAnswer, PackageEntry,
    PackageFilter, PackageOriginFilter, Page, Target, UniverseMetadata,
};

/// Default page size. Small enough that a bare `MathemeQuery["Fonts"]` does
/// not ship the whole 1200-face book over WSTP; `MaxResults -> All` opts out.
const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone)]
pub struct Query {
    pub target: Target,
    pub fonts: FontFilter,
    pub packages: PackageFilter,
    pub page: Page,
    pub allow_downloads: bool,
    pub refresh: bool,
    pub warnings: Vec<String>,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            target: Target::Caches,
            fonts: FontFilter::default(),
            packages: PackageFilter::default(),
            page: Page {
                offset: 0,
                limit: Some(DEFAULT_LIMIT),
            },
            allow_downloads: false,
            refresh: false,
            warnings: Vec::new(),
        }
    }
}

impl Query {
    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

/// Normalize the accepted call shapes:
///
/// - `MathemeQuery[]`
/// - `MathemeQuery["Fonts"]` / `MathemeQuery["Packages"]` / `MathemeQuery["Caches"]`
/// - `MathemeQuery[<|"Target" -> …, …|>]`
/// - `MathemeQuery["Fonts", <|"Family" -> …|>]`
pub fn parse(args: &[Expr]) -> Result<Query, String> {
    let mut query = Query::default();
    let mut positional: Option<&str> = None;
    let mut options: Option<Vec<Entry>> = None;

    match args {
        [] => {}
        [arg] => {
            if let Some(name) = arg.as_str() {
                positional = Some(name);
            } else {
                options =
                    Some(arg.as_association().ok_or_else(|| {
                        "query must be a String target or an Association".to_owned()
                    })?);
            }
        }
        [target, filters] => {
            positional = Some(
                target
                    .as_str()
                    .ok_or_else(|| "query target must be a String".to_owned())?,
            );
            options = Some(
                filters
                    .as_association()
                    .ok_or_else(|| "query filters must be an Association".to_owned())?,
            );
        }
        _ => return Err("MathemeQuery takes at most two arguments".to_owned()),
    }

    if let Some(name) = positional {
        set_target(&mut query, name);
    }

    for entry in options.unwrap_or_default() {
        let (key, value) = match entry.pair() {
            Ok(pair) => pair,
            Err(reason) => {
                query.warn(reason);
                continue;
            }
        };
        let Some(name) = key.as_str() else {
            query.warn(format!(
                "query key {} is not a String; entry skipped",
                key.raw()
            ));
            continue;
        };
        match name {
            "Target" => match value.as_str() {
                Some(name) => set_target(&mut query, name),
                None => query.warn("Target must be a String"),
            },
            "MaxResults" => {
                if value.is_symbol(&Symbol::ALL) {
                    query.page.limit = None;
                } else {
                    match value.as_i64() {
                        Some(count) if count >= 0 => query.page.limit = Some(count as usize),
                        _ => query.warn("MaxResults must be a non-negative Integer or All"),
                    }
                }
            }
            "Offset" => match value.as_i64() {
                Some(offset) if offset >= 0 => query.page.offset = offset as usize,
                _ => query.warn("Offset must be a non-negative Integer"),
            },
            "Family" => match value.as_str() {
                Some(family) => query.fonts.family = Some(family.to_owned()),
                None => query.warn("Family must be a String"),
            },
            "Source" => match value.as_str().and_then(FontOrigin::parse) {
                Some(origin) => query.fonts.origin = Some(origin),
                None => query.warn("Source must be Custom, Host, Embedded or System"),
            },
            "Resident" => match value.as_bool() {
                Some(resident) => query.fonts.resident = Some(resident),
                None => query.warn("Resident must be True or False"),
            },
            "Character" => match value.as_str().and_then(|s| s.chars().next()) {
                Some(character) => {
                    if value.as_str().is_some_and(|s| s.chars().count() > 1) {
                        query.warn("Character has more than one character; using the first");
                    }
                    query.fonts.character = Some(character);
                }
                None => query.warn("Character must be a non-empty String"),
            },
            "Name" => match value.as_str() {
                Some(name) => query.packages.name = Some(name.to_owned()),
                None => query.warn("Name must be a String"),
            },
            "Namespace" => match value.as_str() {
                Some(namespace) => query.packages.namespace = Some(namespace.to_owned()),
                None => query.warn("Namespace must be a String"),
            },
            "Origin" => match value.as_str().and_then(PackageOriginFilter::parse) {
                Some(origin) => query.packages.origin = Some(origin),
                None => query.warn("Origin must be Local, Data, Cache or Universe"),
            },
            "Version" => match value.as_str() {
                Some(version) => query.packages.version = Some(version.to_owned()),
                None => query.warn("Version must be a String"),
            },
            "AllowDownloads" => match value.as_bool() {
                Some(allowed) => query.allow_downloads = allowed,
                None => query.warn("AllowDownloads must be True or False"),
            },
            "Refresh" => match value.as_bool() {
                Some(refresh) => query.refresh = refresh,
                None => query.warn("Refresh must be True or False"),
            },
            other => query.warn(format!("unknown query key: {other}")),
        }
    }

    Ok(query)
}

fn set_target(query: &mut Query, name: &str) {
    match Target::parse(name) {
        Some(target) => query.target = target,
        None => query.warn(format!("unknown query target: {name}")),
    }
}

fn opt(value: &Option<String>) -> Expr {
    match value {
        Some(value) => Expr::string(value),
        None => Expr::null(),
    }
}

fn opt_i64(value: Option<i64>) -> Expr {
    match value {
        Some(value) => value.into(),
        None => Expr::null(),
    }
}

fn str_list(items: &[String]) -> Expr {
    Expr::list(items.iter().cloned())
}

fn max_results_expr(page: &Page) -> Expr {
    match page.limit {
        Some(limit) => (limit as i64).into(),
        None => Expr::null(),
    }
}

fn font_face_expr(face: &FontFace) -> Expr {
    Expr::association([
        Rule::new("Index", face.index as i64),
        Rule::new("Family", face.family.clone()),
        Rule::new("Style", face.style),
        Rule::new("Weight", face.weight as i64),
        Rule::new("Stretch", face.stretch),
        Rule::new("Flags", Expr::list(face.flags.iter().copied())),
        Rule::new("Source", face.origin.as_str()),
        Rule::new("Path", opt(&face.path)),
        Rule::new("CollectionIndex", face.collection_index as i64),
        Rule::new("Resident", face.resident),
    ])
}

fn universe_metadata_expr(metadata: &UniverseMetadata) -> Expr {
    Expr::association([
        Rule::new("Description", opt(&metadata.description)),
        Rule::new("Authors", str_list(&metadata.authors)),
        Rule::new("License", opt(&metadata.license)),
        Rule::new("Keywords", str_list(&metadata.keywords)),
        Rule::new("Categories", str_list(&metadata.categories)),
        Rule::new("Disciplines", str_list(&metadata.disciplines)),
        Rule::new("Entrypoint", opt(&metadata.entrypoint)),
        Rule::new("Repository", opt(&metadata.repository)),
        Rule::new("Homepage", opt(&metadata.homepage)),
        Rule::new("Compiler", opt(&metadata.compiler)),
        Rule::new("UpdatedAt", opt_i64(metadata.updated_at)),
        Rule::new("Template", metadata.template),
    ])
}

fn package_entry_expr(entry: &PackageEntry) -> Expr {
    let path = entry.path.as_ref().map(|path| path.display().to_string());
    let metadata = entry
        .metadata
        .as_ref()
        .map(universe_metadata_expr)
        .unwrap_or_else(Expr::null);
    Expr::association([
        Rule::new("Namespace", entry.namespace.clone()),
        Rule::new("Name", entry.name.clone()),
        Rule::new("Version", entry.version.clone()),
        Rule::new("Origin", entry.origin.as_str()),
        Rule::new("Path", opt(&path)),
        Rule::new("Metadata", metadata),
    ])
}

fn counts_expr(state: &CacheState) -> Expr {
    match &state.font_counts {
        Some(counts) => crate::settings::font_counts_expr(counts),
        None => Expr::null(),
    }
}

fn caches_expr(state: &CacheState) -> Vec<Rule> {
    vec![
        Rule::new(
            "Fonts",
            Expr::association([
                Rule::new("Cached", state.fonts_cached),
                Rule::new("Counts", counts_expr(state)),
                Rule::new("Resident", state.resident as i64),
            ]),
        ),
        Rule::new(
            "Packages",
            Expr::association([
                Rule::new("Cached", state.packages_cached),
                Rule::new("Local", opt_i64(state.local_packages.map(|n| n as i64))),
                Rule::new("UniverseIndexCached", state.universe_cached),
                Rule::new(
                    "Universe",
                    opt_i64(state.universe_packages.map(|n| n as i64)),
                ),
            ]),
        ),
    ]
}

fn fonts_expr(answer: &FontAnswer, page: &Page) -> Vec<Rule> {
    vec![
        Rule::new("Total", answer.total as i64),
        Rule::new("Matched", answer.matched as i64),
        Rule::new("Returned", answer.faces.len() as i64),
        Rule::new("Offset", page.offset as i64),
        Rule::new("MaxResults", max_results_expr(page)),
        Rule::new("Resident", answer.resident as i64),
        Rule::new(
            "Results",
            Expr::list(answer.faces.iter().map(font_face_expr)),
        ),
    ]
}

fn packages_expr(answer: &PackageAnswer, page: &Page) -> Vec<Rule> {
    vec![
        Rule::new("Total", answer.total() as i64),
        Rule::new("Matched", answer.matched as i64),
        Rule::new("Returned", answer.entries.len() as i64),
        Rule::new("Offset", page.offset as i64),
        Rule::new("MaxResults", max_results_expr(page)),
        Rule::new("Local", answer.local_total as i64),
        Rule::new("UniverseIndexCached", answer.universe_cached),
        Rule::new("Universe", answer.universe_total as i64),
        Rule::new(
            "Results",
            Expr::list(answer.entries.iter().map(package_entry_expr)),
        ),
    ]
}

/// Execute a parsed query and render its answer.
pub fn run(query: &Query) -> Expr {
    let mut warnings = query.warnings.clone();

    // Both are explicit opt-ins: a plain query never touches the disk listing
    // twice or reaches the network.
    if query.refresh {
        world::query::refresh_local();
    }
    if query.allow_downloads && !world::query::universe_cached() {
        if let Err(err) = world::prefetch_universe_index() {
            warnings.push(format!("failed to fetch Universe index: {err}"));
        }
    }

    let mut entries = vec![
        Rule::new("Status", "Success"),
        Rule::new("Target", query.target.as_str()),
    ];
    match query.target {
        Target::Caches => entries.extend(caches_expr(&world::query::caches())),
        Target::Fonts => {
            let answer = world::query::fonts(&query.fonts, &query.page);
            entries.extend(fonts_expr(&answer, &query.page));
        }
        Target::Packages => {
            let answer = world::query::packages(&query.packages, &query.page);
            entries.extend(packages_expr(&answer, &query.page));
        }
    }

    if !warnings.is_empty() {
        entries.push(Rule::new(
            "Diagnostics",
            Expr::list(
                warnings
                    .into_iter()
                    .map(|message| crate::diagnostics::expr(&crate::diagnostics::warning(message))),
            ),
        ));
    }

    Expr::association(entries)
}

/// Error answer for a malformed call shape.
pub fn error(reason: &str) -> Expr {
    Expr::association([Rule::new("Status", "Error"), Rule::new("Message", reason)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_target_and_filters() {
        let query = parse(&[
            Expr::from("Fonts"),
            Expr::association([
                Rule::new("Family", "libertinus"),
                Rule::new("Source", "Embedded"),
                Rule::new("Resident", true),
                Rule::new("MaxResults", 7_i64),
            ]),
        ])
        .expect("query");
        assert_eq!(query.target, Target::Fonts);
        assert_eq!(query.fonts.family.as_deref(), Some("libertinus"));
        assert_eq!(query.fonts.origin, Some(FontOrigin::Embedded));
        assert_eq!(query.fonts.resident, Some(true));
        assert_eq!(query.page.limit, Some(7));
        assert!(query.warnings.is_empty());
    }

    #[test]
    fn no_arguments_is_cache_overview() {
        let query = parse(&[]).expect("query");
        assert_eq!(query.target, Target::Caches);
        assert_eq!(query.page.limit, Some(DEFAULT_LIMIT));
    }

    #[test]
    fn all_removes_the_limit_and_bad_keys_warn() {
        let query = parse(&[Expr::association([
            Rule::new("Target", "Packages"),
            Rule::new("MaxResults", Symbol::ALL),
            Rule::new("Mystery", 1_i64),
            Rule::new("Offset", -1_i64),
        ])])
        .expect("query");
        assert_eq!(query.target, Target::Packages);
        assert_eq!(query.page.limit, None);
        assert_eq!(query.page.offset, 0);
        assert_eq!(query.warnings.len(), 2);
    }

    #[test]
    fn delayed_rules_are_reported_not_guessed() {
        let query = parse(&[Expr::normal(
            Symbol::ASSOCIATION,
            [Expr::normal(
                Symbol::RULE_DELAYED,
                [Expr::from("Target"), Expr::from("Fonts")],
            )],
        )])
        .expect("query");
        assert_eq!(query.warnings.len(), 1);
        assert!(query.warnings[0].contains("RuleDelayed"));
    }
}
