//! Wolfram shapes for the two independent cache queries.
//!
//! `MathemeQueryFonts` and `MathemeQueryPackages` are separate endpoints with
//! separate argument models and result schemas. This module only translates WL
//! values into [`matheme_world::query`] filters and renders the answers; the
//! world crate owns the data and all filtering logic.

use matheme_wl::{Expr, Rule, Symbol, assoc, rules};
use matheme_world as world;
use world::query::{
    FontAnswer, FontFace, FontFilter, FontOrigin, PackageAnswer, PackageEntry, PackageFilter,
    PackageOriginFilter, Page, UniverseMetadata,
};

use crate::args::schema;
use crate::call::{Answer, Outcome};
use crate::diagnostics::DiagnosticSource;
use crate::wl_args;
use matheme_wl::decode::{self, Decoder};

/// Default page size. Small enough that a bare `MathemeQueryFonts[]` does not
/// ship the whole font book over WSTP; `MaxResults -> All` opts out.
const DEFAULT_LIMIT: usize = 50;

fn query_limit() -> impl Decoder<Value = Option<usize>> {
    decode::symbol(Symbol::ALL)
        .map(|()| None)
        .or(decode::unsigned().map(Some))
}

wl_args! {
    #[action = "queryFonts"]
    #[derive(Debug, Clone)]
    pub struct FontQueryArgs {
        "Family" => pub family: Option<String> [decode::string().map(Some)] = None,
        "Source" => pub origin: Option<FontOrigin> [schema::enumeration(str::parse::<FontOrigin>).map(Some)] = None,
        "Resident" => pub resident: Option<bool> [decode::boolean().map(Some)] = None,
        "Character" => pub character: Option<char> [decode::character().map(Some)] = None,
        "MaxResults" => pub limit: Option<usize> [query_limit()] = Some(DEFAULT_LIMIT),
        "Offset" => pub offset: usize [decode::unsigned()] = 0,
    }
}

impl FontQueryArgs {
    fn filter(&self) -> FontFilter {
        FontFilter {
            family: self.family.clone(),
            origin: self.origin,
            resident: self.resident,
            character: self.character,
        }
    }

    fn page(&self) -> Page {
        Page {
            offset: self.offset,
            limit: self.limit,
        }
    }
}

wl_args! {
    #[action = "queryPackages"]
    #[derive(Debug, Clone)]
    pub struct PackageQueryArgs {
        "Name" => pub name: Option<String> [decode::string().map(Some)] = None,
        "Namespace" => pub namespace: Option<String> [decode::string().map(Some)] = None,
        "Origin" => pub origin: Option<PackageOriginFilter> [schema::enumeration(str::parse::<PackageOriginFilter>).map(Some)] = None,
        "Version" => pub version: Option<String> [decode::string().map(Some)] = None,
        "MaxResults" => pub limit: Option<usize> [query_limit()] = Some(DEFAULT_LIMIT),
        "Offset" => pub offset: usize [decode::unsigned()] = 0,
        "AllowDownloads" => pub allow_downloads: bool [decode::boolean()] = false,
        "Refresh" => pub refresh: bool [decode::boolean()] = false,
    }
}

impl PackageQueryArgs {
    fn filter(&self) -> PackageFilter {
        PackageFilter {
            name: self.name.clone(),
            namespace: self.namespace.clone(),
            origin: self.origin,
            version: self.version.clone(),
        }
    }

    fn page(&self) -> Page {
        Page {
            offset: self.offset,
            limit: self.limit,
        }
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
    rules![
        "Total" -> answer.total as i64,
        "Matched" -> answer.matched as i64,
        "Returned" -> answer.faces.len() as i64,
        "Offset" -> page.offset as i64,
        "MaxResults" -> max_results_expr(page),
        "Resident" -> answer.resident as i64,
        "Results" -> Expr::list(answer.faces.iter().map(font_face_expr)),
    ]
}

fn packages_expr(answer: &PackageAnswer, page: &Page) -> Vec<Rule> {
    rules![
        "Total" -> answer.total() as i64,
        "Matched" -> answer.matched as i64,
        "Returned" -> answer.entries.len() as i64,
        "Offset" -> page.offset as i64,
        "MaxResults" -> max_results_expr(page),
        "Local" -> answer.local_total as i64,
        "UniverseIndexCached" -> answer.universe_cached,
        "Universe" -> answer.universe_total as i64,
        "Results" -> Expr::list(answer.entries.iter().map(package_entry_expr)),
    ]
}

/// Execute a font query. Reading the book never loads a font file.
pub fn run_fonts(query: FontQueryArgs) -> Answer {
    let filter = query.filter();
    let page = query.page();
    let answer = world::query::fonts(&filter, &page);
    Outcome::new(fonts_expr(&answer, &page)).into()
}

/// Execute a package query. `Refresh` and `AllowDownloads` are the only two
/// opt-ins: a plain query neither rescans the disk nor reaches the network.
pub fn run_packages(query: PackageQueryArgs) -> Answer {
    let mut outcome = Outcome::new(Vec::new());
    if query.refresh {
        world::query::refresh_local();
    }
    if query.allow_downloads
        && !world::query::universe_cached()
        && let Err(err) = world::prefetch_universe_index()
    {
        outcome.warn(
            DiagnosticSource::Package,
            format!("failed to fetch Universe index: {err}"),
        );
    }
    let filter = query.filter();
    let page = query.page();
    let answer = world::query::packages(&filter, &page);
    outcome.extend(packages_expr(&answer, &page));
    outcome.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::FromArgs;
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
            FontQueryArgs::parse_args,
        )
        .expect("valid font query");
        assert_eq!(query.family.as_deref(), Some("libertinus"));
        assert_eq!(query.origin, Some(FontOrigin::Embedded));
        assert_eq!(query.resident, Some(true));
        assert_eq!(query.page().limit, Some(7));
        assert_eq!(query.page().offset, 2);
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
            PackageQueryArgs::parse_args,
        )
        .expect("valid package query");
        assert_eq!(query.name.as_deref(), Some("cetz"));
        assert_eq!(query.origin, Some(PackageOriginFilter::Universe));
        assert_eq!(query.page().limit, None);
        assert_eq!(max_results_expr(&query.page()), Symbol::ALL);
        assert!(query.allow_downloads);
        assert!(query.refresh);
    }

    #[test]
    fn no_arguments_uses_the_default_page() {
        assert_eq!(
            FontQueryArgs::parse_args(&[]).unwrap().page().limit,
            Some(DEFAULT_LIMIT)
        );
        assert_eq!(
            PackageQueryArgs::parse_args(&[]).unwrap().page().limit,
            Some(DEFAULT_LIMIT)
        );
    }

    #[test]
    fn unknown_keys_return_a_failure() {
        let err = with_entries([Rule::new("Mystery", 1_i64)], FontQueryArgs::parse_args)
            .expect_err("unknown font query key");
        assert_eq!(err.kind, crate::call::ErrorKind::Call);
        assert!(
            err.message.contains("unknown queryFonts key: Mystery"),
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
        let err = FontQueryArgs::parse_args(&association.as_association().expect("association"))
            .expect_err("delayed rule");
        assert_eq!(err.kind, crate::call::ErrorKind::Call);
        assert!(err.message.contains("RuleDelayed"), "{}", err.message);
    }
}
