# Matheme

A bridge that lets the Wolfram Language typeset with [Typst](https://typst.app/).
A Rust `cdylib` is loaded **in-process** by Wolfram 15 via WSTP `LinkObject`;
Typst compiles source to PDF, and the Wolfram layer imports that PDF as native
vector `Graphics`. No external process and no Frame-to-Graphics reimplementation.
The only callback into the kernel is an evaluation of
`$InstallationDirectory` to locate Wolfram's bundled fonts; it is cached
in process state and only refreshes the font book if the paths change.

```
Wolfram ──Association──▶ Rust / WSTP ──▶ Typst compile ──▶ PDF bytes
   ▲                                                          │
   └──────────── ImportByteArray[..., {"PDF","PageGraphics"}] ◀┘
```

## Layout

| Path | Role |
|---|---|
| `crates/matheme-world/` | Typst dependency: `World` (with features and clock/timestamp support), font & package caches, selective cache clearing, file loader, CLI-style diagnostics. Knows nothing about Wolfram. |
| `crates/matheme-wl/` | Pure bidirectional WL-expression ↔ Typst-value conversion (`convert::to_typst` / `Expr::to_typst` and `convert::from_typst` / `Expr::from_typst`). `Expr`, `Normal<'a>`, `Entry<'a>`/`Rule` and `Symbol` wrappers expose `From`/`TryFrom` and zero-copy `as_*` accessors; `assoc!` / `rule!` build associations with WL's `key -> value` syntax. No WSTP dependency. |
| `src/` | WSTP bridge: one exported WSTP function receives `<|"Action" -> name, "Arguments" -> <|…|>|>`; `lib.rs` owns the action table, `call.rs` is the uniform read → dispatch → write pipeline, with dedicated modules for `compile.rs` (PDF/SVG/HTML + simultaneous `Evaluate`), `cache.rs`, `version.rs`, `settings.rs`, `query.rs`, `diagnostics.rs`, and `wolfram.rs`. |
| `Kernel/`, `PacletInfo.m` | Wolfram paclet wrapper and public API. Uses the `Wolfram/ErrorTools` paclet to return `Failure` objects. |
| `scripts/` | WSTP SDK setup and one-shot build. |
| `output/` | Generated build artifacts (gitignored): standalone paclet directory `output/Matheme/` and packaged `output/Matheme-*.paclet`. |

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
2. runs `cargo build --release`;
3. assembles a standalone paclet directory at `output/Matheme/` (`PacletInfo.m`,
   `Kernel/`, and `LibraryResources/Windows-x86-64/matheme.dll`);
4. packages the distributable archive `output/Matheme-<version>.paclet` via
   `CreatePacletArchive` (when `wolframscript` is available).

Wolfram ships two SDKs: the MathLink one only has `ml64i4s.lib` (ML* symbols),
which used to require a library alias plus 23 `/alternatename` linker shims.
Using the WSTP DeveloperKit library removes both workarounds and lets
`cargo test --workspace` link normally. Override the SDK location with
`WSTP_SDK_DIR` if Wolfram is installed elsewhere.

## Load

Load directly from the assembled paclet directory:

```wolfram
PacletDirectoryLoad["/absolute/path/to/matheme/output/Matheme"];
Get["Matheme`"];
```

Or install the packaged archive:

```wolfram
PacletInstall["/absolute/path/to/matheme/output/Matheme-0.1.0.paclet"];
Get["Matheme`"];
```

The library function is loaded lazily on first use. The DLL exposes exactly
one WSTP function (`matheme_call`); all public Wolfram functions send it an
envelope `<|"Action" -> name, "Arguments" -> <|…|>|>`. To rebuild the DLL
while the kernel is running, call `MathemeClose[]` first (see *Resource
release* below).

The paclet requires the
[`Wolfram/ErrorTools`](https://resources.wolframcloud.com/PacletRepository/resources/Wolfram/ErrorTools/)
paclet (`PacletInstall["Wolfram/ErrorTools"]`); Wolfram 15 ships it as a
system component. `BeginPackage` loads it to construct `Failure` objects.

## Results and failures

Every public function uses one pipeline. The library returns a wire
Association with an internal `Status` field; on success the Wolfram layer
removes it and returns the plain Association. On failure it returns an
ErrorTools `Failure` object:

```wolfram
res = MathemeCompile["#let x ="];
FailureQ[res]       (* True *)
res[[1]]            (* Matheme`MathemeCompileError *)
res["Diagnostics"]  (* structured diagnostics *)
res["Formatted"]    (* typst-cli style report *)
res["Operation"]    (* "MathemeCompile" *)
```

| Tag | Raised when |
|---|---|
| `MathemeCompileError` | Typst compilation fails; the Failure carries `Diagnostics` and `Formatted`. |
| `MathemeCallError` | Arguments, settings or a network operation fail. |
| `MathemeLibraryError` | The DLL cannot be loaded or returns something unexpected. |

All three are registered with `CreateErrorType`, so
`Handle[expr, FailurePattern[MathemeCompileError]]` works. Non-fatal warnings
are issued as `MathemeDiagnostic::warning` messages (`Off[MathemeDiagnostic::warning]`
silences them) and stay available in the result's `Diagnostics` field.

## Usage

### Compile to PDF

```wolfram
res = MathemeCompile["= Hello $x^2$", "Inputs" -> <|"title" -> "demo"|>];
res["PDF"]        (* ByteArray *)
res["Pages"]
res["Diagnostics"]  (* present when there are warnings *)
res["Formatted"]  (* plain-text diagnostic report, typst-cli style *)
```

On failure `MathemeCompile` returns a `Failure` tagged
`MathemeCompileError` instead of an Association; `MathemeToGraphics` passes
that Failure through unchanged.

Signatures:

```wolfram
MathemeCompile[source_String, opts : OptionsPattern[]]
MathemeToGraphics[source_String, opts : OptionsPattern[]]
```

Options (all names are strings):

| Option | Default | Meaning |
|---|---|---|
| `"Inputs"` | `<\| \|>` | Dictionary injected as Typst `sys.inputs`. |
| `"BasePath"` | `Directory[]` | Root for `#include` / `#image` relative paths. |
| `"AllowDownloads"` | `False` | If `True`, missing Universe packages may be downloaded. |
| `"TypstTemplate"` | `None` | A key of `$TypstTemplates`, or a `Function` / template applied to `source` before compilation (see *Templates*). |
| `"Format"` | `"PDF"` | Output format: `"PDF"`, `"SVG"` (list of per-page SVG strings), or `"HTML"` (automatically enables the `"html"` feature). |
| `"Pages"` | `All` | Page filter (`All`, integer, list of integers/`{start, end}`/`Span`, or range string such as `"1,3-5"`) for PDF and SVG exports. |
| `"PDFStandard"` | `Automatic` | `Automatic`, a standard string (`"1.4"`–`"2.0"`, `"a-1b"`–`"a-4e"`, `"ua-1"`), or a list of standards. |
| `"PDFTags"` | `True` | Whether to emit tagged PDF accessibility structure. |
| `"CreationTimestamp"` | `Automatic` | `Automatic` (system clock), `None`, a `DateObject`, or a Unix timestamp integer. |
| `"Features"` | `{}` | Experimental Typst features (`"html"`, `"bundle"`, `"a11y-extras"`). |
| `"Evaluate"` | `None` | Optional Typst code expression, list, or association evaluated against the compiled document's scope and introspector, returned under `"Evaluated"`. |

CLI-only flags such as `--watch`, `--open`, `--jobs`, `--timings`, `--deps`, `--diagnostic-format`, and `--ppi` (Wolfram handles rasterization via `Rasterize` / `MathemeToGraphics`) are intentionally not exposed.

`Inputs` accepts strings, integers, reals, `True`/`False`/`Null`, lists,
nested associations and `ByteArray` (mapped to Typst `bytes`); unsupported
values become warnings in `Diagnostics`. Conversely, `"Evaluated"` maps Typst
values back to native Wolfram expressions (`Quantity`, `RGBColor`, `DateObject`,
`TimeObject`, `ByteArray`, `Association`, `List`, `Automatic`, `Null`, and
structured associations for `Type`, `Label`, `Symbol`, `Version`, `Decimal`,
`Content`, and `Module`).

```wolfram
res = MathemeCompile[
  "#let raw = sys.inputs.blob\nRaw bytes: #raw.len()",
  "Inputs" -> <|"blob" -> ByteArray[{0, 1, 2, 255}]|>
];
```

### Compile to vector Graphics

```wolfram
res = MathemeToGraphics["= Hello $x^2$"];
res["Graphics"]   (* list of Graphics, one per PDF page *)
```

Same options and return shape as `MathemeCompile`, plus a `"Graphics"` key.
The vector conversion uses Wolfram's built-in
`ImportByteArray[pdf, {"PDF", "PageGraphics"}]`, so text and paths stay vector.

### Diagnostics

`Formatted` renders like `typst` CLI in a `"Code"`-styled Notebook `Cell`
(`DisplayForm[Cell[..., "Code"]]` when non-empty; `""` when there are no
diagnostics), with source snippets and CLI-style file names:

```text
error: unclosed delimiter
  --> <typst source>:2:16
  |
2 | Value: #let x = (
  |                 ^
```

Each entry of `Diagnostics` is an Association with `Severity`, `Message`,
`Location` (`File`, `Line`, `Column`, byte `Range`, source `Text`), `Hints` and
`Trace`. Structured `Line`/`Column` are **1-based**; the `Formatted` report
keeps the CLI's own 0-based column display. File names follow the CLI: project
files are relative to `BasePath`, packages appear as `@preview/cetz:0.3.1/…`,
and the in-memory source is `<typst source>`.

Warnings (including bridge-level ones such as unsupported `Inputs` values or
the offline-package hint) are issued as `MathemeDiagnostic::warning` messages.
Errors are not messages: they are carried by `MathemeCompileError`'s Failure
object together with the CLI-style `Formatted` report.

### Templates

`$TypstTemplates` is a protected, read-only Association of Typst source
templates: `Equation`, `InlineEquation`, `Text`, `MiTeXEquation`,
`MiTeXInlineEquation` and `MiTeXText`. Pass a key through the `"TypstTemplate"`
option, or supply a `Function` / template object directly:

```wolfram
MathemeCompile["x^2 + y^2 = z^2", "TypstTemplate" -> "Equation"]
MathemeCompile["E = mc^2", "TypstTemplate" -> StringTemplate["$``$"]]
MathemeCompile["x^2", "TypstTemplate" -> (("$" <> # <> "$") &)]
```

Templates are applied in the Wolfram layer with `TemplateApply` (or called as
a `Function`) before the resulting source is sent to the compiler; the Rust
side knows nothing about templates. An unknown key returns a
`MathemeCallError` Failure listing the available keys. MiTeX templates import
`@preview/mitex:0.2.7`, so they need `"AllowDownloads" -> True` until the
package is cached — templates never turn downloads on implicitly.

### Packages and fonts

- Packages load from the standard Typst data/cache directories and the
  Universe. **Downloads are off by default**; pass `"AllowDownloads" -> True`
  for one call, or set `"Downloads" -> True` globally with `MathemeSetSettings`.
  An offline miss returns a clear diagnostic that suggests turning downloads on.
- Font sources, in priority order (earlier wins when families tie):
  1. `FontPaths` — explicit directories from `MathemeSetSettings`;
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
  through `MathemeSetSettings`. No environment variables are consulted.

### Settings

Configuration is explicit process state, not environment variables: the caches
read it when they are (re)built, and every change is visible from Wolfram.
Reading and writing are deliberately separate public functions:

```wolfram
MathemeSettings[]                                      (* read-only: <|"Settings" -> …|> *)
MathemeSetSettings[<|"SystemFonts" -> False|>]         (* patch: <|"Changed" -> …, "Settings" -> …|> *)
MathemeSetSettings[<|"FontPaths" -> {"D:/fonts"}|>]
MathemeSetSettings[<|"PackagePath" -> "D:/typst/data", "PackageCache" -> "D:/typst/cache"|>]
MathemeSetSettings[<|"Proxy" -> "http://127.0.0.1:2191", "Downloads" -> True|>]
MathemeSetSettings[<|"FontPaths" -> Null|>]            (* reset one field *)
MathemeResetSettings[]                                 (* reset every field *)
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

`MathemeSettings[]` returns only `Settings`. The two writing functions return
`Changed` (which groups were invalidated) and the effective `Settings`; font
counts and package status are *not* part of this API — use
`MathemeQueryFonts[]` / `MathemeQueryPackages[]`. There is no `Reset` patch key:
single-field reset is `Null`, and full reset is `MathemeResetSettings[]`.
A malformed patch returns a `MathemeCallError` Failure. Host-provided
`HostFontPaths` is reported for transparency but is not patchable and survives
a full reset.
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
therefore needs network access; a failure is issued as a warning message and
leaves the other caches warmed.

### Querying cached metadata

Fonts and packages have **separate** query functions with separate argument
models and result schemas — no target-dispatched mix. Both are read-only views
of exactly the metadata the library caches: the font book (plus each face's
source and residency), the listing of locally installed packages, and the
Universe index once typst has fetched it. A plain query never loads a font
file, writes to disk or reaches the network.

```wolfram
MathemeQueryFonts[]                            (* first 50 faces *)
MathemeQueryFonts[<|"Family" -> "libertinus", "Source" -> "Embedded"|>]
MathemeQueryFonts[<|"Resident" -> True, "MaxResults" -> All|>]
MathemeQueryFonts[<|"MaxResults" -> All, "Offset" -> 100|>]

MathemeQueryPackages[]                         (* first 50 entries *)
MathemeQueryPackages[<|"Name" -> "cetz", "Origin" -> "Universe"|>]
MathemeQueryPackages[<|"AllowDownloads" -> True|>]  (* fetch Universe index *)
```

| Key | Applies to | Meaning |
|---|---|---|
| `Family` | `MathemeQueryFonts` | Case-insensitive substring of the family name. |
| `Source` | `MathemeQueryFonts` | `Custom`, `Host`, `Embedded` or `System`. |
| `Resident` | `MathemeQueryFonts` | `True`/`False`: whether the face's bytes are in memory. |
| `Character` | `MathemeQueryFonts` | One-character string; keeps faces whose typst `Coverage` contains it (the same predicate typst uses for font fallback, so it reflects declared cmap ranges and can be coarse). |
| `Name`, `Namespace`, `Version` | `MathemeQueryPackages` | Name substring, namespace and exact version. |
| `Origin` | `MathemeQueryPackages` | `Local`, `Data`, `Cache` or `Universe`. |
| `Offset`, `MaxResults` | both | Paging; default 50 rows, `MaxResults -> All` removes the cap. |
| `AllowDownloads` | `MathemeQueryPackages` | Explicit opt-in to fetch and cache the Universe index. |
| `Refresh` | `MathemeQueryPackages` | Rescan the local package directories. |

`MathemeQueryFonts` returns `Total` (faces in the book), `Matched`, `Returned`,
`Offset`, `MaxResults`, `Resident` (number of faces whose bytes are in memory)
and `Results` — associations with `Index`, `Family`, `Style`, `Weight`,
`Stretch`, `Flags`, `Source`, `Path`, `CollectionIndex` and `Resident`.
`MathemeQueryPackages` returns those paging fields plus `Local`, `Universe`,
`UniverseIndexCached` and `Results` — associations with `Namespace`, `Name`,
`Version`, `Origin`, `Path` and `Metadata` (Universe index fields such as
`Description`, `Authors`, `License`, `Keywords`, `Categories`, `Entrypoint`,
`Repository`, `UpdatedAt` and `Template`; `Null` for local packages).

Universe entries appear only after the index is cached — by
`MathemeWarmup["PackageIndex" -> True]`, by a compile that resolves an
unversioned `@preview` package, or by `AllowDownloads -> True`. A query that
returns every face still reads no font file: the resident count before and
after `MathemeQueryFonts[<|"MaxResults" -> All|>]` is unchanged. Unknown keys
or wrong value types are our own Wolfram layer's bugs, so they become a
`MathemeCallError` Failure rather than being silently ignored.

### Cache / state management

| Function | Effect |
|---|---|
| `MathemeVersion[]` | Version and DLL metadata: `"Matheme"`, `"Typst"`, `"TypstCommit"`, `"Target"`, `"LibraryLink"`, and `"WSTP"`. |
| `MathemeSettings[]` | Read the configuration (see *Settings*). |
| `MathemeSetSettings[patch]` | Apply a settings patch. |
| `MathemeResetSettings[]` | Restore every user-facing setting. |
| `MathemeQueryFonts[]` | Query cached font-book metadata (see *Querying cached metadata*). |
| `MathemeQueryPackages[]` | Query cached package metadata. |
| `MathemeReloadFonts[]` | Rescan all configured font sources (e.g. after installing a font). |
| `MathemeReloadPackages[]` | Rebuild the package loader and drop the cached Universe index. |
| `ClearMathemeCache[targets]` | Drop selected caches (`"Fonts"`, `"Packages"`, `"Memoization"`, or `All`) without unloading the library. |
| `MathemeClose[]` | Close the WSTP `LinkObject`, drop every library cache (fonts, packages, memoization), unload the single library function and release the DLL image. |

### Memory

Measured on the Wolfram 15 kernel (process working set, `.tmp/test-memory.wl`):

| Stage | Before lazy fonts | After |
|---|---|---|
| Kernel start | 0.21 GiB | 0.21 GiB |
| After font scan (`MathemeWarmup[]`) | **8.2 GiB** | 0.22 GiB |
| After 6 compiles | 8.2 GiB | 0.24 GiB |
| After `MathemeClose[]` | 8.2 GiB (nothing released) | **0.22 GiB** |
| After reload + rescan | 16.2 GiB (doubled) | 0.23 GiB |

`MathemeClose[]` releases the font book and any lazily loaded faces, the
package loader (including the Universe index) and typst's memoized
compilation results (`comemo::evict(0)` — these also pin `Font` clones, so
evicting is required for the memory to actually return). Unloading the
`LibraryFunction`s unmaps the DLL image too (the file becomes writable again).
`MathemeVersion[]` no longer triggers a font scan; the numbers above are
taken after `MathemeWarmup[]` or a font query.

### Resource release

`LibraryFunctionLoad[..., LinkObject, LinkObject]` creates a `LinkObject`
that keeps the DLL file handle open. `MathemeClose[]` asks the library to drop
its caches, unloads the one library function and closes its link, after which
the DLL can be overwritten and later calls transparently reload it. Use it
before rebuilding the DLL in a running kernel.

On Wolfram 15, `LibraryFunctionUnload` already drops the last module
reference, so a subsequent `LibraryUnload` reports "cannot be unloaded" even
though the mapping is gone — a false negative. `MathemeClose[]` therefore
takes one extra `LibraryLoad` reference *before* unloading, leaving the final
`LibraryUnload` with a real mapping to release; it reports
`LibraryUnloaded -> True` and the DLL file becomes writable.

```wolfram
MathemeClose[]  (* <|"Released"->{"Fonts","Packages","Memoization"},
                       "Unloaded"->1, "LinksClosed"->1,
                       "LibraryUnloaded"->True|> )
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
