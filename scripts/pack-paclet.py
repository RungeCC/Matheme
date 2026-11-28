#!/usr/bin/env python3
"""Package an assembled paclet directory into a .paclet archive.

CreatePacletArchive emits a plain deflate zip whose entries are rooted at
"Publisher__Name-Version/". CI does not activate a Wolfram kernel, so this
script reproduces that layout with the standard library instead.
"""

from __future__ import annotations

import argparse
import re
import sys
import time
import zipfile
from pathlib import Path


def read_paclet_info(path: Path) -> tuple[str, str, str]:
    text = path.read_text(encoding="utf-8")

    def field(name: str) -> str:
        match = re.search(rf'\b{name}\s*->\s*"([^"]+)"', text)
        if not match:
            raise SystemExit(f"pack-paclet: cannot find {name} in {path}")
        return match.group(1)

    return field("PublisherID"), field("Name"), field("Version")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--src", required=True, help="assembled paclet directory")
    parser.add_argument("--dst", required=True, help="directory for the .paclet")
    parser.add_argument("--name", help="archive stem (default: Publisher__Name-Version)")
    args = parser.parse_args()

    src = Path(args.src).resolve()
    dst = Path(args.dst).resolve()
    if not (src / "PacletInfo.m").is_file():
        raise SystemExit(f"pack-paclet: {src}/PacletInfo.m not found")

    publisher, name, version = read_paclet_info(src / "PacletInfo.m")
    root = args.name or f"{publisher}__{name.rsplit('/', 1)[-1]}-{version}"
    out = dst / f"{root}.paclet"

    dst.mkdir(parents=True, exist_ok=True)
    entries = sorted(src.rglob("*"), key=lambda p: p.relative_to(src).as_posix())
    dirs = [p for p in entries if p.is_dir()]
    files = [p for p in entries if p.is_file()]

    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as archive:
        stamp = time.localtime()[:6]
        top = zipfile.ZipInfo(f"{root}/", stamp)
        top.external_attr = (0o40755 << 16) | 0x10
        archive.writestr(top, "")

        for directory in dirs:
            info = zipfile.ZipInfo(
                f"{root}/{directory.relative_to(src).as_posix()}/", stamp
            )
            info.external_attr = (0o40755 << 16) | 0x10
            archive.writestr(info, "")

        for path in files:
            archive.write(path, f"{root}/{path.relative_to(src).as_posix()}")

    print(f"Packed: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
