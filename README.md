# Matheme

A bridge that lets the Wolfram Language typeset with [Typst](https://typst.app/).
A Rust `cdylib` is loaded **in-process** by Wolfram 15 via WSTP `LinkObject`;
Typst compiles source to PDF, and the Wolfram layer imports that PDF as native
vector `Graphics`. No external process and no Frame-to-Graphics reimplementation.
The only callback into the kernel is a one-time evaluation of
`$InstallationDirectory` to locate Wolfram's bundled fonts.

```
Wolfram ──Association──▶ Rust / WSTP ──▶ Typst compile ──▶ PDF bytes
   ▲                                                          │
   └──────────── ImportByteArray[..., {"PDF","PageGraphics"}] ◀┘
```

## Layout

| Path | Role |
|---|---|
| `crates/matheme-world/` | Typst dependency: `World`, font & package caches (`query.rs` is a read-only view over their metadata), file loader, CLI-style diagnostics. Knows nothing about Wolfram. |
| `crates/matheme-wl/` | Pure WL-expression ↔ Typst-value conversion. `Expr`, `Normal`, `Entry`/`Rule` and `Symbol` wrappers expose `From`/`Into` and `as_*` accessors instead of raw `ExprKind` matching. `RuleDelayed` stays distinct from `Rule`. No WSTP dependency. |
| `src/` | WSTP bridge: exported functions, argument reading, diagnostic expressions, settings/query mappings (`settings.rs`, `query.rs`) and Wolfram-specific discovery (`wolfram.rs`). |
| `Kernel/`, `PacletInfo.m` | Wolfram paclet wrapper and public API. |
| `scripts/` | WSTP SDK setup and one-shot build. |

Versions are pinned: `typst`/`typst-kit` `=0.15.1`, `wolfram-library-link`
`=0.6.1` (see `Cargo.lock`). `MathemeVersion[]` reports the Typst version from
Typst's own build-time `TYPST_VERSION` environment variable
(`typst::utils::version()`), not a hard-coded literal.

## Build

Requires Rust (MSVC target, `x86_64-pc-windows-msvc`) and Wolfram 15.0 on
Windows x86-64.

```bash
bash scripts/build.sh
```

The script:

1. runs `scripts/setup-wstp.sh`, which copies Wolfram 15's genuine
   `wstp64i4s.lib` (WS* symbols) from the **WSTP DeveloperKit** into `.tmp/`;
2. `cargo build --release`;
3. copies the DLL to `LibraryResources/Windows-x86-64/matheme.dll`.

Wolfram ships two SDKs: the MathLink one only has `ml64i4s.lib` (ML* symbols),
which used to require a library alias plus 23 `/alternatename` linker shims.
Using the WSTP DeveloperKit library removes both workarounds and lets
`cargo test --workspace` link normally. Override the SDK location with
`WSTP_SDK_DIR` if Wolfram is installed elsewhere.

## Load

```wolfram
PacletDirectoryLoad["/absolute/path/to/matheme"];
Get["Matheme`"];
```

The library functions are loaded lazily on first use. To rebuild the DLL while
the kernel is running, call `MathemeClose[]` first — the WSTP `LinkObject`s
hold the DLL open (see *Resource release* below).

## Usage

### Compile to PDF

```wolfram
res = MathemeCompile["= Hello $x^2$", <|"title" -> "demo"|>] ;
res["Status"]     (* "Success" | "Error" *)
res["PDF"]        (* ByteArray, or Null on error *)
res["Pages"]
res["Diagnostics"]
res["Formatted"]  (* plain-text diagnostic report, typst-cli style *)
```

`MathemeCompile` returns the **same Association shape** on success and error;
branch on `Status`.

Signatures:

```wolfram
MathemeCompile[source_String, opts___]
MathemeCompile[source_String, inputs_Association, opts___]
```

Options:

| Option | Default | Meaning |
|---|---|---|
| `"Inputs"` | `<\| \|>` | Dictionary injected as Typst `sys.inputs`. |
| `"BasePath"` | `Directory[]` | Root for `#include` / `#image` relative paths. |
| `"AllowDownloads"` | `False` | If `True`, missing Universe packages may be downloaded. |

`Inputs` accepts strings, integers, reals, `True`/`False`/`Null`, lists,
nested associations and `ByteArray` (mapped to Typst `bytes`); unsupported
values become warnings in `Diagnostics`.

```wolfram
res = MathemeCompile[
  "#let raw = sys.inputs.blob\nRaw bytes: #raw.len()",
  <|"blob" -> ByteArray[{0, 1, 2, 255}]|>
];
```

### Compile to vector Graphics

```wolfram
res = MathemeToGraphics["= Hello $x^2$"];
res["Graphics"]   (* list of Graphics, one per PDF page *)
```

Same options and return shape as `MathemeCompile`, plus a `"Graphics"` key.
On error `"Graphics"` is `{}`. The vector conversion uses Wolfram's built-in
`ImportByteArray[pdf, {"PDF", "PageGraphics"}]`, so text and paths stay vector.

### Diagnostics

`Formatted` renders like `typst` CLI, with source snippets and CLI-style file
names:

```text
error: unclosed delimiter
  ┌─ <typst source>:2:16
  │
2 │ Value: #let x = (
  │                 ^
```

Each entry of `Diagnostics` is an Association with `Severity`, `Message`,
`Location` (`File`, `Line`, `Column`, byte `Range`, source `Text`), `Hints` and
`Trace`. Structured `Line`/`Column` are **1-based**; the `Formatted` report
keeps the CLI's own 0-based column display. File names follow the CLI: project
files are relative to `BasePath`, packages appear as `@preview/cetz:0.3.1/…`,
and the in-memory source is `<typst source>`.

### Packages and fonts

- Packages load from the standard Typst data/cache directories and the
  Universe. **Downloads are off by default**; pass `"AllowDownloads" -> True`
  for one call, or set `Downloads -> True` globally with `MathemeSettings`.
  An offline miss returns a clear diagnostic that suggests turning downloads on.
- Font sources, in priority order (earlier wins when families tie):
  1. `FontPaths` — explicit directories from `MathemeSettings`;
  2. Wolfram's bundled fonts — the bridge evaluates
     ``System`$InstallationDirectory`` over WSTP once per process (no filesystem
     guessing) and injects `$InstallationDirectory/SystemFiles/Fonts/TrueType`
     (Mathematica Sans/Mono, Alegreya, Source Sans, Clear Sans, …); these are
     not OS-registered, so Typst would not see them otherwise. On this machine
     that is 149 faces.
  3. **Typst's embedded fonts** (`typst-assets` via `typst-kit`): Libertinus
     Serif, New Computer Modern (+ Math) and DejaVu Sans Mono. They give a
     sensible default rendering even with `SystemFonts -> False` and no fonts
     installed — 17 faces here.
  4. OS fonts, unless `SystemFonts -> False`.
  Totals on this machine: 149 host + 17 embedded + 1069 system = **1235 faces**
  (custom paths add on top). Family names follow the font metadata, e.g.
  `"MathematicaSans"` / `"MathematicaMono"` (no space) and `"Libertinus Serif"`.
- **Font data loads lazily.** Only the font *book* (metadata) is built at scan
  time; a font file is read when a document first selects that face, and then
  stays resident for reuse. This matters: typst's `Font` holds the entire font
  file, and a multi-face `.ttc` is held once *per face* — eagerly materializing
  all 1069 system faces costs **7.4 GiB** on this machine (a single 16-face
  `SourceHanSans.ttc` alone accounts for 4.9 GiB). With lazy loading the
  steady state is the book plus whatever your documents actually use (e.g. a
  CJK document pulls in one full `SourceHanSans.ttc` read, ~300 MiB).
  `crates/matheme-world/examples/font-memory.rs` reproduces these numbers.
- The world crate itself remains Wolfram-agnostic: the bridge supplies the
  bundled directory through `set_host_font_paths`, and users configure paths
  through `MathemeSettings`. No environment variables are consulted.

### Settings

Configuration is explicit process state, not environment variables: the caches
read it when they are (re)built, and every change is visible from Wolfram.

```wolfram
MathemeSettings[]                                      (* report *)
MathemeSettings[<|"SystemFonts" -> False|>]            (* patch *)
MathemeSettings[<|"FontPaths" -> {"D:/fonts"}|>]
MathemeSettings[<|"PackagePath" -> "D:/typst/data", "PackageCache" -> "D:/typst/cache"|>]
MathemeSettings[<|"Proxy" -> "http://127.0.0.1:2191", "Downloads" -> True|>]
MathemeSettings[<|"FontPaths" -> Null|>]               (* reset one field *)
MathemeSettings[<|"Reset" -> True|>]                   (* reset everything *)
```

| Key | Type | Effect |
|---|---|---|
| `FontPaths` | list of strings / `Null` | Extra font directories, searched before everything else. |
| `SystemFonts` | `True`/`False` | Scan OS fonts. |
| `EmbeddedFonts` | `True`/`False` | Use Typst's bundled Libertinus / New CM / DejaVu families. |
| `PackagePath` | string / `Null` | Typst package data directory (`Null` = platform default). |
| `PackageCache` | string / `Null` | Package cache directory (`Null` = platform default). |
| `Proxy` | string / `Null` | Explicit proxy for downloads; `Null` connects directly and ignores the process environment. |
| `Downloads` | `True`/`False` | Global download policy; `AllowDownloads` still opts in per call. |
| `Reset` | `True`/`False` | Restore every default before applying the rest of the patch. |

Returns `Status`, `Changed` (which groups were invalidated), `FontsReloaded`,
`PackagesRebuilt`, the effective `Settings`, `FontCounts` and package status.
Host-provided `HostFontPaths` is reported for transparency but is not patchable.
A null `Proxy` deliberately ignores `HTTP_PROXY`/`HTTPS_PROXY`: the process
environment is not a configuration channel here. Windows uses schannel via
`native-tls`, so no OpenSSL DLL is needed.

### Eager warmup

By default the font scan and package loader initialize lazily on the first
`MathemeCompile`, making that first call slower. To pay that cost up front:

```wolfram
MathemeWarmup[]                                   (* everything *)
MathemeWarmup["PackageIndex" -> False]            (* skip network index fetch *)
MathemeWarmup["Fonts" -> True, "Packages" -> False, "PackageIndex" -> False]
```

Returns per-stage timings (`FontsMs`, `PackagesMs`, `PackageIndexMs`) and counts.
Warmup builds the font *book* (metadata) and the package loader; font *files*
are not read — they load lazily when a document first uses a face, so warmup
is fast and cheap in memory. `PackageIndex` fetches the Universe index and
therefore needs network access; a failure is reported in `Diagnostics` and
leaves the other caches warmed.

### Querying cached metadata

`MathemeQuery` is a read-only view of exactly the metadata the library caches:
the font book (plus each face's source and residency), the listing of locally
installed packages, and the Universe index once typst has fetched it. A plain
query never loads a font file, writes to disk or reaches the network.

```wolfram
MathemeQuery[]                                   (* cache state only *)
MathemeQuery["Fonts"]                            (* first 50 faces *)
MathemeQuery["Fonts", <|"Family" -> "libertinus", "Source" -> "Embedded"|>]
MathemeQuery["Fonts", <|"Resident" -> True, "MaxResults" -> All|>]
MathemeQuery["Packages", <|"Name" -> "cetz", "Origin" -> "Universe"|>]
MathemeQuery[<|"Target" -> "Fonts", "MaxResults" -> All, "Offset" -> 100|>]
```

| Key | Applies to | Meaning |
|---|---|---|
| `Target` | all | `"Fonts"`, `"Packages"` or `"Caches"`; may also be the first positional argument. |
| `Family` | Fonts | Case-insensitive substring of the family name. |
| `Source` | Fonts | `Custom`, `Host`, `Embedded` or `System`. |
| `Resident` | Fonts | `True`/`False`: whether the face's bytes are in memory. |
| `Character` | Fonts | One-character string; keeps faces whose typst `Coverage` contains it (the same predicate typst uses for font fallback, so it reflects declared cmap ranges and can be coarse). |
| `Name`, `Namespace`, `Version` | Packages | Name substring, namespace and exact version. |
| `Origin` | Packages | `Local`, `Data`, `Cache` or `Universe`. |
| `Offset`, `MaxResults` | Fonts, Packages | Paging; default 50 rows, `MaxResults -> All` removes the cap. |
| `AllowDownloads` | Packages | Explicit opt-in to fetch and cache the Universe index. |
| `Refresh` | Packages | Rescan the local package directories. |

Every answer carries `Status` and `Target`. Font answers add `Total` (faces in
the book), `Matched`, `Returned`, `Offset`, `MaxResults`, `Resident` (number of
faces whose bytes are in memory) and `Results` — associations with `Index`,
`Family`, `Style`, `Weight`, `Stretch`, `Flags`, `Source`, `Path`,
`CollectionIndex` and `Resident`. Package answers add `Total`, `Local`,
`Universe`, `UniverseIndexCached` and `Results` — associations with
`Namespace`, `Name`, `Version`, `Origin`, `Path` and `Metadata` (Universe index
fields such as `Description`, `Authors`, `License`, `Keywords`, `Categories`,
`Entrypoint`, `Repository`, `UpdatedAt` and `Template`; `Null` for local
packages). `MathemeQuery[]` reports only cache state: whether each cache is
live, face counts by source, resident face count, local package count and
whether the Universe index has been fetched.

Universe entries appear only after the index is cached — by
`MathemeWarmup["PackageIndex" -> True]`, by a compile that resolves an
unversioned `@preview` package, or by `AllowDownloads -> True`. A query that
returns every face still reads no font file: the resident count before and
after `MathemeQuery["Fonts", <|"MaxResults" -> All|>]` is unchanged.

### Cache / state management

| Function | Effect |
|---|---|
| `MathemeVersion[]` | Matheme/Typst versions (with commit), effective settings, font counts per source, Wolfram font dirs/count (plus `WolframFontError` if kernel discovery failed), package dirs/count and download policy. |
| `MathemeSettings[]` | Read or patch the configuration (see *Settings*). |
| `MathemeQuery[]` | Query the cached font and package metadata (see *Querying cached metadata*). |
| `MathemeReloadFonts[]` | Rescan all configured font sources (e.g. after installing a font). |
| `MathemeReloadPackages[]` | Rebuild the package loader and drop the cached Universe index. |
| `MathemeClose[]` | Close WSTP `LinkObject`s, drop every library cache (fonts, packages, memoization) and unload the library functions. |

### Memory

Measured on the Wolfram 15 kernel (process working set, `.tmp/test-memory.wl`):

| Stage | Before lazy fonts | After |
|---|---|---|
| Kernel start | 0.21 GiB | 0.21 GiB |
| After font scan (`MathemeVersion[]`) | **8.2 GiB** | 0.22 GiB |
| After 6 compiles | 8.2 GiB | 0.24 GiB |
| After `MathemeClose[]` | 8.2 GiB (nothing released) | **0.22 GiB** |
| After reload + rescan | 16.2 GiB (doubled) | 0.23 GiB |

`MathemeClose[]` releases the font book and any lazily loaded faces, the
package loader (including the Universe index) and typst's memoized
compilation results (`comemo::evict(0)` — these also pin `Font` clones, so
evicting is required for the memory to actually return). Unloading the
`LibraryFunction`s unmaps the DLL image too (the file becomes writable
again). The subsequent best-effort `LibraryUnload` reports `$Failed` on
Wolfram 15 because there is nothing left to unload — the same happens for a
trivial probe library — so `MathemeClose[]` reports `LibraryUnloaded -> False`
without relying on that call.

### Resource release

`LibraryFunctionLoad[..., LinkObject, LinkObject]` creates extra `LinkObject`s
that keep the DLL file handle open. `MathemeClose[]` asks the library to drop
its caches, unloads all eight library functions and closes their links, after
which the DLL can be overwritten and later calls transparently reload it. Use
it before rebuilding the DLL in a running kernel.

```wolfram
MathemeClose[]  (* <|"Status"->"Success", "Unloaded"->8, "LinksClosed"->…,
                       "CachesReleased"->True, "LibraryUnloaded"->False|> *)
```

## Performance: rebuild the World vs. reuse one with unique virtual files

A repeatable benchmark lives in
`crates/matheme-world/examples/bench.rs`:

```bash
cargo run --release -p matheme-world --example bench
```

It compiles a deliberately heavy multi-page document many times. Every
document's content is made unique, so comemo's memoization cannot flatter later
iterations. Representative results (measured with 1069 system faces; the
current build has 1235 faces after adding host and embedded fonts, with no
material change to these ratios):

| Strategy | Per compile |
|---|---|
| `Library::builder().build()` alone | ~0.1 ms |
| `MathemeWorld::new` (library + loader + parse heavy source) | ~0.2 ms |
| Compile, fresh World per call (current design) | ~6 ms |
| Compile, reuse one World + **unique** virtual file name per call | ~6 ms |
| Compile, reuse one World + fixed name (identical inputs) | ~0 ms (cache hit) |
| Compile, reuse fixed name with `comemo::evict(0)` each call | ~8 ms |

**Conclusion: rebuilding the World is not significantly slower than maintaining
one World and minting a fresh virtual file each call.** The two are within
noise (~1.0×). Three reasons:

1. World construction is negligible — library build and source parsing together
   are ~0.2 ms against ~6 ms of actual typesetting, about 3%.
2. The expensive pieces (fonts, package loader/Universe index) are already
   cached globally, so a fresh `MathemeWorld` only rebuilds the cheap, per-call
   state.
3. Comemo memoization is **content-addressed**: a unique file name (and unique
   content) misses the cache anyway, so a persistent World buys nothing for
   distinct documents.

The only strategy that is dramatically faster is reusing one World with a
*fixed* file name — but that speed is entirely comemo memoization (evicting the
cache restores ~8 ms). Typst's own `World` docs warn that source files must be
cleared after each compilation: a long-lived World that keeps serving a fixed
`FileId` can return **stale content**. Minting a unique virtual file avoids the
staleness, but `FileId::new` permanently leaks an entry into Typst's global
interner (u16-id bounded at 65536) and still does not hit the cache. Given that
the current per-call World is both correct and just as fast, a persistent
session / `ManagedLibraryExpression` design is not warranted here.

## Known constraints

- Windows x86-64 + Wolfram 15.0 is the tested target; the WSTP DeveloperKit
  path is version-sensitive (override with `WSTP_SDK_DIR`).
- Wolfram 15 sends packet heads such as `ReturnPacket` over WSTP without a
  context. Kernel evaluation in `src/wolfram.rs` therefore uses the same
  context-resolving expression reader as argument parsing; the stock
  `try_evaluate()` helper cannot read these packets.
- Fonts come from the machine, so identical source can render slightly
  differently on machines with different fonts.
- Structured columns are 1-based while the `Formatted` report shows the CLI's
  0-based column; see *Diagnostics*.
