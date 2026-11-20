#!/usr/bin/env bash
# Build the LibraryLink shared library, assemble the standalone paclet directory,
# and package the distributable .paclet archive under output/.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bash scripts/setup-wstp.sh
cargo build --release -p matheme

paclet_dir="$root/output/Matheme"
lib_dir="$paclet_dir/LibraryResources/Windows-x86-64"

rm -rf "$paclet_dir"
mkdir -p "$paclet_dir/Kernel" "$lib_dir"

cp PacletInfo.m "$paclet_dir/PacletInfo.m"
cp Kernel/*.m Kernel/*.wl "$paclet_dir/Kernel/"
cp target/release/matheme.dll "$lib_dir/matheme.dll"

rm -f "$root"/output/Matheme-*.paclet

if command -v wolframscript >/dev/null 2>&1; then
  wolframscript -code '
    src = FileNameJoin[{Directory[], "output", "Matheme"}];
    dst = FileNameJoin[{Directory[], "output"}];
    res = CreatePacletArchive[src, dst];
    If[FailureQ[res] || res === $Failed, Exit[1]];
    Print["Packed: ", res]
  '
else
  echo "Warning: wolframscript not found; skipped .paclet packaging." >&2
fi

echo "Built paclet dir: output/Matheme"
