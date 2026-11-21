#!/usr/bin/env nu
# Wolfram 15 ships two WSTP SDKs. The MathLink one only has ml64i4s.lib (ML*
# symbols), while the WSTP DeveloperKit has the real wstp64i4s.lib with the
# WS* symbols that the Rust `wstp` crate binds. Copy the latter into .tmp/ and
# point .cargo/config.toml at it, so no library renaming or linker aliases are
# needed.

const ROOT = (path self | path dirname | path dirname)
const DEFAULT_SDK = 'C:\Program Files\Wolfram Research\Wolfram\15.0\SystemFiles\Links\WSTP\DeveloperKit\Windows-x86-64\CompilerAdditions'

def main [
    --sdk-dir: path # Override the WSTP DeveloperKit CompilerAdditions directory (falls back to $env.WSTP_SDK_DIR)
] {
    let dest = ($ROOT | path join ".tmp" "wstp-compiler-additions")
    let sdk = ($sdk_dir | default ($env.WSTP_SDK_DIR? | default $DEFAULT_SDK))

    let src_lib = ($sdk | path join "wstp64i4s.lib")
    if not ($src_lib | path exists) {
        print --stderr $"wstp64i4s.lib not found in: ($sdk)"
        print --stderr "Pass --sdk-dir or set WSTP_SDK_DIR to the CompilerAdditions directory of the WSTP DeveloperKit."
        exit 1
    }

    mkdir $dest
    let dst_lib = ($dest | path join "wstp64i4s.lib")

    # Replace a stale copy from an older setup (e.g. the ML* alias hack).
    let needs_copy = if not ($dst_lib | path exists) {
        true
    } else {
        (open --raw $src_lib | hash sha256) != (open --raw $dst_lib | hash sha256)
    }

    if $needs_copy {
        cp --force $src_lib $dst_lib
    }

    print $"WSTP SDK ready: ($dest)"
}
