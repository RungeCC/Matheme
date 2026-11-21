#!/usr/bin/env nu
# Build the LibraryLink shared library, assemble the standalone paclet directory,
# and package the distributable .paclet archive under output/.

const ROOT = (path self | path dirname | path dirname)

def main [
    --sdk-dir: path # Override the WSTP DeveloperKit CompilerAdditions directory
    --skip-pack     # Assemble output/Matheme without invoking wolframscript CreatePacletArchive
] {
    cd $ROOT

    let setup_script = ($ROOT | path join "scripts" "setup-wstp.nu")
    if ($sdk_dir != null) {
        ^nu $setup_script --sdk-dir $sdk_dir
    } else {
        ^nu $setup_script
    }

    ^cargo build --release -p matheme

    let paclet_dir = ($ROOT | path join "output" "Matheme")
    let kernel_dir = ($paclet_dir | path join "Kernel")
    let lib_dir = ($paclet_dir | path join "LibraryResources" "Windows-x86-64")

    if ($paclet_dir | path exists) {
        rm --recursive --force $paclet_dir
    }
    mkdir $kernel_dir $lib_dir

    cp ($ROOT | path join "PacletInfo.m") ($paclet_dir | path join "PacletInfo.m")
    for file in ["LICENSE", "NOTICE"] {
        cp ($ROOT | path join $file) ($paclet_dir | path join $file)
    }
    # Apache-2.0 (Typst) and OFL-1.1 (embedded fonts) notices must travel with
    # the compiled DLL; see NOTICE for the full list of obligations.
    cp --recursive ($ROOT | path join "third-party") $paclet_dir
    for file in (glob "Kernel/*.{m,wl}") {
        cp $file $kernel_dir
    }
    cp ($ROOT | path join "target" "release" "matheme.dll") ($lib_dir | path join "matheme.dll")

    for old in (glob "output/Matheme-*.paclet") {
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
            ^wolframscript -code $wl_code
        } else {
            print --stderr "Warning: wolframscript not found; skipped .paclet packaging."
        }
    }

    print "Built paclet dir: output/Matheme"
}
