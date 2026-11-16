#!/usr/bin/env bash
# Build the LibraryLink shared library and install it into the paclet layout.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bash scripts/setup-wstp.sh
cargo build --release
mkdir -p LibraryResources/Windows-x86-64
cp target/release/matheme.dll LibraryResources/Windows-x86-64/matheme.dll

echo "Built: LibraryResources/Windows-x86-64/matheme.dll"
