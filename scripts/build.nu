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

def main [
    --sdk-dir: path # Override the WSTP DeveloperKit CompilerAdditions directory
    --skip-pack     # Assemble output/Matheme without invoking wolframscript CreatePacletArchive
    --jobs (-j): int # Limit parallel Cargo build jobs (useful on memory-constrained systems)
] {
    cd $ROOT

    let platform = (platform-info)

    let setup_script = ($ROOT | path join "scripts" "setup-wstp.nu")
    if ($sdk_dir != null) {
        ^nu $setup_script --sdk-dir $sdk_dir
    } else {
        ^nu $setup_script
    }

    if ($jobs != null) {
        ^cargo build --release -p matheme --features option-schema -j $jobs
    } else {
        ^cargo build --release -p matheme --features option-schema
    }
    let generator = if $nu.os-info.name == "windows" {
        "matheme-option-schema.exe"
    } else {
        "matheme-option-schema"
    }
    ^($ROOT | path join "target" "release" $generator)

    # FrontEnd completions are generated from the WL sources (Core.wl
    # $displayNames, the generated ArgumentTypes.wl, and WL option defaults).
    if (which wolframscript | is-empty) {
        print --stderr "wolframscript is required to generate specialArgFunctions.tr."
        exit 1
    }
    ^wolframscript -file ($ROOT | path join "scripts" "generate-completions.wl")

    let paclet_dir = ($ROOT | path join "output" "Matheme")
    let lib_dir = ($paclet_dir | path join "LibraryResources" $platform.system_id)

    if ($paclet_dir | path exists) {
        rm --recursive --force $paclet_dir
    }
    mkdir $lib_dir

    cp ($ROOT | path join "PacletInfo.m") ($paclet_dir | path join "PacletInfo.m")
    for file in ["LICENSE", "NOTICE"] {
        cp ($ROOT | path join $file) ($paclet_dir | path join $file)
    }
    # Apache-2.0 (Typst) and OFL-1.1 (embedded fonts) notices must travel with
    # the compiled shared library; see NOTICE for the full list of obligations.
    cp --recursive ($ROOT | path join "third-party") $paclet_dir
    # Copy Kernel/ including subdirectories such as Kernel/TypstForm/.
    cp --recursive ($ROOT | path join "Kernel") $paclet_dir
    # FrontEnd completion data (specialArgFunctions.tr); declared by the
    # AutoCompletionData extension in PacletInfo.m.
    cp --recursive ($ROOT | path join "AutoCompletionData") $paclet_dir
    cp ($ROOT | path join "target" "release" $platform.cargo_lib) ($lib_dir | path join $platform.wl_lib)

    for old in (glob "output/*Matheme-*.paclet") {
        rm --force $old
    }

    if (not $skip_pack) {
        if (which wolframscript | is-not-empty) {
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
