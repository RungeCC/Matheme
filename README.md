# Matheme

Matheme brings [Typst](https://typst.app/) typesetting into the Wolfram Language. Typst source is compiled in-process by a Rust library loaded over WSTP and returned as native Wolfram vector `Graphics` — or as PDF, SVG, and HTML — with no external process and no PDF import round-trip.

```
Wolfram ── source + options ──▶ Rust / WSTP ──▶ Typst ──▶ Graphics | PDF | SVG | HTML
```

In a notebook FrontEnd, option names, settings keys, and query/cache filter keys offer argument autocompletion.

## Requirements

- Wolfram 15.0 on Windows x86-64 or Linux x86-64 (tested on Windows Wolfram 15.0 and Arch Linux WolframEngine 15.0)
- The `Wolfram/ErrorTools` paclet (shipped with Wolfram 15)
- A built paclet: `nu scripts/build.nu` (Rust toolchain, Nushell, and the WSTP DeveloperKit; on Linux also `openssl` and `util-linux-libs`/`libuuid`) produces `output/Matheme/` and `output/Runge__Matheme-0.1.0.paclet` (pass `-j <N>` to limit Cargo parallelism on memory-constrained machines)

## Install and load

```wolfram
PacletInstall["/absolute/path/to/matheme/output/Runge__Matheme-0.1.0.paclet"];
Get["Runge`Matheme`"];
```

During development, load the assembled directory directly instead:

```wolfram
PacletDirectoryLoad["/absolute/path/to/matheme/output/Matheme"];
Get["Runge`Matheme`"];
```

The library function is loaded lazily on the first call.

## Quick start

```wolfram
res = MathemeToGraphics["= Hello $x^2$"];
First[res["Graphics"]]     (* native vector Graphics, one per page *)

pdf = MathemeCompile["= Hello $x^2$"];
pdf["PDF"]                 (* ByteArray *)
pdf["Pages"]               (* exported page count; "TotalPages" is the full count *)
```

Wolfram expressions can be converted automatically:

```wolfram
MathemeToGraphics[TypstForm[HoldForm[Integrate[Sin[x]/x, {x, 0, Infinity}]]]]
```

## Functions

| Function | Purpose |
| -------- | ------- |
| `MathemeCompile[source, opts]` | Compile Typst source to `"PDF"` (default), `"SVG"`, `"HTML"`, or `"Graphics"`. |
| `MathemeToGraphics[source, opts]` | Compile and return vector `Graphics` pages (vector formats only). |
| `TypstForm[expr, opts]`, `MathemeToTypst[expr, opts]`, `BoxesToTypst[boxes, opts]` | Convert Wolfram expressions or boxes into Typst math strings. |
| `TypstRaw[str]`, `MakeTypst[expr]` | Extension hooks for literal Typst fragments and per-type conversions. |
| `SetMatheme[rules...]`, `ResetMatheme[]` | Read (`SetMatheme[]`), update, and reset process-wide compiler settings. |
| `MathemeQueryFonts[filters]`, `MathemeQueryPackages[filters]` | Inspect cached font and package metadata. |
| `MathemeWarmup[opts]`, `MathemeReloadFonts[]`, `MathemeReloadPackages[]`, `ClearMathemeCache[targets]` | Initialize and manage caches. |
| `MathemeVersion[]`, `MathemeClose[]` | Version metadata; release the loaded DLL. |

## Options

All option names are strings. `MathemeToGraphics` shares the same options, with `"Format" -> "Graphics"` as its default.

| Option | Default | Meaning |
| ------ | ------- | ------- |
| `"Inputs"` | `<\|\|>` | Values injected as Typst `sys.inputs` (strings, signed 64-bit integers, machine reals, booleans, lists, associations, `ByteArray`). |
| `"BasePath"` | `Directory[]` | Root for relative `#include` / `#image` paths. |
| `"AllowDownloads"` | `False` | Allow missing Universe packages to be downloaded for this call. |
| `"TypstTemplate"` | `Automatic` | Uses `"Equation"` for `TypstForm` and leaves string input as supplied. Use `None` to disable templating, or provide a built-in name, template, or function. |
| `"Format"` | `"PDF"` | `"PDF"`, `"SVG"` (per-page strings), `"HTML"`, or `"Graphics"` (alias `"Vector"`). |
| `"Pages"` | `All` | Page filter (integer, list, `Span`, or `"1,3-5"`) for paged formats. PDF filtering requires `"PDFTags" -> False` and must select at least one page. |
| `"PDFStandard"` | `Automatic` | PDF version, archival standard, accessibility standard, or a compatible list of these. |
| `"PDFTags"` | `True` | Emit tagged PDF accessibility structure. |
| `"CreationTimestamp"` | `Automatic` | `None`, a `DateObject`, or a Unix timestamp. A fixed local offset also sets the default offset for Typst's `datetime.today()`. |
| `"Features"` | `{}` | Experimental Typst features: `"html"`, `"bundle"`, `"a11y-extras"`. |
| `"Evaluate"` | `None` | Typst code evaluated with the main module's scope, project-relative paths, library styles, and queries over the full document. Results appear under `"Evaluated"`. |

Downloads are off by default: use `"AllowDownloads" -> True` per call or `"Downloads" -> True` in the settings. Paged exports report `"Pages"` and `"TotalPages"`; results also carry `Diagnostics` and `FormattedCell` when there are diagnostics.

Call options use Wolfram's `OptionValue` precedence and honor `SetOptions` on the public function. Wolfram checks option names, basic types, and format-level conflicts before loading the library. Rust validates ranges, accepted values, and exporter-specific constraints. Rational and arbitrary-precision real inputs are converted to machine precision with a conversion warning. Integers outside Typst's range become `none` with a warning.

When Rust argument types change, regenerate `Kernel/ArgumentTypes.wl` with `cargo run -p matheme --features option-schema --bin matheme-option-schema`. The Rust tests check that the generated file is current.

## Results and errors

Success returns a plain Association. Failure returns an ErrorTools `Failure` object rather than aborting the evaluation:

```wolfram
res = MathemeCompile["#let x ="];
FailureQ[res]         (* True *)
res[[1]]              (* MathemeCompileError *)
res["Diagnostics"]    (* structured diagnostics with Severity and Source *)
res["FormattedCell"]  (* Notebook-ready Code Cell diagnostic report *)
```

| Tag | Raised when |
| --- | ----------- |
| `MathemeCompileError` | Typst compilation, export, or evaluation fails. |
| `MathemeCallError` | Invalid arguments, options, option combinations, or settings. |
| `MathemeLibraryError` | The shared library cannot be loaded, or WSTP transport / envelope protocol fails. |
| `MathemeInternalError` | An unexpected internal backend panic or invariant violation occurs. |

Each entry in `Diagnostics` includes `"Severity"` (`"Error"`, `"Warning"`, or `"Hint"`), `"Source"` (`"Typst"`, `"Graphics"`, `"Conversion"`, `"Package"`, or `"Host"`), `"Message"`, `"Location"`, `"Hints"`, and `"Trace"`. Non-fatal `"Warning"` diagnostics are issued as `MathemeDiagnostic::warning` messages while `"Hint"` diagnostics stay silently attached to `Diagnostics` and `FormattedCell`.

## Templates and TypstForm

Built-in `"TypstTemplate"` presets are `"Equation"`, `"InlineEquation"`, `"Text"`, `"MiTeXEquation"`, `"MiTeXInlineEquation"`, and `"MiTeXText"`:

```wolfram
MathemeCompile["x^2 + y^2 = z^2", "TypstTemplate" -> "Equation"]
MathemeCompile["E = mc^2", "TypstTemplate" -> StringTemplate["$``$"]]
```

`TypstForm` and `MathemeToTypst` accept style options (`"Form"`, `"DelimiterStyle"`, `"FractionStyle"`, `"ConstantStyle"`, and others) and rule hooks (`"ExpressionRules"`, `"BoxRules"`, `"TemplateRules"`, `"CharacterRules"`, ...). Passing a `TypstForm` to `MathemeCompile` or `MathemeToGraphics` automatically applies the `Equation` template. MiTeX templates import `@preview/mitex:0.2.7`, so they need `"AllowDownloads" -> True` until the package is cached.

## Settings and caches

```wolfram
SetMatheme[]                                              (* inspect current settings *)
SetMatheme["SystemFonts" -> False, "FontPaths" -> {"D:/fonts"}];
MathemeQueryFonts[<|"Family" -> "libertinus"|>];
MathemeQueryPackages[<|"Name" -> "cetz"|>];
ClearMathemeCache["Fonts"];
```

Settings keys are `FontPaths`, `SystemFonts`, `EmbeddedFonts`, `PackagePath`, `PackageCache`, `Proxy`, and `Downloads`; `Null` resets a single field and `ResetMatheme[]` resets all. Query filters include `Family`, `Source`, `Resident`, `Character`, `Name`, `Namespace`, `Version`, `Origin`, `Offset`, `MaxResults`, `AllowDownloads`, and `Refresh`. `MathemeWarmup[]` initializes the caches before the first compile; `MathemeClose[]` releases the DLL, so call it before rebuilding the library in a running kernel.

## License

Matheme's own Rust and Wolfram Language sources are [Apache-2.0](LICENSE). See [NOTICE](NOTICE) and `third-party/` for the components compiled into `matheme.dll` (Typst and its bundled fonts). The DLL also statically links Wolfram's proprietary WSTP library, which is **not** covered by Apache-2.0 and is governed by the [MathLink License Agreement](https://www.wolfram.com/legal/agreements/mathlink.html). A Wolfram installation and `Wolfram/ErrorTools` are user-supplied runtime requirements and are not bundled.

Copyright 2026 Runge
