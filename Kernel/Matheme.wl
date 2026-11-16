BeginPackage["Matheme`"];

MathemeCompile::usage = "MathemeCompile[source, inputs] compiles Typst source to PDF and returns an Association with Status, PDF, Pages and Diagnostics.";
MathemeToGraphics::usage = "MathemeToGraphics[source, inputs] compiles Typst source and imports the PDF pages as vector Graphics.";
MathemeVersion::usage = "MathemeVersion[] returns version information for the Rust bridge, Typst and the paclet.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans system fonts and returns the number of fonts found.";

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

MathemeCompile[source_String, inputs_Association: <||>] := $compile[<|"Source" -> source, "Inputs" -> inputs|>];

MathemeToGraphics[source_String, inputs_Association: <||>] := Module[
  {result, pdf, pages},
  result = MathemeCompile[source, inputs];
  If[!AssociationQ[result], Return[result]];
  If[result["Status"] =!= "Success", Return[result]];
  pdf = Lookup[result, "PDF", Null];
  pages = If[Head[pdf] === ByteArray,
    ImportByteArray[pdf, {"PDF", "PageGraphics"}],
    {}
  ];
  Join[result, <|"Graphics" -> pages|>]
];

MathemeVersion[] := $version[];

MathemeReloadFonts[] := $reloadFonts[];

End[];

EndPackage[];
