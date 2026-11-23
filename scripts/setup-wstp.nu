#!/usr/bin/env nu
# Stage the WSTP DeveloperKit library into .tmp/wstp-compiler-additions so
# .cargo/config.toml can point `wstp-sys` at a consistent directory across
# platforms without renaming system files.
#
# - Windows: Wolfram 15 ships both MathLink (ml64i4s.lib) and WSTP DeveloperKit
#   (wstp64i4s.lib). Copy wstp64i4s.lib (WS* symbols).
# - macOS: Copy libWSTPi4.a.
# - Linux: Prefer libWSTP64i4.a when present. WolframEngine 15 on Linux ships
#   only libWSTP64i4.so; in that case copy libWSTP64i4.so and create a minimal
#   empty libWSTP64i4.a archive so `wstp-sys 0.6.0` succeeds while
#   `crates/matheme-wstp/build.rs` links dynamically against libWSTP64i4.so.

const ROOT = (path self | path dirname | path dirname)

def wolfram-system-id [] {
    let os = $nu.os-info.name
    let arch = $nu.os-info.arch
    match [$os, $arch] {
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
}

def sdk-subdir [install_dir: path, system_id: string] {
    $install_dir | path join "SystemFiles" "Links" "WSTP" "DeveloperKit" $system_id "CompilerAdditions"
}

def discover-sdk-dir [system_id: string] {
    if ($env.WSTP_SDK_DIR? | default "" | is-not-empty) {
        return $env.WSTP_SDK_DIR
    }

    if (which wolframscript | is-not-empty) {
        let probe = (do { "" | ^wolframscript -code 'WriteString["stdout", $InstallationDirectory]; Exit[0]' } | complete)
        if $probe.exit_code == 0 {
            let install_dir = ($probe.stdout | str trim)
            if ($install_dir | is-not-empty) {
                let candidate = (sdk-subdir $install_dir $system_id)
                if ($candidate | path exists) {
                    return $candidate
                }
            }
        }
    }

    let os = $nu.os-info.name
    let candidates = match $os {
        "windows" => {
            let roots = [
                'C:\Program Files\Wolfram Research\Wolfram'
                'C:\Program Files\Wolfram Research\Mathematica'
                'C:\Program Files\Wolfram Research\Wolfram Engine'
            ]
            $roots | each {|root|
                if ($root | path exists) {
                    ls $root | where type == dir | sort-by name --reverse | each {|d| sdk-subdir $d.name $system_id }
                } else {
                    []
                }
            } | flatten
        }
        "linux" => {
            let direct = [
                "/opt/WolframEngine"
                "/opt/Wolfram"
                "/opt/Mathematica"
                "/usr/local/Wolfram/WolframEngine"
            ] | each {|d| sdk-subdir $d $system_id }
            let versioned = ["/usr/local/Wolfram", "/opt/Wolfram"] | each {|root|
                ["Wolfram", "Mathematica", "WolframEngine"] | each {|prod|
                    let pdir = ($root | path join $prod)
                    if ($pdir | path exists) {
                        ls $pdir | where type == dir | sort-by name --reverse | each {|d| sdk-subdir $d.name $system_id }
                    } else {
                        []
                    }
                } | flatten
            } | flatten
            $direct | append $versioned
        }
        "macos" => {
            [
                "/Applications/Wolfram.app/Contents"
                "/Applications/Mathematica.app/Contents"
                "/Applications/Wolfram Engine.app/Contents"
            ] | each {|d| sdk-subdir $d $system_id }
        }
        _ => []
    }

    let found = ($candidates | where {|p| $p | path exists })
    if ($found | is-not-empty) {
        return ($found | first)
    }

    match $os {
        "windows" => (sdk-subdir 'C:\Program Files\Wolfram Research\Wolfram\15.0' $system_id)
        "linux" => (sdk-subdir "/opt/WolframEngine" $system_id)
        "macos" => (sdk-subdir "/Applications/Wolfram.app/Contents" $system_id)
        _ => ""
    }
}

def copy-if-changed [src: path, dst: path] {
    let needs_copy = if not ($dst | path exists) {
        true
    } else {
        (open --raw $src | hash sha256) != (open --raw $dst | hash sha256)
    }
    if $needs_copy {
        cp --force $src $dst
    }
}

def main [
    --sdk-dir: path # Override the WSTP DeveloperKit CompilerAdditions directory (falls back to $env.WSTP_SDK_DIR or auto-discovery)
] {
    let system_id = (wolfram-system-id)
    let dest = ($ROOT | path join ".tmp" "wstp-compiler-additions")
    let sdk = ($sdk_dir | default (discover-sdk-dir $system_id))

    mkdir $dest

    let os = $nu.os-info.name
    match $os {
        "windows" => {
            let src_lib = ($sdk | path join "wstp64i4s.lib")
            if not ($src_lib | path exists) {
                print --stderr $"wstp64i4s.lib not found in: ($sdk)"
                print --stderr "Pass --sdk-dir or set WSTP_SDK_DIR to the CompilerAdditions directory of the WSTP DeveloperKit."
                exit 1
            }
            copy-if-changed $src_lib ($dest | path join "wstp64i4s.lib")
        }
        "macos" => {
            let src_lib = ($sdk | path join "libWSTPi4.a")
            if not ($src_lib | path exists) {
                print --stderr $"libWSTPi4.a not found in: ($sdk)"
                print --stderr "Pass --sdk-dir or set WSTP_SDK_DIR to the CompilerAdditions directory of the WSTP DeveloperKit."
                exit 1
            }
            copy-if-changed $src_lib ($dest | path join "libWSTPi4.a")
        }
        "linux" => {
            let src_static = ($sdk | path join "libWSTP64i4.a")
            let src_shared = ($sdk | path join "libWSTP64i4.so")
            let dst_static = ($dest | path join "libWSTP64i4.a")
            let dst_shared = ($dest | path join "libWSTP64i4.so")

            if ($src_static | path exists) {
                copy-if-changed $src_static $dst_static
                if ($dst_shared | path exists) {
                    rm --force $dst_shared
                }
            } else if ($src_shared | path exists) {
                copy-if-changed $src_shared $dst_shared
                # Standard empty Unix `ar` archive header (`!<arch>\n`, 8 bytes)
                # so `wstp-sys 0.6.0`'s static archive check succeeds while
                # `crates/matheme-wstp/build.rs` links dynamically to libWSTP64i4.so.
                let empty_ar = ("!<arch>\n" | encode utf-8)
                let needs_stub = if not ($dst_static | path exists) {
                    true
                } else {
                    (open --raw $dst_static) != $empty_ar
                }
                if $needs_stub {
                    $empty_ar | save --force --raw $dst_static
                }
            } else {
                print --stderr $"Neither libWSTP64i4.a nor libWSTP64i4.so found in: ($sdk)"
                print --stderr "Pass --sdk-dir or set WSTP_SDK_DIR to the CompilerAdditions directory of the WSTP DeveloperKit."
                exit 1
            }
        }
    }

    let src_header = ($sdk | path join "wstp.h")
    if ($src_header | path exists) {
        copy-if-changed $src_header ($dest | path join "wstp.h")
    }

    print $"WSTP SDK ready ($system_id): ($dest) \(from ($sdk)\)"
}
