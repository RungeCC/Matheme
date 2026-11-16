BeginPackage["Matheme`"];

MathemeCompile::usage = "MathemeCompile[source, inputs] compiles Typst source to PDF and returns an Association with Status, PDF, Pages, Diagnostics and Formatted (typst-cli style report). Each diagnostic has a Location with File, 1-based Line/Column, byte Range and source Text. Inputs may contain String, Integer, Real, True/False/Null, List, Association and ByteArray (Typst bytes). Options: \"Inputs\", \"BasePath\" (default: current directory), \"AllowDownloads\" (default: False).";
MathemeToGraphics::usage = "MathemeToGraphics[source, inputs] compiles Typst source and imports the PDF pages as vector Graphics. Accepts the same options as MathemeCompile.";
MathemeVersion::usage = "MathemeVersion[] returns version information (Matheme, Typst and Typst commit), Wolfram bundled-font directories/count, package directories and the download setting.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans system fonts and returns the number of fonts found.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] drops the cached Universe index and returns package cache information.";
MathemeWarmup::usage = "MathemeWarmup[] eagerly initializes the font cache, package loader and Universe index so the first MathemeCompile is fast. Options: \"Fonts\", \"Packages\", \"PackageIndex\" (all True by default). Returns per-stage timings; PackageIndex needs network access. The library is loaded if needed.";
MathemeClose::usage = "MathemeClose[] closes the WSTP LinkObjects, unloads the LibraryFunctions and releases the DLL file handle. The library is reloaded automatically on the next call.";

Begin["`Private`"];

(* Wolfram bundles fonts under $InstallationDirectory/SystemFiles/Fonts/TrueType;
   they are not registered as OS fonts, so point the Rust font scanner at them.
   An explicit MATHEME_WOLFRAM_DIR or MATHEME_WOLFRAM_FONTS set by the user wins. *)
Module[{current = Quiet[GetEnvironment["MATHEME_WOLFRAM_DIR"]]},
  If[!MatchQ[current, _Rule] || !StringQ[current[[2]]],
    SetEnvironment["MATHEME_WOLFRAM_DIR" -> $InstallationDirectory]
  ]
];

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

$expectedFunctions = 5;

loadFunctions[] := Module[{before, loaded, good},
  If[Length[$functions] == $expectedFunctions, Return[True]];
  before = Links[];
  loaded = <|
    "compile" -> LibraryFunctionLoad[$library, "matheme_compile", LinkObject, LinkObject],
    "version" -> LibraryFunctionLoad[$library, "matheme_version", LinkObject, LinkObject],
    "reloadFonts" -> LibraryFunctionLoad[$library, "matheme_reload_fonts", LinkObject, LinkObject],
    "reloadPackages" -> LibraryFunctionLoad[$library, "matheme_reload_packages", LinkObject, LinkObject],
    "warmup" -> LibraryFunctionLoad[$library, "matheme_warmup", LinkObject, LinkObject]
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

MathemeClose[] := Module[{before, funcs, unloaded = 0, links = $links},
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
  <|
    "Status" -> "Success",
    "Unloaded" -> unloaded,
    "LinksClosed" -> Length[Complement[before, Links[]]]
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

Options[MathemeWarmup] = {"Fonts" -> True, "Packages" -> True, "PackageIndex" -> True};
MathemeWarmup[opts : OptionsPattern[]] := callFunction["warmup", <|
  "Fonts" -> TrueQ[OptionValue["Fonts"]],
  "Packages" -> TrueQ[OptionValue["Packages"]],
  "PackageIndex" -> TrueQ[OptionValue["PackageIndex"]]
|>];

End[];

EndPackage[];
