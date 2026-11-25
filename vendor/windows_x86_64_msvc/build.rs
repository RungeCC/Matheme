//! Supply `windows.lib` for `windows` 0.32 without checking a binary into git.
//!
//! The import library shipped with `windows_x86_64_msvc` 0.32.0 binds
//! `WaitOnAddress`/`WakeByAddress*` to `vertdll.dll`, which crashes libtest on
//! Windows (microsoft/windows-rs#2027). The bug was fixed upstream in 0.42.x,
//! so we reuse that official, corrected library: it is pulled in as a
//! host-gated build-dependency and copied into `OUT_DIR` here.
//!
//! `windows` 0.32 only depends on this crate for x86_64 Windows MSVC targets,
//! so on other OSes it is not in the graph at all. The `TARGET` check below
//! is defense in depth (e.g. if the crate is built directly).

use std::path::PathBuf;

const FIXED_CRATE: &str = "windows_x86_64_msvc-0.42.2";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_HOME");

    let target = std::env::var("TARGET").unwrap_or_default();
    if target != "x86_64-pc-windows-msvc" && target != "x86_64-uwp-windows-msvc" {
        return;
    }

    let source = find_fixed_lib().unwrap_or_else(|| {
        panic!(
            "cannot find lib/windows.lib from {FIXED_CRATE} in Cargo's registry cache; \
             run `cargo fetch` and retry"
        )
    });

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is not set"));
    let target_lib = out_dir.join("windows.lib");
    std::fs::copy(&source, &target_lib).unwrap_or_else(|err| {
        panic!("failed to copy {} to {}: {err}", source.display(), target_lib.display())
    });

    // `windows` bindings contain `#[link(name = "windows")]`; this is how the
    // upstream crate exposes the import library, and it works the same here.
    println!("cargo:rustc-link-search=native={}", out_dir.display());
}

fn find_fixed_lib() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
                .map(|home| home.join(".cargo"))
        })?;

    for registry in std::fs::read_dir(home.join("registry/src")).ok()?.flatten() {
        let candidate = registry.path().join(FIXED_CRATE).join("lib/windows.lib");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
