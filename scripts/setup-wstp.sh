#!/usr/bin/env bash
# Wolfram 15 ships two WSTP SDKs. The MathLink one only has ml64i4s.lib (ML*
# symbols), while the WSTP DeveloperKit has the real wstp64i4s.lib with the
# WS* symbols that the Rust `wstp` crate binds. Copy the latter into .tmp/ and
# point .cargo/config.toml at it, so no library renaming or linker aliases are
# needed.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/.tmp/wstp-compiler-additions"
sdk="${WSTP_SDK_DIR:-/c/Program Files/Wolfram Research/Wolfram/15.0/SystemFiles/Links/WSTP/DeveloperKit/Windows-x86-64/CompilerAdditions}"

if [ ! -f "$sdk/wstp64i4s.lib" ]; then
  echo "wstp64i4s.lib not found in: $sdk" >&2
  echo "Set WSTP_SDK_DIR to the CompilerAdditions directory of the WSTP DeveloperKit." >&2
  exit 1
fi

mkdir -p "$dest"
# Replace a stale copy from an older setup (e.g. the ML* alias hack).
if [ ! -f "$dest/wstp64i4s.lib" ] || ! cmp -s "$sdk/wstp64i4s.lib" "$dest/wstp64i4s.lib"; then
  rm -f "$dest/wstp64i4s.lib"
  cp -l "$sdk/wstp64i4s.lib" "$dest/wstp64i4s.lib" 2>/dev/null || \
    cp "$sdk/wstp64i4s.lib" "$dest/wstp64i4s.lib"
fi

echo "WSTP SDK ready: $dest"
