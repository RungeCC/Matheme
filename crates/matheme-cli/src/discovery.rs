//! Active discovery of local Wolfram Language installations and kernels.
//!
//! Resolution order:
//! 1. Explicit `--kernel <PATH>` CLI flag
//! 2. `MATHEME_WOLFRAM_KERNEL` / `WOLFRAM_KERNEL` environment variables
//! 3. `wolfram-app-discovery` (standard OS installation paths / registry)
//! 4. `WolframKernel` / `MathKernel` executables on `PATH`

use std::path::{Path, PathBuf};

use wolfram_app_discovery::WolframApp;

#[derive(Debug, Clone)]
pub struct KernelCandidate {
    pub kernel_path: PathBuf,
    pub install_dir: Option<PathBuf>,
    pub font_dir: Option<PathBuf>,
    pub version: Option<String>,
    pub app_name: String,
    pub source: &'static str,
}

fn bundled_font_dir(install_dir: &Path) -> Option<PathBuf> {
    let dir = install_dir.join("SystemFiles").join("Fonts").join("TrueType");
    dir.is_dir().then_some(dir)
}

fn infer_install_dir(kernel_path: &Path) -> Option<PathBuf> {
    // On Windows/Linux, WolframKernel sits directly in `<install_dir>` (or `<install_dir>/Executables`).
    let parent = kernel_path.parent()?;
    if parent.join("SystemFiles").is_dir() {
        return Some(parent.to_path_buf());
    }
    let grandparent = parent.parent()?;
    if grandparent.join("SystemFiles").is_dir() {
        return Some(grandparent.to_path_buf());
    }
    None
}

fn candidate_from_path(kernel_path: PathBuf, source: &'static str) -> KernelCandidate {
    let install_dir = infer_install_dir(&kernel_path);
    let font_dir = install_dir.as_deref().and_then(bundled_font_dir);
    KernelCandidate {
        kernel_path,
        install_dir,
        font_dir,
        version: None,
        app_name: "WolframKernel".into(),
        source,
    }
}

fn candidate_from_app(app: &WolframApp, source: &'static str) -> Option<KernelCandidate> {
    let kernel_path = app.kernel_executable_path().ok()?;
    let install_dir = app.installation_directory();
    let font_dir = bundled_font_dir(&install_dir);
    let version = app.wolfram_version().ok().map(|v| v.to_string());
    Some(KernelCandidate {
        kernel_path,
        install_dir: Some(install_dir),
        font_dir,
        version,
        app_name: format!("{:?}", app.app_type()),
        source,
    })
}

fn read_version_id(install_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(install_dir.join(".VersionID")).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn candidate_from_install_dir(
    install_dir: PathBuf,
    app_name: String,
    source: &'static str,
) -> Option<KernelCandidate> {
    let kernel_names: &[&str] = if cfg!(windows) {
        &["WolframKernel.exe", "MathKernel.exe"]
    } else {
        &["Executables/WolframKernel", "WolframKernel", "Executables/MathKernel", "MathKernel"]
    };
    let kernel_path = kernel_names
        .iter()
        .map(|rel| install_dir.join(rel))
        .find(|p| p.is_file())?;
    let font_dir = bundled_font_dir(&install_dir);
    let version = read_version_id(&install_dir).or_else(|| {
        install_dir
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
    });
    Some(KernelCandidate {
        kernel_path,
        install_dir: Some(install_dir),
        font_dir,
        version,
        app_name,
        source,
    })
}

/// Scan standard OS installation directories (including Wolfram 14.1+/15.0's
/// `Wolfram Research\Wolfram\<ver>` layout that older `wolfram-app-discovery`
/// versions do not know about).
fn find_in_standard_dirs() -> Vec<KernelCandidate> {
    let mut found = Vec::new();

    #[cfg(windows)]
    {
        let roots = ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|p| PathBuf::from(p).join("Wolfram Research"));

        let products = ["Wolfram", "Mathematica", "Wolfram Engine", "Wolfram Desktop"];
        for root in roots {
            for product in products {
                let product_dir = root.join(product);
                let Ok(entries) = std::fs::read_dir(&product_dir) else {
                    continue;
                };
                let mut versions: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                // Prefer highest version directory first.
                versions.sort();
                versions.reverse();
                for dir in versions {
                    if let Some(c) = candidate_from_install_dir(
                        dir,
                        product.to_owned(),
                        "standard install dir",
                    ) {
                        found.push(c);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for (app, name) in [
            ("/Applications/Wolfram.app/Contents", "Wolfram"),
            ("/Applications/Mathematica.app/Contents", "Mathematica"),
            ("/Applications/Wolfram Engine.app/Contents", "Wolfram Engine"),
        ] {
            let dir = PathBuf::from(app);
            let kernel = dir.join("MacOS").join("WolframKernel");
            if kernel.is_file() {
                let font_dir = bundled_font_dir(&dir);
                let version = read_version_id(&dir);
                found.push(KernelCandidate {
                    kernel_path: kernel,
                    install_dir: Some(dir),
                    font_dir,
                    version,
                    app_name: name.to_owned(),
                    source: "standard install dir",
                });
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for root in ["/usr/local/Wolfram", "/opt/Wolfram"] {
            for product in ["Wolfram", "Mathematica", "WolframEngine"] {
                let product_dir = Path::new(root).join(product);
                let Ok(entries) = std::fs::read_dir(&product_dir) else {
                    continue;
                };
                let mut versions: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                versions.sort();
                versions.reverse();
                for dir in versions {
                    if let Some(c) = candidate_from_install_dir(
                        dir,
                        product.to_owned(),
                        "standard install dir",
                    ) {
                        found.push(c);
                    }
                }
            }
        }
    }

    found
}

fn find_on_path() -> Vec<KernelCandidate> {
    let mut found = Vec::new();
    let Some(path_var) = std::env::var_os("PATH") else {
        return found;
    };
    let names: &[&str] = if cfg!(windows) {
        &["WolframKernel.exe", "MathKernel.exe"]
    } else {
        &["WolframKernel", "MathKernel"]
    };

    for dir in std::env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                found.push(candidate_from_path(candidate, "PATH"));
            }
        }
    }
    found
}

/// Discover all available Wolfram kernels on this machine, deduplicated by path.
pub fn discover_kernels(explicit: Option<&Path>) -> Vec<KernelCandidate> {
    let mut candidates = Vec::new();

    if let Some(path) = explicit {
        candidates.push(candidate_from_path(path.to_path_buf(), "--kernel"));
    }

    for var in ["MATHEME_WOLFRAM_KERNEL", "WOLFRAM_KERNEL"] {
        if let Some(val) = std::env::var_os(var) {
            if !val.is_empty() {
                candidates.push(candidate_from_path(PathBuf::from(val), var));
            }
        }
    }

    if let Ok(default_app) = WolframApp::try_default() {
        if let Some(c) = candidate_from_app(&default_app, "wolfram-app-discovery (default)") {
            candidates.push(c);
        }
    }

    for app in wolfram_app_discovery::discover() {
        if let Some(c) = candidate_from_app(&app, "wolfram-app-discovery") {
            candidates.push(c);
        }
    }

    candidates.extend(find_in_standard_dirs());
    candidates.extend(find_on_path());

    // Deduplicate by canonicalized (or raw) kernel path, keeping highest-priority source.
    let mut seen = Vec::<PathBuf>::new();
    candidates.retain(|c| {
        let key = std::fs::canonicalize(&c.kernel_path).unwrap_or_else(|_| c.kernel_path.clone());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });

    candidates
}

/// Resolve the single best Wolfram kernel candidate to connect to.
pub fn resolve_kernel(explicit: Option<&Path>) -> Result<KernelCandidate, String> {
    if let Some(path) = explicit {
        if !path.is_file() {
            return Err(format!(
                "specified Wolfram kernel does not exist: {}",
                path.display()
            ));
        }
        return Ok(candidate_from_path(path.to_path_buf(), "--kernel"));
    }

    discover_kernels(None).into_iter().next().ok_or_else(|| {
        "no Wolfram kernel found; install Wolfram/Mathematica or pass --kernel <PATH>".into()
    })
}

/// Collect bundled Wolfram font directories from the preferred installation and
/// register them with `matheme_world` as host font paths.
pub fn configure_wolfram_fonts(explicit_kernel: Option<&Path>) -> Option<PathBuf> {
    let candidate = discover_kernels(explicit_kernel)
        .into_iter()
        .find(|c| c.font_dir.is_some())?;
    let font_dir = candidate.font_dir?;
    matheme_world::set_host_font_paths(vec![font_dir.clone()]);
    Some(font_dir)
}
