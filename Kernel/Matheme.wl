BeginPackage["Matheme`"];

MathemeCompile::usage = "MathemeCompile[source, inputs] compiles Typst source to PDF and returns an Association with Status, PDF, Pages and Diagnostics. Options: \"Inputs\", \"BasePath\" (default: current directory), \"AllowDownloads\" (default: False).";
MathemeToGraphics::usage = "MathemeToGraphics[source, inputs] compiles Typst source and imports the PDF pages as vector Graphics. Accepts the same options as MathemeCompile.";
MathemeVersion::usage = "MathemeVersion[] returns version information, package directories and the download setting.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans system fonts and returns the number of fonts found.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] drops the cached Universe index and returns package cache information.";

Begin["`Private`"];

$library = Module[{found = Quiet[FindLibrary["matheme"]]},
  If[TrueQ[found =!= $Failed],
    found,
    FileNameJoin[{DirectoryName[DirectoryName[$Input]], "LibraryResources",
      $SystemID, "matheme.dll"}]
  ]
];

$compile = LibraryFunctionLoad[$library, "matheme_compile", LinkObject, LinkObject];
$version = LibraryFunctionLoad[$library, "matheme_version", LinkObject, LinkObject];
$reloadFonts = LibraryFunctionLoad[$library, "matheme_reload_fonts", LinkObject, LinkObject];
$reloadPackages = LibraryFunctionLoad[$library, "matheme_reload_packages", LinkObject, LinkObject];

compileWith[source_, inputs_, base_, allowDownloads_] := $compile[<|
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

MathemeVersion[] := $version[];

MathemeReloadFonts[] := $reloadFonts[];

MathemeReloadPackages[] := $reloadPackages[];

End[];

EndPackage[];
