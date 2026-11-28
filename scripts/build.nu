#!/usr/bin/env nu
# Build the LibraryLink shared library, assemble the standalone paclet directory,
# and package the distributable .paclet archive under output/.

const ROOT = (path self | path dirname | path dirname)

def platform-info [] {
    let os = $nu.os-info.name
    let arch = $nu.os-info.arch
    let system_id = match [$os, $arch] {
        ["windows", "x86_64"] => "Windows-x86-64"
        ["linux", "x86_64"] => "Linux-x86-64"
        ["linux", "aarch64"] => "Linux-ARM64"
        ["macos", "x86_64"] => "MacOSX-x86-64"
        ["macos", "aarch64"] => "MacOSX-ARM64"
        _ => {
            print --stderr $"Unsupported host platform: ($os)/($arch)"
            exit 1
        }
    }
    let cargo_lib = match $os {
        "windows" => "matheme.dll"
        "linux" => "libmatheme.so"
        "macos" => "libmatheme.dylib"
    }
    # Wolfram's FindLibrary["matheme"] looks for matheme.{dll,so,dylib}
    # (without Cargo's Unix `lib` prefix).
    let wl_lib = match $os {
        "windows" => "matheme.dll"
        "linux" => "matheme.so"
        "macos" => "matheme.dylib"
    }
    {
        system_id: $system_id
        cargo_lib: $cargo_lib
        wl_lib: $wl_lib
    }
}

# Rust target triples accepted by --target, carrying the same facts
# platform-info derives from the host.
def rust-target-info [target: string] {
    match $target {
        "x86_64-pc-windows-msvc" => {
            let cargo_lib = "matheme.dll"
            let wl_lib = "matheme.dll"
            { system_id: "Windows-x86-64", cargo_lib: $cargo_lib, wl_lib: $wl_lib }
        }
        "x86_64-unknown-linux-gnu" => {
            let cargo_lib = "libmatheme.so"
            let wl_lib = "matheme.so"
            { system_id: "Linux-x86-64", cargo_lib: $cargo_lib, wl_lib: $wl_lib }
        }
        "aarch64-unknown-linux-gnu" => {
            let cargo_lib = "libmatheme.so"
            let wl_lib = "matheme.so"
            { system_id: "Linux-ARM64", cargo_lib: $cargo_lib, wl_lib: $wl_lib }
        }
        "aarch64-apple-darwin" => {
            let cargo_lib = "libmatheme.dylib"
            let wl_lib = "matheme.dylib"
            { system_id: "MacOSX-ARM64", cargo_lib: $cargo_lib, wl_lib: $wl_lib }
        }
        "x86_64-apple-darwin" => {
            let cargo_lib = "libmatheme.dylib"
            let wl_lib = "matheme.dylib"
            { system_id: "MacOSX-x86-64", cargo_lib: $cargo_lib, wl_lib: $wl_lib }
        }
        _ => {
            print --stderr $"Unsupported rust target: ($target)"
            exit 1
        }
    }
}

def main [
    --sdk-dir: path # Override the WSTP DeveloperKit CompilerAdditions directory
    --skip-pack     # Assemble output/Matheme without packaging a .paclet
    --jobs (-j): int # Limit parallel Cargo build jobs (useful on memory-constrained systems)
    --target: string # Rust target triple to cross-compile (e.g. x86_64-apple-darwin); the library lands in target/<triple>/release
    --pregenerated: path # Use checked-in completions from this directory instead of a Wolfram kernel
] {
    cd $ROOT

    let platform = (if ($target == null) { platform-info } else { rust-target-info $target })

    let setup_script = ($ROOT | path join "scripts" "setup-wstp.nu")
    if ($sdk_dir != null) {
        ^nu $setup_script --sdk-dir $sdk_dir
    } else {
        ^nu $setup_script
    }

    # Cargo builds the library and runs the schema generator; it writes both
    # ArgumentTypes.wl (actions/completions) and TypstForm/NativeCapabilities.wl
    # (native config fields) from the Rust-side declarations. The schema
    # generator is a host binary and never carries --target.
    if ($target == null) {
        if ($jobs != null) {
            ^cargo build --release -p matheme -j $jobs
            ^cargo run --quiet --release -p matheme --bin matheme-option-schema -j $jobs
        } else {
            ^cargo build --release -p matheme
            ^cargo run --quiet --release -p matheme --bin matheme-option-schema
        }
    } else {
        if ($jobs != null) {
            ^cargo build --release -p matheme --target $target -j $jobs
            ^cargo run --quiet --release -p matheme --bin matheme-option-schema -j $jobs
        } else {
            ^cargo build --release -p matheme --target $target
            ^cargo run --quiet --release -p matheme --bin matheme-option-schema
        }
    }

    # The Rust renderer reads its lookup tables from a generated snapshot of
    # Kernel/TypstForm/{Characters,Boxes}.wl. A stale snapshot would make the
    # two engines diverge silently, so the generator's recorded source hash is
    # compared against the current files. Regenerate with:
    #   wolframscript -file scripts/generate-box-tables.wl
    let tables_file = ($ROOT | path join "crates" "matheme-box" "src" "tables.json")
    let recorded = (open $tables_file | get "$typstTableSourceHash" | default "")
    if ($recorded == "") {
        print --stderr "warning: tables.json carries no $typstTableSourceHash; run wolframscript -file scripts/generate-box-tables.wl"
    } else {
        let current = [
            (open --raw ($ROOT | path join "Kernel" "TypstForm" "Characters.wl") | hash md5)
            (open --raw ($ROOT | path join "Kernel" "TypstForm" "Boxes.wl") | hash md5)
        ] | str join ":"
        if ($recorded != $current) {
            print --stderr "tables.json is stale: it mirrors older Kernel/TypstForm/Characters.wl or Boxes.wl."
            print --stderr "Regenerate it with: wolframscript -file scripts/generate-box-tables.wl"
            exit 1
        }
    }

    let paclet_dir = ($ROOT | path join "output" "Matheme")
    let lib_dir = ($paclet_dir | path join "LibraryResources" $platform.system_id)

    if ($paclet_dir | path exists) {
        rm --recursive --force $paclet_dir
    }
    mkdir $lib_dir

    # FrontEnd completions are written straight into the assembled paclet.
    # They come from the loaded package, the generated ArgumentTypes.wl, and
    # the WL-only positional data in CompletionData.wl.
    if ($pregenerated == null) {
        if (which wolframscript | is-empty) {
            print --stderr "wolframscript is required to generate specialArgFunctions.tr."
            exit 1
        }
        ^wolframscript -file ($ROOT | path join "scripts" "generate-completions.wl")
    } else {
        # CI has no activated kernel: consume the table produced locally by
        # `nu scripts/precompile-completions.nu`, after checking it still
        # matches the sources it was derived from.
        let manifest_file = ($pregenerated | path join "specialArgFunctions.manifest.json")
        let table_file = ($pregenerated | path join "specialArgFunctions.tr")
        if not ($manifest_file | path exists) or not ($table_file | path exists) {
            print --stderr $"missing pregenerated completions under ($pregenerated)"
            exit 1
        }
        let manifest = (open $manifest_file)
        for rel in ["Kernel/ArgumentTypes.wl" "Kernel/CompletionData.wl" "Kernel/Matheme.wl"] {
            let recorded = ($manifest.sources | get $rel)
            let actual = (open --raw ($ROOT | path join $rel) | hash sha256)
            if ($recorded != $actual) {
                print --stderr $"pregenerated completions are stale: ($rel) changed."
                print --stderr "Regenerate with: nu scripts/precompile-completions.nu"
                exit 1
            }
        }
        let completions_dir = ($paclet_dir | path join "AutoCompletionData")
        mkdir $completions_dir
        cp $table_file ($completions_dir | path join "specialArgFunctions.tr")
    }

    cp ($ROOT | path join "PacletInfo.m") ($paclet_dir | path join "PacletInfo.m")
    for file in ["LICENSE", "NOTICE"] {
        cp ($ROOT | path join $file) ($paclet_dir | path join $file)
    }
    # Apache-2.0 (Typst) and OFL-1.1 (embedded fonts) notices must travel with
    # the compiled shared library; see NOTICE for the full list of obligations.
    cp --recursive ($ROOT | path join "third-party") $paclet_dir
    # Copy Kernel/ including subdirectories such as Kernel/TypstForm/.
    cp --recursive ($ROOT | path join "Kernel") $paclet_dir
    let release_dir = (if ($target == null) {
        $ROOT | path join "target" "release"
    } else {
        $ROOT | path join "target" $target "release"
    })
    cp ($release_dir | path join $platform.cargo_lib) ($lib_dir | path join $platform.wl_lib)

    for old in (glob "output/*Matheme-*.paclet") {
        rm --force $old
    }

    if (not $skip_pack) {
        if ($pregenerated != null) {
            # No kernel on this path: pack the directory with a plain zip.
            let python = (if (which python3 | is-not-empty) { "python3" } else { "python" })
            ^$python ($ROOT | path join "scripts" "pack-paclet.py") --src $paclet_dir --dst ($ROOT | path join "output")
        } else if (which wolframscript | is-not-empty) {
            let wl_code = '
                src = FileNameJoin[{Directory[], "output", "Matheme"}];
                dst = FileNameJoin[{Directory[], "output"}];
                res = CreatePacletArchive[src, dst];
                If[FailureQ[res] || res === $Failed, Exit[1]];
                Print["Packed: ", res]
            '
            "" | ^wolframscript -code $wl_code
        } else {
            print --stderr "Warning: wolframscript not found; skipped .paclet packaging."
        }
    }

    print $"Built paclet dir: output/Matheme \(($platform.system_id)\)"
}
