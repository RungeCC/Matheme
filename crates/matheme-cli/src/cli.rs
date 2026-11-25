//! `clap` command-line interface for `matheme` — a Typst CLI extension that
//! actively discovers and communicates with a Wolfram Kernel over WSTP.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use matheme_world::compile::{self, Feature, OutputFormat, PageRanges, PdfStandard};
use matheme_world::query::{FontOrigin, PackageOriginFilter};

#[derive(Debug, Parser)]
#[command(
    name = "matheme",
    version,
    about = "Typst compiler extension with active Wolfram Kernel discovery and WSTP communication"
)]
pub struct Cli {
    /// Explicit path to a WolframKernel / MathKernel executable.
    #[arg(long, global = true, value_name = "PATH")]
    pub kernel: Option<PathBuf>,

    /// Connect to an existing WSTP link name instead of launching a new kernel.
    #[arg(long, global = true, value_name = "LINK")]
    pub link_name: Option<String>,

    /// Disable automatic discovery of Wolfram's bundled fonts.
    #[arg(long, global = true)]
    pub no_wolfram_fonts: bool,

    /// Additional font directories to search (repeatable).
    #[arg(long = "font-path", global = true, value_name = "DIR")]
    pub font_paths: Vec<PathBuf>,

    /// Do not scan operating-system fonts.
    #[arg(long, global = true)]
    pub ignore_system_fonts: bool,

    /// Typst package data directory override.
    #[arg(long, global = true, value_name = "DIR")]
    pub package_path: Option<PathBuf>,

    /// Typst package cache directory override.
    #[arg(long, global = true, value_name = "DIR")]
    pub package_cache: Option<PathBuf>,

    /// Explicit HTTP/HTTPS proxy URL for package downloads.
    #[arg(long, global = true, value_name = "URL")]
    pub proxy: Option<String>,

    /// Allow downloading missing Typst Universe packages.
    #[arg(long, global = true)]
    pub allow_downloads: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Compile a Typst file to PDF, SVG, or HTML, optionally evaluating Wolfram
    /// expressions into `sys.inputs` over WSTP.
    #[command(visible_alias = "c")]
    Compile(CompileCmd),

    /// Evaluate one or more Wolfram Language expressions against a discovered
    /// Wolfram Kernel over WSTP.
    #[command(visible_alias = "e")]
    Eval(EvalCmd),

    /// Discover and list Wolfram Language installations and kernels on this machine.
    Kernels,

    /// Query cached Typst font metadata (including auto-discovered Wolfram fonts).
    Fonts(FontsCmd),

    /// Query local and Typst Universe packages.
    Packages(PackagesCmd),

    /// Show Matheme CLI, Typst, and detected Wolfram Kernel version information.
    Version,
}

#[derive(Debug, Args)]
pub struct CompileCmd {
    /// Input `.typ` file path, or `-` to read Typst source from stdin.
    pub input: PathBuf,

    /// Output file path (defaults to `<input>.<ext>`; `-` writes to stdout).
    pub output: Option<PathBuf>,

    /// Output format: pdf, svg, or html (inferred from extension if omitted).
    #[arg(short = 'f', long)]
    pub format: Option<OutputFormat>,

    /// Project root directory for relative `#include` / `#image` paths.
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// Inject a string `KEY=VALUE` into Typst `sys.inputs` (repeatable).
    #[arg(long = "input", value_name = "KEY=VALUE", value_parser = parse_key_val)]
    pub inputs: Vec<(String, String)>,

    /// Evaluate a Wolfram expression over WSTP and inject the converted Typst
    /// value into `sys.inputs.<KEY>` (repeatable).
    #[arg(long = "wl-input", value_name = "KEY=WL_EXPR", value_parser = parse_key_val)]
    pub wl_inputs: Vec<(String, String)>,

    /// Wolfram Language prelude executed in the kernel session before `--wl-input`.
    #[arg(long = "wl-prelude", value_name = "WL_CODE")]
    pub wl_prelude: Option<String>,

    /// Page ranges to export (e.g. `1,3-5`).
    #[arg(long, value_name = "RANGES", value_parser = compile::parse_page_str)]
    pub pages: Option<PageRanges>,

    /// PDF standard(s) to enforce (repeatable, e.g. `a-2b`, `ua-1`).
    #[arg(long = "pdf-standard", value_name = "STD", value_parser = compile::parse_pdf_standard)]
    pub pdf_standards: Vec<PdfStandard>,

    /// Disable tagged PDF accessibility structure.
    #[arg(long)]
    pub no_pdf_tags: bool,

    /// Fixed Unix creation timestamp (seconds since epoch).
    #[arg(long, value_name = "UNIX")]
    pub creation_timestamp: Option<i64>,

    /// Experimental Typst features to enable (repeatable: `html`, `bundle`, `a11y-extras`).
    #[arg(long = "features", value_name = "FEATURE", value_parser = compile::parse_feature)]
    pub features: Vec<Feature>,

    /// Evaluate Typst code expression(s) against the compiled document and print results.
    #[arg(long = "evaluate", value_name = "TYPST_CODE")]
    pub evaluate: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum EvalFormat {
    /// Print raw Wolfram Language expression form.
    #[default]
    Wl,
    /// Convert the result to a Typst value via `matheme-wl` and print its Typst repr.
    Typst,
}

#[derive(Debug, Args)]
pub struct EvalCmd {
    /// Wolfram Language expression(s) to evaluate; omit or pass `-` to read lines from stdin.
    pub expressions: Vec<String>,

    /// Optional Wolfram Language prelude evaluated before the expressions.
    #[arg(long, value_name = "WL_CODE")]
    pub prelude: Option<String>,

    /// Output representation for evaluated values.
    #[arg(short = 'f', long, value_enum, default_value_t = EvalFormat::Wl)]
    pub format: EvalFormat,
}

#[derive(Debug, Args)]
pub struct FontsCmd {
    /// Case-insensitive substring filter on font family name.
    #[arg(long)]
    pub family: Option<String>,

    /// Filter by font source: custom, host, embedded, or system.
    #[arg(long)]
    pub source: Option<FontOrigin>,

    /// Filter to faces whose coverage contains this single character.
    #[arg(long)]
    pub character: Option<char>,

    /// Maximum number of faces to display (ignored with `--all`).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,

    /// Offset into matched faces.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Show all matched faces without paging cap.
    #[arg(long)]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct PackagesCmd {
    /// Substring filter on package name.
    #[arg(long)]
    pub name: Option<String>,

    /// Filter by package namespace (e.g. `preview`).
    #[arg(long)]
    pub namespace: Option<String>,

    /// Filter by package origin: local, data, cache, or universe.
    #[arg(long)]
    pub origin: Option<PackageOriginFilter>,

    /// Filter by exact version string.
    #[arg(long)]
    pub version: Option<String>,

    /// Maximum number of packages to display (ignored with `--all`).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,

    /// Offset into matched packages.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Show all matched packages without paging cap.
    #[arg(long)]
    pub all: bool,
}

fn parse_key_val(raw: &str) -> Result<(String, String), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got {raw:?}"))?;
    if key.is_empty() {
        return Err("key before '=' cannot be empty".into());
    }
    Ok((key.to_owned(), value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compile_with_wl_inputs_and_global_flags() {
        let cli = Cli::try_parse_from([
            "matheme",
            "--no-wolfram-fonts",
            "compile",
            "doc.typ",
            "out.svg",
            "--input",
            "title=Demo",
            "--wl-prelude",
            "a = 6;",
            "--wl-input",
            "val=a * 7",
            "--pages",
            "1,3-5",
            "--evaluate",
            "sys.inputs.val",
        ])
        .expect("valid cli");

        assert!(cli.no_wolfram_fonts);
        match cli.command {
            Command::Compile(cmd) => {
                assert_eq!(cmd.input, PathBuf::from("doc.typ"));
                assert_eq!(cmd.output, Some(PathBuf::from("out.svg")));
                assert_eq!(cmd.inputs, vec![("title".into(), "Demo".into())]);
                assert_eq!(cmd.wl_prelude.as_deref(), Some("a = 6;"));
                assert_eq!(cmd.wl_inputs, vec![("val".into(), "a * 7".into())]);
                assert_eq!(compile::count_exported_pages(6, cmd.pages.as_ref()), 4);
                assert_eq!(cmd.evaluate, vec!["sys.inputs.val".to_string()]);
            }
            other => panic!("expected compile command, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_key_val_inputs() {
        assert!(parse_key_val("missing_equals").is_err());
        assert!(parse_key_val("=value").is_err());
        assert_eq!(
            parse_key_val("k=a=b").unwrap(),
            ("k".to_string(), "a=b".to_string())
        );
    }

    #[test]
    fn compile_options_deserialize_to_typst_types() {
        let cli = Cli::try_parse_from([
            "matheme",
            "compile",
            "doc.typ",
            "--format",
            "PDF",
            "--pdf-standard",
            "A-2B",
            "--features",
            "A11yExtras",
        ])
        .expect("valid CLI options");
        let Command::Compile(cmd) = cli.command else {
            panic!("expected compile command");
        };
        assert_eq!(cmd.format, Some(OutputFormat::Pdf));
        assert_eq!(cmd.pdf_standards, vec![PdfStandard::A_2b]);
        assert_eq!(cmd.features, vec![Feature::A11yExtras]);
        for (flag, value) in [
            ("--pdf-standard", "bogus"),
            ("--features", "bogus"),
            ("--pages", "3-2"),
        ] {
            assert!(Cli::try_parse_from(["matheme", "compile", "doc.typ", flag, value]).is_err());
        }
    }

    #[test]
    fn parses_font_source_and_package_origin() {
        let cli = Cli::try_parse_from(["matheme", "fonts", "--source", "embedded"])
            .expect("valid font source");
        assert!(matches!(
            cli.command,
            Command::Fonts(FontsCmd {
                source: Some(FontOrigin::Embedded),
                ..
            })
        ));
        assert!(Cli::try_parse_from(["matheme", "fonts", "--source", "bogus"]).is_err());

        let cli = Cli::try_parse_from(["matheme", "packages", "--origin", "local"])
            .expect("valid package origin");
        assert!(matches!(
            cli.command,
            Command::Packages(PackagesCmd {
                origin: Some(PackageOriginFilter::Local),
                ..
            })
        ));
        assert!(Cli::try_parse_from(["matheme", "packages", "--origin", "bogus"]).is_err());
    }
}
