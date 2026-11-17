//! WSTP bridge between the Wolfram Language package and `matheme-world`.
//!
//! This crate only declares the exported link functions. Each one names an
//! [`endpoints::Endpoint`], which models its expected arguments; [`call::serve`]
//! implements the shared read → parse → invoke → write pipeline.

mod call;
mod diagnostics;
mod endpoints;
mod query;
mod settings;
mod wolfram;

use wolfram_library_link::{export, wstp::Link};

use call::serve;
use endpoints::{
    Close, Compile, QueryFonts, QueryPackages, ReloadFonts, ReloadPackages, Settings, Version,
    Warmup,
};

#[export(wstp)]
fn matheme_compile(link: &mut Link) {
    serve::<Compile>(link);
}

#[export(wstp)]
fn matheme_version(link: &mut Link) {
    serve::<Version>(link);
}

#[export(wstp)]
fn matheme_settings(link: &mut Link) {
    serve::<Settings>(link);
}

#[export(wstp)]
fn matheme_warmup(link: &mut Link) {
    serve::<Warmup>(link);
}

#[export(wstp)]
fn matheme_reload_fonts(link: &mut Link) {
    serve::<ReloadFonts>(link);
}

#[export(wstp)]
fn matheme_reload_packages(link: &mut Link) {
    serve::<ReloadPackages>(link);
}

#[export(wstp)]
fn matheme_query_fonts(link: &mut Link) {
    serve::<QueryFonts>(link);
}

#[export(wstp)]
fn matheme_query_packages(link: &mut Link) {
    serve::<QueryPackages>(link);
}

#[export(wstp)]
fn matheme_close(link: &mut Link) {
    serve::<Close>(link);
}
