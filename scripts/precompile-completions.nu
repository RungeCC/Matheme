#!/usr/bin/env nu
# Regenerate the checked-in FrontEnd completion table that CI consumes.
#
# Run this locally (with an activated Wolfram kernel) before pushing changes to
# Kernel/Matheme.wl, Kernel/CompletionData.wl, or the Rust option schema. CI
# never starts a Wolfram kernel: it only verifies the recorded source hashes
# and copies the table into the assembled paclet.

const ROOT = (path self | path dirname | path dirname)
const OUT = ($ROOT | path join ".github" "pregenerated")

def main [] {
    cd $ROOT

    if (which wolframscript | is-empty) {
        print --stderr "wolframscript is required to precompile completions."
        exit 1
    }

    if not (($ROOT | path join "Kernel" "ArgumentTypes.wl") | path exists) {
        print --stderr "Kernel/ArgumentTypes.wl is missing."
        print --stderr "Generate it first with: cargo run --release -p matheme --bin matheme-option-schema"
        exit 1
    }

    mkdir $OUT
    with-env { MATHEME_COMPLETIONS_OUT: ($OUT | into string) } {
        ^wolframscript -file ($ROOT | path join "scripts" "generate-completions.wl")
    }

    print $"Pregenerated completions in ($OUT)"
}
