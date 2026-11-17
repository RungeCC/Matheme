BeginPackage["Matheme`"];

MathemeCompile::usage = "MathemeCompile[source, inputs] compiles Typst source to PDF and returns an Association with Status, PDF, Pages, Diagnostics and Formatted (typst-cli style report). Each diagnostic has a Location with File, 1-based Line/Column, byte Range and source Text. Inputs may contain String, Integer, Real, True/False/Null, List, Association and ByteArray (Typst bytes). Options: \"Inputs\", \"BasePath\" (default: current directory), \"AllowDownloads\" (default: False).";
MathemeToGraphics::usage = "MathemeToGraphics[source, inputs] compiles Typst source and imports the PDF pages as vector Graphics. Accepts the same options as MathemeCompile.";
MathemeVersion::usage = "MathemeVersion[] returns version information (Matheme, Typst and Typst commit), the effective settings, font counts per source, Wolfram bundled-font directories/count and package directories.";
MathemeSettings::usage = "MathemeSettings[] returns the effective compiler settings: FontPaths (highest-priority font directories), SystemFonts, EmbeddedFonts, PackagePath, PackageCache, Proxy and Downloads. MathemeSettings[association] applies a patch and returns Status, Changed, Settings, FontCounts and package information. Null resets a field; \"Reset\" -> True restores every default. Settings replace environment variables.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans all configured font sources (custom paths, Wolfram, embedded, system) and returns the number of faces found.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] rebuilds the package loader (dropping the cached Universe index) and returns package information.";
MathemeWarmup::usage = "MathemeWarmup[] eagerly initializes the font cache, package loader and Universe index so the first MathemeCompile is fast. Options: \"Fonts\", \"Packages\", \"PackageIndex\" (all True by default). Returns per-stage timings; PackageIndex needs network access. The library is loaded if needed.";
MathemeQuery::usage = "MathemeQuery[] reports the state of the process-wide caches. MathemeQuery[\"Fonts\"] and MathemeQuery[\"Packages\"] return the cached metadata; MathemeQuery[target, filters] and MathemeQuery[association] add filters (Family, Source, Resident, Character, Name, Namespace, Origin, Version), paging (Offset, MaxResults -> All) and the opt-ins AllowDownloads and Refresh. Queries never load font bytes or reach the network unless AllowDownloads -> True is given.";
MathemeClose::usage = "MathemeClose[] closes the WSTP LinkObjects and asks the library to drop its caches (font book, lazily loaded font faces, package loader, memoized compilation results), returning that memory to the operating system. The library functions are unloaded and the DLL image is released; the library is reloaded automatically on the next call.";

Begin["`Private`"];

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

$expectedFunctions = 8;

loadFunctions[] := Module[{before, loaded, good},
  If[Length[$functions] == $expectedFunctions, Return[True]];
  before = Links[];
  loaded = <|
    "compile" -> LibraryFunctionLoad[$library, "matheme_compile", LinkObject, LinkObject],
    "version" -> LibraryFunctionLoad[$library, "matheme_version", LinkObject, LinkObject],
    "reloadFonts" -> LibraryFunctionLoad[$library, "matheme_reload_fonts", LinkObject, LinkObject],
    "reloadPackages" -> LibraryFunctionLoad[$library, "matheme_reload_packages", LinkObject, LinkObject],
    "warmup" -> LibraryFunctionLoad[$library, "matheme_warmup", LinkObject, LinkObject],
    "settings" -> LibraryFunctionLoad[$library, "matheme_settings", LinkObject, LinkObject],
    "query" -> LibraryFunctionLoad[$library, "matheme_query", LinkObject, LinkObject],
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

callFunction[name_String, args___] := Module[{},
  If[!loadFunctions[],
    Return[<|"Status" -> "Error", "PDF" -> Null, "Pages" -> 0,
      "Diagnostics" -> {}, "Formatted" -> "Failed to load matheme library\n"|>]
  ];
  $functions[name][args]
];

MathemeClose[] := Module[{before, unloaded = 0, links = $links, released = False, dllUnloaded, result},
  (* Ask the library to drop its process-wide caches first; this must happen
     while the functions are still loaded. *)
  If[Length[$functions] == $expectedFunctions,
    result = Quiet@Check[$functions["close"][], $Failed];
    released = AssociationQ[result] && TrueQ[result["Status"] === "Success"]];
  before = Links[];
  Do[
    If[Head[f] === LibraryFunction, Quiet[LibraryFunctionUnload[f]]; unloaded++],
    {f, Values[$functions]}
  ];
  (* LibraryFunctionUnload normally closes the link already; the explicit
     close keeps the frontend's own LinkObject references from lingering. *)
  Scan[Quiet[LinkClose[#]] &, links];
  $functions = <||>;
  $links = {};
  (* Best effort: unmap the DLL so even its statics return to the OS. Wolfram
     15 currently refuses this even for trivial libraries, hence the report. *)
  dllUnloaded = Quiet@Check[LibraryUnload[$library]; True, False];
  <|
    "Status" -> "Success",
    "Unloaded" -> unloaded,
    "LinksClosed" -> Length[Complement[before, Links[]]],
    "CachesReleased" -> released,
    "LibraryUnloaded" -> dllUnloaded
  |>
];

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

MathemeToGraphics[source_String, opts : OptionsPattern[]] := Module[
  {result = MathemeCompile[source, opts], pdf},
  If[!AssociationQ[result], Return[result]];
  If[result["Status"] =!= "Success", Return[result]];
  pdf = Lookup[result, "PDF", Null];
  If[Head[pdf] =!= ByteArray, Return[Join[result, <|"Graphics" -> {}|>]]];
  Join[result, <|"Graphics" -> ImportByteArray[pdf, {"PDF", "PageGraphics"}]|>]
];

MathemeToGraphics[source_String, inputs_Association, opts : OptionsPattern[]] := Module[
  {result = MathemeCompile[source, inputs, opts], pdf},
  If[!AssociationQ[result], Return[result]];
  If[result["Status"] =!= "Success", Return[result]];
  pdf = Lookup[result, "PDF", Null];
  If[Head[pdf] =!= ByteArray, Return[Join[result, <|"Graphics" -> {}|>]]];
  Join[result, <|"Graphics" -> ImportByteArray[pdf, {"PDF", "PageGraphics"}]|>]
];

MathemeVersion[] := callFunction["version"];
MathemeReloadFonts[] := callFunction["reloadFonts"];
MathemeReloadPackages[] := callFunction["reloadPackages"];

MathemeSettings[] := callFunction["settings"];
MathemeSettings[settings_Association] := callFunction["settings", settings];

MathemeQuery[] := callFunction["query"];
MathemeQuery[target_String] := callFunction["query", target];
MathemeQuery[filters_Association] := callFunction["query", filters];
MathemeQuery[target_String, filters_Association] := callFunction["query", target, filters];
(* Anything else is forwarded so the Rust side can report a shaped error
   instead of leaving an unevaluated Wolfram expression behind. *)
MathemeQuery[args___] := callFunction["query", args];

Options[MathemeWarmup] = {"Fonts" -> True, "Packages" -> True, "PackageIndex" -> True};
MathemeWarmup[opts : OptionsPattern[]] := callFunction["warmup", <|
  "Fonts" -> TrueQ[OptionValue["Fonts"]],
  "Packages" -> TrueQ[OptionValue["Packages"]],
  "PackageIndex" -> TrueQ[OptionValue["PackageIndex"]]
|>];

End[];

EndPackage[];
