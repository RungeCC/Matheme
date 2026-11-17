(* The failure representation follows the convention in
   Wolfram/ErrorTools: private code raises Failure objects, public functions
   catch them and return them normally. *)
BeginPackage["Matheme`", {"Wolfram`ErrorTools`"}];

MathemeCompile::usage = "MathemeCompile[source, inputs] compiles Typst source to PDF and returns an Association with PDF, Pages, Diagnostics and Formatted (a typst-cli style report). On failure it returns a Failure tagged MathemeCompileError carrying the structured diagnostics. Warnings are issued as messages. Inputs may contain String, Integer, Real, True/False/Null, List, Association and ByteArray (Typst bytes). Options: \"Inputs\", \"BasePath\" (default: current directory), \"AllowDownloads\" (default: False).";
MathemeToGraphics::usage = "MathemeToGraphics[source, inputs] compiles Typst source and imports the PDF pages as vector Graphics. Accepts the same options as MathemeCompile and returns failures unchanged.";
MathemeVersion::usage = "MathemeVersion[] returns <|\"Matheme\" -> version, \"Typst\" -> version, \"TypstCommit\" -> commit|>. Configuration is reported by MathemeSettings[].";
MathemeSettings::usage = "MathemeSettings[] returns the effective compiler settings: FontPaths, SystemFonts, EmbeddedFonts, PackagePath, PackageCache, Proxy, Downloads and HostFontPaths. MathemeSettings[association] applies a patch and also returns Changed, FontsReloaded, PackagesRebuilt, FontCounts and package information. Null resets a field; \"Reset\" -> True restores every default.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans all configured font sources and returns the number of faces found.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] rebuilds the package loader (dropping the cached Universe index) and returns package information.";
MathemeWarmup::usage = "MathemeWarmup[] eagerly initializes the font cache, package loader and Universe index. Options: \"Fonts\", \"Packages\", \"PackageIndex\" (all True by default). Returns per-stage timings; PackageIndex needs network access.";
MathemeQueryFonts::usage = "MathemeQueryFonts[] and MathemeQueryFonts[filters] return the cached font-book metadata (family, variant, flags, source, path, collection index and residency). Filters: Family, Source, Resident, Character; paging: Offset, MaxResults (default 50, All for everything). Queries never load a font file.";
MathemeQueryPackages::usage = "MathemeQueryPackages[] and MathemeQueryPackages[filters] return the cached package metadata (locally installed packages plus the Universe index once fetched). Filters: Name, Namespace, Origin, Version; paging: Offset, MaxResults; opt-ins: Refresh (rescan local directories), AllowDownloads (fetch the Universe index).";
MathemeClose::usage = "MathemeClose[] drops every library cache (fonts, packages, memoization), closes the WSTP LinkObjects, unloads the library functions and releases the DLL image. The library is reloaded automatically on the next call.";

MathemeLibraryError::usage = "MathemeLibraryError is the Failure tag used when the WSTP library cannot be loaded or returns an unexpected result.";
MathemeCallError::usage = "MathemeCallError is the Failure tag used when a library call reports a failure (invalid arguments, bad configuration, network errors).";
MathemeCompileError::usage = "MathemeCompileError is the Failure tag used when Typst compilation fails. The Failure carries Diagnostics and Formatted.";
MathemeDiagnostic::usage = "MathemeDiagnostic::warning is the message issued for every non-fatal diagnostic; use Off[MathemeDiagnostic::warning] to silence warnings.";

Begin["`Private`"];

(* Register the three failure tags once, when the package is loaded. *)
Scan[CreateErrorType[#, {}] &,
  {MathemeLibraryError, MathemeCallError, MathemeCompileError}];

$errorTags = <|
  "Library" -> MathemeLibraryError,
  "Call" -> MathemeCallError,
  "Compile" -> MathemeCompileError
|>;

$displayNames = <|
  "compile" -> "MathemeCompile",
  "version" -> "MathemeVersion",
  "settings" -> "MathemeSettings",
  "warmup" -> "MathemeWarmup",
  "reloadFonts" -> "MathemeReloadFonts",
  "reloadPackages" -> "MathemeReloadPackages",
  "queryFonts" -> "MathemeQueryFonts",
  "queryPackages" -> "MathemeQueryPackages",
  "close" -> "MathemeClose"
|>;

MathemeDiagnostic::warning = "Matheme `1`: `2`";

$library = Module[{found = Quiet[FindLibrary["matheme"]]},
  If[TrueQ[found =!= $Failed],
    found,
    FileNameJoin[{DirectoryName[DirectoryName[$Input]], "LibraryResources",
      $SystemID, "matheme.dll"}]
  ]
];

(* LibraryFunctionLoad with LinkObject creates an extra LinkObject that keeps
   the DLL open. Load lazily so MathemeClose can release it and later calls
   can transparently load the library again. *)
$functions = <||>;
$links = {};

$expectedFunctions = 9;

loadFunctions[] := Module[{before, loaded, good},
  If[Length[$functions] == $expectedFunctions, Return[True]];
  before = Links[];
  loaded = <|
    "compile" -> LibraryFunctionLoad[$library, "matheme_compile", LinkObject, LinkObject],
    "version" -> LibraryFunctionLoad[$library, "matheme_version", LinkObject, LinkObject],
    "settings" -> LibraryFunctionLoad[$library, "matheme_settings", LinkObject, LinkObject],
    "warmup" -> LibraryFunctionLoad[$library, "matheme_warmup", LinkObject, LinkObject],
    "reloadFonts" -> LibraryFunctionLoad[$library, "matheme_reload_fonts", LinkObject, LinkObject],
    "reloadPackages" -> LibraryFunctionLoad[$library, "matheme_reload_packages", LinkObject, LinkObject],
    "queryFonts" -> LibraryFunctionLoad[$library, "matheme_query_fonts", LinkObject, LinkObject],
    "queryPackages" -> LibraryFunctionLoad[$library, "matheme_query_packages", LinkObject, LinkObject],
    "close" -> LibraryFunctionLoad[$library, "matheme_close", LinkObject, LinkObject]
  |>;
  good = Select[Values[loaded], Head[#] === LibraryFunction &];
  If[Length[good] === $expectedFunctions,
    $functions = loaded;
    $links = Complement[Links[], before];
    Return[True]
  ];
  (* Partial failure: release whatever was loaded. *)
  Scan[Quiet[LibraryFunctionUnload[#]] &, good];
  $functions = <||>;
  $links = {};
  False
];

diagnosticText[diag_Association] := Module[
  {loc = Lookup[diag, "Location", None], message = Lookup[diag, "Message", ""]},
  If[AssociationQ[loc],
    StringRiffle[
      {Lookup[loc, "File", "?"], ToString[Lookup[loc, "Line", 0]],
       ToString[Lookup[loc, "Column", 0]]}, ":"] <> ": " <> message,
    message]
];

(* Warnings are messages; errors are carried by the Failure object. *)
issueDiagnostics[name_, diagnostics_] := Scan[
  Function[diag,
    If[AssociationQ[diag] && Lookup[diag, "Severity", "Warning"] === "Warning",
      Message[MathemeDiagnostic::warning, Lookup[$displayNames, name, name],
        diagnosticText[diag]]]
  ],
  diagnostics
];

failureDetails[name_, result_] := <|
  "MessageTemplate" -> "Matheme `Operation` failed: `Detail`",
  "MessageParameters" -> <|
    "Operation" -> Lookup[$displayNames, name, name],
    "Detail" -> Lookup[result, "Message", "unknown error"]
  |>,
  "Message" -> Lookup[result, "Message", "unknown error"],
  "Operation" -> Lookup[$displayNames, name, name],
  "ErrorTag" -> Lookup[result, "ErrorTag", "Call"],
  "Diagnostics" -> Lookup[result, "Diagnostics", {}],
  "Formatted" -> Lookup[result, "Formatted", ""]
|>;

callUnchecked[name_String, args___] := Module[{result, tag},
  If[!loadFunctions[],
    With[{t = MathemeLibraryError},
      Raise[t, <|
        "MessageTemplate" -> "Could not load the matheme library: `Library`.",
        "MessageParameters" -> <|"Library" -> $library|>
      |>]]
  ];
  result = $functions[name][args];
  If[!AssociationQ[result],
    With[{t = MathemeLibraryError},
      Raise[t, <|
        "MessageTemplate" -> "The matheme library returned an unexpected result for `Operation`.",
        "MessageParameters" -> <|"Operation" -> Lookup[$displayNames, name, name]|>,
        "Result" -> result
      |>]]
  ];
  issueDiagnostics[name, Lookup[result, "Diagnostics", {}]];
  If[Lookup[result, "Status", "Error"] =!= "Success",
    tag = Lookup[$errorTags, Lookup[result, "ErrorTag", "Call"], MathemeCallError];
    With[{t = tag, details = failureDetails[name, result]}, Raise[t, details]]
  ];
  KeyDrop[result, "Status"]
];

(* Every public call goes through the same pipeline: success returns the
   association without its internal Status field, failure returns a Failure. *)
callFunction[name_String, args___] := Handle[callUnchecked[name, args], _Failure];

compileWith[source_, inputs_, base_, allowDownloads_] := callFunction["compile", <|
  "Source" -> source,
  "Inputs" -> inputs,
  "BasePath" -> base,
  "AllowDownloads" -> allowDownloads
|>];

Options[MathemeCompile] = {"Inputs" -> <||>, "BasePath" -> Automatic, "AllowDownloads" -> False};
Options[MathemeToGraphics] = Options[MathemeCompile];

MathemeCompile[source_String, opts : OptionsPattern[]] := compileWith[
  source,
  OptionValue["Inputs"],
  If[OptionValue["BasePath"] === Automatic, Directory[], OptionValue["BasePath"]],
  TrueQ[OptionValue["AllowDownloads"]]
];

MathemeCompile[source_String, inputs_Association, opts : OptionsPattern[]] := compileWith[
  source,
  inputs,
  If[OptionValue["BasePath"] === Automatic, Directory[], OptionValue["BasePath"]],
  TrueQ[OptionValue["AllowDownloads"]]
];

toGraphics[result_] := Module[{pdf},
  If[FailureQ[result] || !AssociationQ[result], Return[result]];
  pdf = Lookup[result, "PDF", Null];
  If[Head[pdf] =!= ByteArray, Return[Join[result, <|"Graphics" -> {}|>]]];
  Join[result, <|"Graphics" -> ImportByteArray[pdf, {"PDF", "PageGraphics"}]|>]
];

MathemeToGraphics[source_String, opts : OptionsPattern[]] :=
  toGraphics[MathemeCompile[source, opts]];
MathemeToGraphics[source_String, inputs_Association, opts : OptionsPattern[]] :=
  toGraphics[MathemeCompile[source, inputs, opts]];

MathemeVersion[] := callFunction["version"];
MathemeReloadFonts[] := callFunction["reloadFonts"];
MathemeReloadPackages[] := callFunction["reloadPackages"];

MathemeSettings[] := callFunction["settings"];
MathemeSettings[settings_Association] := callFunction["settings", settings];

MathemeQueryFonts[] := callFunction["queryFonts"];
MathemeQueryFonts[filters_Association] := callFunction["queryFonts", filters];

MathemeQueryPackages[] := callFunction["queryPackages"];
MathemeQueryPackages[filters_Association] := callFunction["queryPackages", filters];

Options[MathemeWarmup] = {"Fonts" -> True, "Packages" -> True, "PackageIndex" -> True};
MathemeWarmup[opts : OptionsPattern[]] := callFunction["warmup", <|
  "Fonts" -> TrueQ[OptionValue["Fonts"]],
  "Packages" -> TrueQ[OptionValue["Packages"]],
  "PackageIndex" -> TrueQ[OptionValue["PackageIndex"]]
|>];

MathemeClose[] := Module[
  {before = Links[], unloaded = 0, links = $links, released = <||>,
   dllUnloaded = False},
  If[Length[$functions] == $expectedFunctions,
    (* LibraryFunctionUnload already drops the last module reference on
       Wolfram 15, so a later LibraryUnload would report "cannot be unloaded"
       even though the DLL was released. Pin one extra reference here so the
       final LibraryUnload has a real mapping to release and returns Null. *)
    LibraryLoad[$library];
    released = callFunction["close"];
    If[!AssociationQ[released], released = <||>];
    Do[
      If[Head[f] === LibraryFunction, Quiet[LibraryFunctionUnload[f]]; unloaded++],
      {f, Values[$functions]}
    ];
    (* LibraryFunctionUnload normally closes the link already; the explicit
       close keeps the frontend's own LinkObject references from lingering. *)
    Scan[Quiet[LinkClose[#]] &, links];
    $functions = <||>;
    $links = {};
    dllUnloaded = TrueQ[Quiet@LibraryUnload[$library] =!= $Failed];
  ];
  Join[released, <|
    "Unloaded" -> unloaded,
    "LinksClosed" -> Length[Complement[before, Links[]]],
    "LibraryUnloaded" -> dllUnloaded
  |>]
];

End[];

EndPackage[];
