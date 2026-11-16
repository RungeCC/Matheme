#!/usr/bin/env bash
# Wolfram 15 renamed the WSTP static archive from wstp64i4s.lib to
# ml64i4s.lib.  wstp-sys 0.6 looks for the old name, so create a hard link
# with the expected name in .tmp/ and point .cargo/config.toml at it.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/.tmp/wstp-compiler-additions"
sdk="${WSTP_SDK_DIR:-/c/Program Files/Wolfram Research/Wolfram/15.0/SystemFiles/Links/MathLink/DeveloperKit/Windows-x86-64/CompilerAdditions}"

if [ ! -f "$sdk/ml64i4s.lib" ]; then
  echo "ml64i4s.lib not found in: $sdk" >&2
  exit 1
fi

mkdir -p "$dest"
if [ ! -f "$dest/wstp64i4s.lib" ]; then
  cp -l "$sdk/ml64i4s.lib" "$dest/wstp64i4s.lib" 2>/dev/null || \
    cp "$sdk/ml64i4s.lib" "$dest/wstp64i4s.lib"
fi

echo "WSTP SDK alias ready: $dest"
