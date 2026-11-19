//! Entry point for `matheme` CLI — a Typst high-level extension that actively
//! discovers and communicates with a Wolfram Kernel over WSTP.

mod cli;
mod discovery;
mod kernel;

use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use matheme_world::compile::{
    self, CompileOutput, CompileRequest, EvaluateSpec, EvaluatedValues, OutputFormat, Timestamp,
};
use matheme_world::query::{FontFilter, FontOrigin, PackageFilter, PackageOriginFilter, Page};
use matheme_world::settings::{Patch, Update};
use matheme_world::{datetime_from_unix, WorldDate};
use typst::foundations::{Dict, Repr, Str, Value};

use crate::cli::{
    Cli, Command, CompileCmd, EvalCmd, EvalFormat, FontSourceArg, FontsCmd, FormatArg,
    PackageOriginArg, PackagesCmd,
};
use crate::kernel::KernelSession;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn apply_world_settings(cli: &Cli, with_wolfram_fonts: bool) {
    let mut patch = Patch::default();
    if !cli.font_paths.is_empty() {
        patch.font_paths = Update::Set(cli.font_paths.clone());
    }
    if cli.ignore_system_fonts {
        patch.system_fonts = Update::Set(false);
    }
    if let Some(ref path) = cli.package_path {
        patch.package_data = Update::Set(path.clone());
    }
    if let Some(ref cache) = cli.package_cache {
        patch.package_cache = Update::Set(cache.clone());
    }
    if let Some(ref proxy) = cli.proxy {
        patch.proxy = Update::Set(proxy.clone());
    }
    if cli.allow_downloads {
        patch.downloads = Update::Set(true);
    }
    matheme_world::apply(&patch);

    if with_wolfram_fonts && !cli.no_wolfram_fonts {
        let _ = discovery::configure_wolfram_fonts(cli.kernel.as_deref());
    }
}

fn open_session(cli: &Cli) -> Result<KernelSession, String> {
    if let Some(ref link_name) = cli.link_name {
        return KernelSession::connect(link_name).map_err(|e| e.to_string());
    }
    let candidate = discovery::resolve_kernel(cli.kernel.as_deref())?;
    KernelSession::launch(&candidate.kernel_path).map_err(|e| e.to_string())
}

fn run(cli: Cli) -> Result<(), String> {
    match &cli.command {
        Command::Kernels => cmd_kernels(cli.kernel.as_deref()),
        Command::Version => cmd_version(cli.kernel.as_deref()),
        Command::Eval(cmd) => cmd_eval(&cli, cmd),
        Command::Fonts(cmd) => {
            apply_world_settings(&cli, true);
            cmd_fonts(cmd)
        }
        Command::Packages(cmd) => {
            apply_world_settings(&cli, false);
            cmd_packages(&cli, cmd)
        }
        Command::Compile(cmd) => {
            apply_world_settings(&cli, true);
            cmd_compile(&cli, cmd)
        }
    }
}

fn cmd_kernels(explicit: Option<&Path>) -> Result<(), String> {
    let candidates = discovery::discover_kernels(explicit);
    if candidates.is_empty() {
        println!("No Wolfram kernels found.");
        return Ok(());
    }
    for (idx, c) in candidates.iter().enumerate() {
        let marker = if idx == 0 { "*" } else { " " };
        let version = c.version.as_deref().unwrap_or("unknown");
        println!(
            "{marker} {} ({version}) [{}]",
            c.app_name, c.source
        );
        println!("    kernel: {}", c.kernel_path.display());
        if let Some(ref dir) = c.install_dir {
            println!("    install: {}", dir.display());
        }
        if let Some(ref fonts) = c.font_dir {
            println!("    fonts: {}", fonts.display());
        }
    }
    Ok(())
}

fn cmd_version(explicit: Option<&Path>) -> Result<(), String> {
    let typst_ver = typst::utils::version();
    println!("matheme-cli {}", env!("CARGO_PKG_VERSION"));
    println!(
        "typst {} ({})",
        typst_ver.raw(),
        typst_ver.commit().unwrap_or("unknown")
    );
    match discovery::resolve_kernel(explicit) {
        Ok(c) => {
            let ver = c.version.as_deref().unwrap_or("unknown");
            println!(
                "wolfram {} ({}) at {}",
                c.app_name,
                ver,
                c.kernel_path.display()
            );
        }
        Err(_) => println!("wolfram: not detected"),
    }
    Ok(())
}

fn print_eval_sideband(output: &kernel::EvalOutput) {
    for msg in &output.messages {
        eprintln!("wolfram message: {msg}");
    }
    for line in &output.prints {
        eprintln!("{line}");
    }
}

fn cmd_eval(cli: &Cli, cmd: &EvalCmd) -> Result<(), String> {
    let mut session = open_session(cli)?;

    if let Some(ref prelude) = cmd.prelude {
        let out = session.eval_code(prelude).map_err(|e| e.to_string())?;
        print_eval_sideband(&out);
    }

    let read_stdin = cmd.expressions.is_empty()
        || (cmd.expressions.len() == 1 && cmd.expressions[0] == "-");

    let run_one = |session: &mut KernelSession, code: &str| -> Result<(), String> {
        match cmd.format {
            EvalFormat::Wl => {
                let out = session.eval_code(code).map_err(|e| e.to_string())?;
                print_eval_sideband(&out);
                println!("{}", out.expr);
            }
            EvalFormat::Typst => {
                let (val, warnings, out) = session.eval_to_typst(code).map_err(|e| e.to_string())?;
                print_eval_sideband(&out);
                for w in warnings {
                    eprintln!("warning: {w}");
                }
                println!("{}", val.repr());
            }
        }
        Ok(())
    };

    if read_stdin {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line.map_err(|e| format!("failed to read stdin: {e}"))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            run_one(&mut session, trimmed)?;
        }
    } else {
        for expr in &cmd.expressions {
            run_one(&mut session, expr)?;
        }
    }

    session.close();
    Ok(())
}

fn cmd_fonts(cmd: &FontsCmd) -> Result<(), String> {
    let origin = cmd.source.map(|src| match src {
        FontSourceArg::Custom => FontOrigin::Custom,
        FontSourceArg::Host => FontOrigin::Host,
        FontSourceArg::Embedded => FontOrigin::Embedded,
        FontSourceArg::System => FontOrigin::System,
    });
    let filter = FontFilter {
        family: cmd.family.clone(),
        origin,
        resident: None,
        character: cmd.character,
    };
    let page = Page {
        offset: cmd.offset,
        limit: (!cmd.all).then_some(cmd.limit),
    };
    let answer = matheme_world::query::fonts(&filter, &page);
    println!(
        "Fonts: showing {} of {} matched ({} total)",
        answer.faces.len(),
        answer.matched,
        answer.total
    );
    for face in &answer.faces {
        println!(
            "  {:32} {:8} {:>4}  [{}]",
            face.family,
            face.style,
            face.weight,
            face.origin.as_str()
        );
    }
    Ok(())
}

fn cmd_packages(cli: &Cli, cmd: &PackagesCmd) -> Result<(), String> {
    if cli.allow_downloads && !matheme_world::query::universe_cached() {
        if let Err(err) = matheme_world::prefetch_universe_index() {
            eprintln!("warning: failed to fetch Universe index: {err}");
        }
    }
    let origin = cmd.origin.map(|orig| match orig {
        PackageOriginArg::Local => PackageOriginFilter::Local,
        PackageOriginArg::Data => PackageOriginFilter::Data,
        PackageOriginArg::Cache => PackageOriginFilter::Cache,
        PackageOriginArg::Universe => PackageOriginFilter::Universe,
    });
    let filter = PackageFilter {
        name: cmd.name.clone(),
        namespace: cmd.namespace.clone(),
        origin,
        version: cmd.version.clone(),
    };
    let page = Page {
        offset: cmd.offset,
        limit: (!cmd.all).then_some(cmd.limit),
    };
    let answer = matheme_world::query::packages(&filter, &page);
    println!(
        "Packages: showing {} of {} matched ({} local, {} universe)",
        answer.entries.len(),
        answer.matched,
        answer.local_total,
        answer.universe_total
    );
    for entry in &answer.entries {
        println!(
            "  @{}/{}:{}  [{}]",
            entry.namespace,
            entry.name,
            entry.version,
            entry.origin.as_str()
        );
    }
    Ok(())
}

fn resolve_format(cmd: &CompileCmd) -> OutputFormat {
    if let Some(fmt) = cmd.format {
        return match fmt {
            FormatArg::Pdf => OutputFormat::Pdf,
            FormatArg::Svg => OutputFormat::Svg,
            FormatArg::Html => OutputFormat::Html,
        };
    }
    if let Some(ref output) = cmd.output {
        if let Some(ext) = output.extension().and_then(|e| e.to_str()) {
            if let Some(fmt) = OutputFormat::parse(ext) {
                return fmt;
            }
        }
    }
    OutputFormat::Pdf
}

fn default_output_path(input: &Path, format: OutputFormat) -> PathBuf {
    let ext = match format {
        OutputFormat::Pdf => "pdf",
        OutputFormat::Svg => "svg",
        OutputFormat::Html => "html",
    };
    if input == Path::new("-") {
        PathBuf::from("-")
    } else {
        input.with_extension(ext)
    }
}

fn validate_compile_output(out_path: &Path, has_evaluate: bool) -> Result<(), String> {
    if out_path == Path::new("-") && has_evaluate {
        return Err(
            "cannot write compiled output to stdout ('-') while using --evaluate; specify an output file path"
                .into(),
        );
    }
    Ok(())
}

fn cmd_compile(cli: &Cli, cmd: &CompileCmd) -> Result<(), String> {
    let format = resolve_format(cmd);
    let out_path = cmd
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&cmd.input, format));
    validate_compile_output(&out_path, !cmd.evaluate.is_empty())?;

    let source = if cmd.input == Path::new("-") {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read Typst source from stdin: {e}"))?;
        buf
    } else {
        fs::read_to_string(&cmd.input)
            .map_err(|e| format!("failed to read {}: {e}", cmd.input.display()))?
    };

    let base_path = match cmd.root {
        Some(ref root) => root.clone(),
        None if cmd.input != Path::new("-") => cmd
            .input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => PathBuf::from("."),
    };

    let pages = match cmd.pages.as_deref() {
        Some(spec) => Some(compile::parse_page_str(spec)?),
        None => None,
    };
    let pdf_standards = compile::parse_pdf_standards(&cmd.pdf_standards)?;
    let features = compile::parse_features(&cmd.features, format)?;

    let (world_date, pdf_timestamp) = match cmd.creation_timestamp {
        Some(unix) => {
            let dt = datetime_from_unix(unix, None)
                .ok_or_else(|| "creation timestamp is out of range".to_owned())?;
            (WorldDate::Fixed(unix), Some(Timestamp::new_utc(dt)))
        }
        None => (WorldDate::System, None),
    };

    let mut inputs = Dict::new();
    for (k, v) in &cmd.inputs {
        inputs.insert(Str::from(k.as_str()), Value::Str(Str::from(v.as_str())));
    }

    let mut extra_diagnostics = Vec::new();
    if !cmd.wl_inputs.is_empty() || cmd.wl_prelude.is_some() {
        let mut session = open_session(cli)?;
        if let Some(ref prelude) = cmd.wl_prelude {
            let out = session.eval_code(prelude).map_err(|e| e.to_string())?;
            print_eval_sideband(&out);
        }
        for (key, wl_code) in &cmd.wl_inputs {
            let (val, warnings, out) = session
                .eval_to_typst(wl_code)
                .map_err(|e| format!("--wl-input {key}: {e}"))?;
            print_eval_sideband(&out);
            for w in warnings {
                extra_diagnostics.push(matheme_world::diagnostics::warning(format!(
                    "--wl-input {key}: {w}"
                )));
            }
            inputs.insert(Str::from(key.as_str()), val);
        }
        session.close();
    }

    let evaluate = if cmd.evaluate.is_empty() {
        None
    } else if cmd.evaluate.len() == 1 {
        Some(EvaluateSpec::Single(cmd.evaluate[0].clone()))
    } else {
        Some(EvaluateSpec::List(cmd.evaluate.clone()))
    };

    let request = CompileRequest {
        source,
        inputs,
        base_path,
        allow_downloads: cli.allow_downloads,
        format,
        pages,
        pdf_standards,
        pdf_tagged: !cmd.no_pdf_tags,
        pdf_timestamp,
        world_date,
        features,
        evaluate,
        extra_diagnostics,
    };

    match compile::compile(request) {
        Ok(success) => {
            if !success.formatted.is_empty() {
                eprint!("{}", success.formatted);
            }

            write_output(&out_path, &success.output)?;

            if let Some(evaluated) = success.evaluated {
                match evaluated {
                    EvaluatedValues::Single(v) => println!("{}", v.repr()),
                    EvaluatedValues::List(vs) => {
                        for v in vs {
                            println!("{}", v.repr());
                        }
                    }
                    EvaluatedValues::Map(pairs) => {
                        for (k, v) in pairs {
                            println!("{k} = {}", v.repr());
                        }
                    }
                }
            }
            Ok(())
        }
        Err(failure) => {
            if !failure.formatted.is_empty() {
                eprint!("{}", failure.formatted);
            }
            Err(format!(
                "Typst compilation failed with {} error(s): {}",
                failure.error_count, failure.summary
            ))
        }
    }
}

fn write_output(path: &Path, output: &CompileOutput) -> Result<(), String> {
    let to_stdout = path == Path::new("-");
    match output {
        CompileOutput::Pdf { bytes, .. } => {
            if to_stdout {
                io::stdout()
                    .write_all(bytes)
                    .map_err(|e| format!("failed to write PDF to stdout: {e}"))
            } else {
                fs::write(path, bytes)
                    .map_err(|e| format!("failed to write {}: {e}", path.display()))
            }
        }
        CompileOutput::Html { html } => {
            if to_stdout {
                io::stdout()
                    .write_all(html.as_bytes())
                    .map_err(|e| format!("failed to write HTML to stdout: {e}"))
            } else {
                fs::write(path, html)
                    .map_err(|e| format!("failed to write {}: {e}", path.display()))
            }
        }
        CompileOutput::Svg { pages, .. } => {
            if to_stdout {
                let mut stdout = io::stdout();
                for svg in pages {
                    stdout
                        .write_all(svg.as_bytes())
                        .map_err(|e| format!("failed to write SVG to stdout: {e}"))?;
                }
                Ok(())
            } else if pages.len() <= 1 {
                let content = pages.first().map(String::as_str).unwrap_or("");
                fs::write(path, content)
                    .map_err(|e| format!("failed to write {}: {e}", path.display()))
            } else {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("page");
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("svg");
                let parent = path.parent().unwrap_or_else(|| Path::new(""));
                for (idx, svg) in pages.iter().enumerate() {
                    let page_path = parent.join(format!("{stem}-{}.{ext}", idx + 1));
                    fs::write(&page_path, svg)
                        .map_err(|e| format!("failed to write {}: {e}", page_path.display()))?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_stdout_output_combined_with_evaluate() {
        assert!(validate_compile_output(Path::new("-"), true).is_err());
        assert!(validate_compile_output(Path::new("-"), false).is_ok());
        assert!(validate_compile_output(Path::new("out.pdf"), true).is_ok());
    }
}
