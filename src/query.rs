//! Wolfram shapes for the two independent cache queries.
//!
//! `MathemeQueryFonts` and `MathemeQueryPackages` are separate endpoints with
//! separate argument models and result schemas. This module only translates WL
//! values into [`matheme_world::query`] filters and renders the answers; the
//! world crate owns the data and all filtering logic.

use matheme_wl::{Entry, Expr, Rule, Symbol};
use matheme_world as world;
use world::query::{
    FontAnswer, FontFace, FontFilter, FontOrigin, PackageAnswer, PackageEntry, PackageFilter,
    PackageOriginFilter, Page, UniverseMetadata,
};

use crate::call::{self, Outcome};

/// Default page size. Small enough that a bare `MathemeQueryFonts[]` does not
/// ship the whole font book over WSTP; `MaxResults -> All` opts out.
const DEFAULT_LIMIT: usize = 50;

/// Expected arguments of `MathemeQueryFonts`: an optional Association of font
/// filters and paging keys.
#[derive(Debug, Clone)]
pub struct FontQuery {
    pub filter: FontFilter,
    pub page: Page,
}

/// Expected arguments of `MathemeQueryPackages`: an optional Association of
/// package filters, paging keys and the two explicit opt-ins.
#[derive(Debug, Clone)]
pub struct PackageQuery {
    pub filter: PackageFilter,
    pub page: Page,
    pub allow_downloads: bool,
    pub refresh: bool,
}

fn default_page() -> Page {
    Page {
        offset: 0,
        limit: Some(DEFAULT_LIMIT),
    }
}

/// The two paging keys shared by both query surfaces.
fn page_entry(page: &mut Page, name: &str, value: &Expr) -> bool {
    match name {
        "MaxResults" => {
            page.limit = if value.is_symbol(&Symbol::ALL) {
                None
            } else {
                Some(call::nonnegative(value, "MaxResults") as usize)
            };
            true
        }
        "Offset" => {
            page.offset = call::nonnegative(value, "Offset") as usize;
            true
        }
        _ => false,
    }
}

impl FontQuery {
    pub fn parse(args: Vec<Entry>) -> Self {
        let mut query = Self {
            filter: FontFilter::default(),
            page: default_page(),
        };
        for entry in &args {
            let (key, value) = call::pair(entry);
            let name = call::key(key);
            if page_entry(&mut query.page, name, value) {
                continue;
            }
            match name {
                "Family" => query.filter.family = Some(call::string(value)),
                "Source" => {
                    query.filter.origin = Some(call::named(value, FontOrigin::parse, "font source"))
                }
                "Resident" => query.filter.resident = Some(call::boolean(value)),
                "Character" => query.filter.character = Some(call::character(value)),
                other => panic!("unknown font query key: {other}"),
            }
        }
        query
    }
}

impl PackageQuery {
    pub fn parse(args: Vec<Entry>) -> Self {
        let mut query = Self {
            filter: PackageFilter::default(),
            page: default_page(),
            allow_downloads: false,
            refresh: false,
        };
        for entry in &args {
            let (key, value) = call::pair(entry);
            let name = call::key(key);
            if page_entry(&mut query.page, name, value) {
                continue;
            }
            match name {
                "Name" => query.filter.name = Some(call::string(value)),
                "Namespace" => query.filter.namespace = Some(call::string(value)),
                "Origin" => {
                    query.filter.origin = Some(call::named(
                        value,
                        PackageOriginFilter::parse,
                        "package origin",
                    ))
                }
                "Version" => query.filter.version = Some(call::string(value)),
                "AllowDownloads" => query.allow_downloads = call::boolean(value),
                "Refresh" => query.refresh = call::boolean(value),
                other => panic!("unknown package query key: {other}"),
            }
        }
        query
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

/// Execute a font query. Reading the book never loads a font file.
pub fn run_fonts(query: &FontQuery) -> Outcome {
    let answer = world::query::fonts(&query.filter, &query.page);
    Outcome::new(fonts_expr(&answer, &query.page))
}

/// Execute a package query. `Refresh` and `AllowDownloads` are the only two
/// opt-ins: a plain query neither rescans the disk nor reaches the network.
pub fn run_packages(query: &PackageQuery) -> Outcome {
    let mut outcome = Outcome::new(Vec::new());
    if query.refresh {
        world::query::refresh_local();
    }
    if query.allow_downloads && !world::query::universe_cached() {
        if let Err(err) = world::prefetch_universe_index() {
            outcome.warn(format!("failed to fetch Universe index: {err}"));
        }
    }
    let answer = world::query::packages(&query.filter, &query.page);
    outcome.extend(packages_expr(&answer, &query.page));
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(rules: impl IntoIterator<Item = Rule>) -> Vec<Entry> {
        Expr::association(rules)
            .as_association()
            .expect("association")
    }

    #[test]
    fn font_filters_and_paging() {
        let query = FontQuery::parse(entries([
            Rule::new("Family", "libertinus"),
            Rule::new("Source", "Embedded"),
            Rule::new("Resident", true),
            Rule::new("MaxResults", 7_i64),
            Rule::new("Offset", 2_i64),
        ]));
        assert_eq!(query.filter.family.as_deref(), Some("libertinus"));
        assert_eq!(query.filter.origin, Some(FontOrigin::Embedded));
        assert_eq!(query.filter.resident, Some(true));
        assert_eq!(query.page.limit, Some(7));
        assert_eq!(query.page.offset, 2);
    }

    #[test]
    fn package_filters_and_opt_ins() {
        let query = PackageQuery::parse(entries([
            Rule::new("Name", "cetz"),
            Rule::new("Origin", "Universe"),
            Rule::new("MaxResults", Symbol::ALL),
            Rule::new("AllowDownloads", true),
            Rule::new("Refresh", true),
        ]));
        assert_eq!(query.filter.name.as_deref(), Some("cetz"));
        assert_eq!(query.filter.origin, Some(PackageOriginFilter::Universe));
        assert_eq!(query.page.limit, None);
        assert!(query.allow_downloads);
        assert!(query.refresh);
    }

    #[test]
    fn no_arguments_uses_the_default_page() {
        assert_eq!(FontQuery::parse(vec![]).page.limit, Some(DEFAULT_LIMIT));
        assert_eq!(PackageQuery::parse(vec![]).page.limit, Some(DEFAULT_LIMIT));
    }

    #[test]
    #[should_panic(expected = "unknown font query key")]
    fn unknown_keys_are_not_silently_ignored() {
        FontQuery::parse(entries([Rule::new("Mystery", 1_i64)]));
    }

    #[test]
    #[should_panic(expected = "RuleDelayed")]
    fn delayed_rules_are_not_guessed() {
        let association = Expr::normal(
            Symbol::ASSOCIATION,
            [Expr::normal(
                Symbol::RULE_DELAYED,
                [Expr::from("Family"), Expr::from("Libertinus")],
            )],
        );
        FontQuery::parse(association.as_association().expect("association"));
    }
}
