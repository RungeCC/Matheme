//! Wolfram shapes for the two independent cache queries.
//!
//! `MathemeQueryFonts` and `MathemeQueryPackages` are separate endpoints with
//! separate argument models and result schemas. This module only translates WL
//! values into [`matheme_world::query`] filters and renders the answers; the
//! world crate owns the data and all filtering logic.

use matheme_wl::{assoc, rule, Entry, Expr, Rule, Symbol};
use matheme_world as world;
use world::query::{
    FontAnswer, FontFace, FontFilter, FontOrigin, PackageAnswer, PackageEntry, PackageFilter,
    PackageOriginFilter, Page, UniverseMetadata,
};

use crate::call::{self, Answer, Endpoint, Failure, Outcome};

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
fn page_entry(page: &mut Page, name: &str, value: &Expr) -> Result<bool, Failure> {
    match name {
        "MaxResults" => {
            page.limit = if value.is_symbol(&Symbol::ALL) {
                None
            } else {
                Some(call::nonnegative(value, "MaxResults")? as usize)
            };
            Ok(true)
        }
        "Offset" => {
            page.offset = call::nonnegative(value, "Offset")? as usize;
            Ok(true)
        }
        _ => Ok(false),
    }
}

impl FontQuery {
    pub fn parse(args: &[Entry<'_>]) -> Result<Self, Failure> {
        let mut query = Self {
            filter: FontFilter::default(),
            page: default_page(),
        };
        for entry in args {
            let (key, value) = call::pair(entry)?;
            let name = call::key(key)?;
            if page_entry(&mut query.page, name, value)? {
                continue;
            }
            match name {
                "Family" => query.filter.family = Some(call::string(value)?),
                "Source" => {
                    query.filter.origin =
                        Some(call::named(value, FontOrigin::parse, "font source")?)
                }
                "Resident" => query.filter.resident = Some(call::boolean(value)?),
                "Character" => query.filter.character = Some(call::character(value)?),
                other => return Err(call::error(format!("unknown font query key: {other}"))),
            }
        }
        Ok(query)
    }
}

impl PackageQuery {
    pub fn parse(args: &[Entry<'_>]) -> Result<Self, Failure> {
        let mut query = Self {
            filter: PackageFilter::default(),
            page: default_page(),
            allow_downloads: false,
            refresh: false,
        };
        for entry in args {
            let (key, value) = call::pair(entry)?;
            let name = call::key(key)?;
            if page_entry(&mut query.page, name, value)? {
                continue;
            }
            match name {
                "Name" => query.filter.name = Some(call::string(value)?),
                "Namespace" => query.filter.namespace = Some(call::string(value)?),
                "Origin" => {
                    query.filter.origin = Some(call::named(
                        value,
                        PackageOriginFilter::parse,
                        "package origin",
                    )?)
                }
                "Version" => query.filter.version = Some(call::string(value)?),
                "AllowDownloads" => query.allow_downloads = call::boolean(value)?,
                "Refresh" => query.refresh = call::boolean(value)?,
                other => return Err(call::error(format!("unknown package query key: {other}"))),
            }
        }
        Ok(query)
    }
}

fn str_list(items: &[String]) -> Expr {
    Expr::list(items.iter().cloned())
}

fn max_results_expr(page: &Page) -> Expr {
    match page.limit {
        Some(limit) => (limit as i64).into(),
        None => Expr::symbol(Symbol::ALL),
    }
}

fn font_face_expr(face: &FontFace) -> Expr {
    assoc! {
        "Index" -> face.index as i64,
        "Family" -> face.family.as_str(),
        "Style" -> face.style,
        "Weight" -> face.weight as i64,
        "Stretch" -> face.stretch,
        "Flags" -> Expr::list(face.flags.iter().copied()),
        "Source" -> face.origin.as_str(),
        "Path" -> face.path.as_deref(),
        "CollectionIndex" -> face.collection_index as i64,
        "Resident" -> face.resident,
    }
}

fn universe_metadata_expr(metadata: &UniverseMetadata) -> Expr {
    assoc! {
        "Description" -> metadata.description.as_deref(),
        "Authors" -> str_list(&metadata.authors),
        "License" -> metadata.license.as_deref(),
        "Keywords" -> str_list(&metadata.keywords),
        "Categories" -> str_list(&metadata.categories),
        "Disciplines" -> str_list(&metadata.disciplines),
        "Entrypoint" -> metadata.entrypoint.as_deref(),
        "Repository" -> metadata.repository.as_deref(),
        "Homepage" -> metadata.homepage.as_deref(),
        "Compiler" -> metadata.compiler.as_deref(),
        "UpdatedAt" -> metadata.updated_at,
        "Template" -> metadata.template,
    }
}

fn package_entry_expr(entry: &PackageEntry) -> Expr {
    let path = entry.path.as_ref().map(|path| path.display().to_string());
    let metadata = entry.metadata.as_ref().map(universe_metadata_expr);
    assoc! {
        "Namespace" -> entry.namespace.as_str(),
        "Name" -> entry.name.as_str(),
        "Version" -> entry.version.as_str(),
        "Origin" -> entry.origin.as_str(),
        "Path" -> path,
        "Metadata" -> metadata,
    }
}

fn fonts_expr(answer: &FontAnswer, page: &Page) -> Vec<Rule> {
    vec![
        rule!("Total" -> answer.total as i64),
        rule!("Matched" -> answer.matched as i64),
        rule!("Returned" -> answer.faces.len() as i64),
        rule!("Offset" -> page.offset as i64),
        rule!("MaxResults" -> max_results_expr(page)),
        rule!("Resident" -> answer.resident as i64),
        rule!("Results" -> Expr::list(answer.faces.iter().map(font_face_expr))),
    ]
}

fn packages_expr(answer: &PackageAnswer, page: &Page) -> Vec<Rule> {
    vec![
        rule!("Total" -> answer.total() as i64),
        rule!("Matched" -> answer.matched as i64),
        rule!("Returned" -> answer.entries.len() as i64),
        rule!("Offset" -> page.offset as i64),
        rule!("MaxResults" -> max_results_expr(page)),
        rule!("Local" -> answer.local_total as i64),
        rule!("UniverseIndexCached" -> answer.universe_cached),
        rule!("Universe" -> answer.universe_total as i64),
        rule!("Results" -> Expr::list(answer.entries.iter().map(package_entry_expr))),
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

pub struct QueryFonts;

impl Endpoint for QueryFonts {
    const NAME: &'static str = "queryFonts";
    type Input = FontQuery;

    fn parse(args: &[Entry<'_>]) -> Result<Self::Input, Failure> {
        FontQuery::parse(args)
    }

    fn invoke(input: Self::Input) -> Answer {
        run_fonts(&input).into()
    }
}

pub struct QueryPackages;

impl Endpoint for QueryPackages {
    const NAME: &'static str = "queryPackages";
    const HOST_FONTS: bool = false;
    type Input = PackageQuery;

    fn parse(args: &[Entry<'_>]) -> Result<Self::Input, Failure> {
        PackageQuery::parse(args)
    }

    fn invoke(input: Self::Input) -> Answer {
        run_packages(&input).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::with_entries;

    #[test]
    fn font_filters_and_paging() {
        let query = with_entries(
            [
                Rule::new("Family", "libertinus"),
                Rule::new("Source", "Embedded"),
                Rule::new("Resident", true),
                Rule::new("MaxResults", 7_i64),
                Rule::new("Offset", 2_i64),
            ],
            FontQuery::parse,
        )
        .expect("valid font query");
        assert_eq!(query.filter.family.as_deref(), Some("libertinus"));
        assert_eq!(query.filter.origin, Some(FontOrigin::Embedded));
        assert_eq!(query.filter.resident, Some(true));
        assert_eq!(query.page.limit, Some(7));
        assert_eq!(query.page.offset, 2);
    }

    #[test]
    fn package_filters_and_opt_ins() {
        let query = with_entries(
            [
                Rule::new("Name", "cetz"),
                Rule::new("Origin", "Universe"),
                Rule::new("MaxResults", Symbol::ALL),
                Rule::new("AllowDownloads", true),
                Rule::new("Refresh", true),
            ],
            PackageQuery::parse,
        )
        .expect("valid package query");
        assert_eq!(query.filter.name.as_deref(), Some("cetz"));
        assert_eq!(query.filter.origin, Some(PackageOriginFilter::Universe));
        assert_eq!(query.page.limit, None);
        assert_eq!(max_results_expr(&query.page), Symbol::ALL);
        assert!(query.allow_downloads);
        assert!(query.refresh);
    }

    #[test]
    fn no_arguments_uses_the_default_page() {
        assert_eq!(
            FontQuery::parse(&[]).unwrap().page.limit,
            Some(DEFAULT_LIMIT)
        );
        assert_eq!(
            PackageQuery::parse(&[]).unwrap().page.limit,
            Some(DEFAULT_LIMIT)
        );
    }

    #[test]
    fn unknown_keys_return_a_failure() {
        let err = with_entries([Rule::new("Mystery", 1_i64)], FontQuery::parse)
            .expect_err("unknown font query key");
        assert_eq!(err.tag, "Call");
        assert!(
            err.message.contains("unknown font query key"),
            "{}",
            err.message
        );
    }

    #[test]
    fn delayed_rules_return_a_failure() {
        let association = Expr::normal(
            Symbol::ASSOCIATION,
            [Expr::normal(
                Symbol::RULE_DELAYED,
                [Expr::from("Family"), Expr::from("Libertinus")],
            )],
        );
        let err = FontQuery::parse(&association.as_association().expect("association"))
            .expect_err("delayed rule");
        assert_eq!(err.tag, "Call");
        assert!(err.message.contains("RuleDelayed"), "{}", err.message);
    }
}
