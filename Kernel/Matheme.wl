(* ::Package:: *)

(* The failure representation follows the convention in
   Wolfram/ErrorTools: private code raises Failure objects, public functions
   catch them and return them normally. *)

(* A reload must close any open WSTP link / DLL reference first, then drop
   both public and private symbols before re-defining them. *)
If[MemberQ[$Packages, "Runge`Matheme`"],
  Quiet[Runge`Matheme`MathemeClose[]];
  Unprotect["Runge`Matheme`*", "Runge`Matheme`Private`*", "Runge`Matheme`Private`TypstForm`*"];
  ClearAll["Runge`Matheme`*", "Runge`Matheme`Private`*", "Runge`Matheme`Private`TypstForm`*"]
];

BeginPackage["Runge`Matheme`", {"Wolfram`ErrorTools`"}];

MathemeCompile::usage = "MathemeCompile[source, opts] compiles Typst source to \"PDF\", \"SVG\", \"HTML\", or \"Graphics\" (with optional \"Evaluate\", \"Pages\", \"PDFStandard\", \"PDFTags\", \"CreationTimestamp\", \"Features\", \"Inputs\", \"BasePath\", \"AllowDownloads\", and \"TypstTemplate\"). Paged formats return \"Pages\" (exported page count) and \"TotalPages\" (full compiled page count).";
MathemeToGraphics::usage = "MathemeToGraphics[source, opts] compiles Typst source and returns vector Graphics pages (directly from Typst's Frame tree by default, or imported when \"Format\" is \"SVG\" or \"PDF\"). Rejects non-vector formats such as \"HTML\".";
TypstForm::usage = "TypstForm[expr, opts] prints as a customizable Typst math expression representation of expr.";
MathemeToTypst::usage = "MathemeToTypst[expr, opts] converts expr to a Typst math String. Supports customization via \"Form\", \"ExpressionRules\", \"BoxRules\", \"TemplateRules\", \"CharacterRules\", \"OperatorNames\", \"DelimiterStyle\", \"FractionStyle\", \"ConstantStyle\", \"DifferentialStyle\", \"TransposeSymbol\", \"ConjugateStyle\", \"BlackboardStyle\", \"ScriptStyle\", \"MultiLetterStyle\", \"MatrixDelimiter\", \"PreserveStyle\", \"StripContext\", \"UnknownCharacterHandler\", and \"UnsupportedBoxHandler\".";
BoxesToTypst::usage = "BoxesToTypst[boxes, opts] converts a Wolfram Language Box expression to a Typst math String.";
TypstRaw::usage = "TypstRaw[str] embeds a literal Typst math fragment verbatim inside expressions converted by TypstForm or MathemeToTypst.";
MakeTypst::usage = "MakeTypst[expr] (or MakeTypst[expr, conv]) is an extensible UpValue formatting hook checked by TypstForm and MathemeToTypst before boxing expr.";
MathemeVersion::usage = "MathemeVersion[] returns version metadata for Matheme, Typst, LibraryLink, and WSTP.";
SetMatheme::usage = "SetMatheme[] returns the active compiler settings Association. SetMatheme[key -> val, ...] updates process-wide settings (Null resets a field) and returns <|\"Changed\" -> ..., \"Settings\" -> ...|>.";
ResetMatheme::usage = "ResetMatheme[] restores user settings to defaults and returns <|\"Changed\" -> ..., \"Settings\" -> ...|>.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans configured font sources and returns face counts.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] rebuilds the package loader and clears the cached Universe index.";
MathemeWarmup::usage = "MathemeWarmup[opts] eagerly initializes \"Fonts\", \"Packages\", and \"PackageIndex\" caches.";
MathemeQueryFonts::usage = "MathemeQueryFonts[filters] queries cached font metadata without loading font files.";
MathemeQueryPackages::usage = "MathemeQueryPackages[filters] queries cached local and Universe package metadata.";
ClearMathemeCache::usage = "ClearMathemeCache[targets] drops \"Fonts\", \"Packages\", \"Memoization\", or All caches without unloading the library.";
MathemeClose::usage = "MathemeClose[] clears all caches, closes the WSTP link, and unloads the DLL.";

MathemeLibraryError::usage = "MathemeLibraryError is the Failure tag for library load, WSTP transport, or bridge protocol errors.";
MathemeCallError::usage = "MathemeCallError is the Failure tag for invalid arguments, options, or settings.";
MathemeCompileError::usage = "MathemeCompileError is the Failure tag for Typst compilation, export, and evaluation errors.";
MathemeInternalError::usage = "MathemeInternalError is the Failure tag for unexpected internal backend panics or invariant violations.";
MathemeDiagnostic::usage = "MathemeDiagnostic::warning is issued for non-fatal warning diagnostics.";
TypstForm::unspt = "TypstForm of `1` is not supported.";
TypstForm::unkchar = "Unmapped Wolfram private-use character `1` (`2`) encountered in TypstForm; falling back to text literal.";

Begin["`Private`"];

(* Core WSTP bridge, library lifecycle, diagnostics, and MathemeClose. *)
Get[FileNameJoin[{DirectoryName[$InputFileName], "Core.wl"}]];

(* ------------------------------------------------------------------------ *)
(* Templates                                                                *)
(* ------------------------------------------------------------------------ *)

makeMiTeXTemplate[fnName_String] := TemplateObject[{
    "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n",
    "#import \"@preview/mitex:0.2.7\": " <> fnName <> "\n",
    "#" <> fnName <> "(\"", TemplateSlot[1], "\")"},
  CombinerFunction -> StringJoin,
  InsertionFunction -> (StringReplace[TextString[#], {"\\" -> "\\\\", "\"" -> "\\\""}] &),
  MetaInformation -> <| |>
];

$TypstTemplates = <|
  "Equation" -> StringTemplate["#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$ `` $"],
  "InlineEquation" -> StringTemplate["#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$``$"],
  "Text" -> StringTemplate["#set page(width: auto, height: auto, margin: 0pt, fill: none)\n``"],
  "MiTeXEquation" -> makeMiTeXTemplate["mitex"],
  "MiTeXInlineEquation" -> makeMiTeXTemplate["mi"],
  "MiTeXText" -> makeMiTeXTemplate["mitext"]
|>;

raiseUnknownTemplate[name_] := raiseCallError[
  "Unknown \"TypstTemplate\" `Name`. Available keys: `Keys`.",
  <|
    "Name" -> name,
    "Keys" -> StringRiffle[Keys[$TypstTemplates], ", "]
  |>
];

applyTemplate[None, source_String] := source;
applyTemplate[name_String, source_String] := Module[{template},
  template = Lookup[$TypstTemplates, name, $Failed];
  If[template === $Failed, raiseUnknownTemplate[name]];
  applyTemplate[template, source]
];
applyTemplate[template_StringTemplate, source_String] := TemplateApply[template, source];
applyTemplate[template_TemplateObject, source_String] := TemplateApply[template, source];
applyTemplate[template_Function, source_String] := template[source];
applyTemplate[other_, _String] := raiseCallError[
  "\"TypstTemplate\" must be None, a built-in template name (`Keys`), or a template; got `Type`.",
  <|
    "Keys" -> StringRiffle[Keys[$TypstTemplates], ", "],
    "Type" -> Head[other]
  |>
];

(* ------------------------------------------------------------------------ *)
(* Compilation                                                              *)
(* ------------------------------------------------------------------------ *)

normalizeTimestamp[Automatic] := Automatic;
normalizeTimestamp[None | Null] := None;
normalizeTimestamp[ts_Integer] := ts;
normalizeTimestamp[assoc_Association] := assoc;
normalizeTimestamp[dt_DateObject] := Module[{unix, tz},
  unix = Quiet[UnixTime[dt]];
  If[!IntegerQ[unix],
    raiseCallError["Invalid DateObject for \"CreationTimestamp\"."]
  ];
  tz = Quiet[DateValue[dt, "TimeZone"]];
  If[NumericQ[tz],
    <|"Unix" -> unix, "OffsetMinutes" -> Round[60 * tz]|>,
    unix
  ]
];
normalizeTimestamp[other_] := raiseCallError[
  "\"CreationTimestamp\" must be Automatic, None, a DateObject, or a Unix Integer; got `Type`.",
  <|"Type" -> Head[other]|>
];

normalizeFeatures[list_List] := list;
normalizeFeatures[single_String] := {single};
normalizeFeatures[other_] := raiseCallError[
  "\"Features\" must be a String or List of Strings; got `Type`.",
  <|"Type" -> Head[other]|>
];

Options[MathemeCompile] = {
  "Inputs" -> <||>,
  "BasePath" -> Automatic,
  "AllowDownloads" -> False,
  "TypstTemplate" -> None,
  "Format" -> "PDF",
  "Pages" -> All,
  "PDFStandard" -> Automatic,
  "PDFTags" -> True,
  "CreationTimestamp" -> Automatic,
  "Features" -> {},
  "Evaluate" -> None
};
Options[compileChecked] = Options[MathemeCompile];

compileChecked[source_String, OptionsPattern[]] := Module[{rendered},
  rendered = applyTemplate[OptionValue["TypstTemplate"], source];
  If[!StringQ[rendered],
    raiseCallError[
      "\"TypstTemplate\" must produce a String; got `Type`.",
      <|"Type" -> Head[rendered]|>
    ]
  ];
  callUnchecked["compile", <|
    "Source" -> rendered,
    "Inputs" -> OptionValue["Inputs"],
    "BasePath" -> If[OptionValue["BasePath"] === Automatic, Directory[], OptionValue["BasePath"]],
    "AllowDownloads" -> OptionValue["AllowDownloads"],
    "Format" -> OptionValue["Format"],
    "Pages" -> OptionValue["Pages"],
    "PDFStandard" -> OptionValue["PDFStandard"],
    "PDFTags" -> OptionValue["PDFTags"],
    "CreationTimestamp" -> normalizeTimestamp[OptionValue["CreationTimestamp"]],
    "Features" -> normalizeFeatures[OptionValue["Features"]],
    "Evaluate" -> OptionValue["Evaluate"]
  |>]
];

(* Template and option validation also raise through ErrorTools, so the public
   entry handles pre-call and library failures in one Handle scope. *)
MathemeCompile[source_String, opts : OptionsPattern[]] :=
  Handle[_Failure] @ compileChecked[source, opts];
MathemeCompile[TypstForm[expr_, tOpts : OptionsPattern[]], opts : OptionsPattern[]] :=
  Module[{src = Runge`Matheme`Private`TypstForm`typstFormToString[expr, tOpts], flatOpts = Flatten[{opts}], merged},
    merged = If[
      KeyExistsQ[Association[flatOpts], "TypstTemplate"] ||
        OptionValue[MathemeCompile, "TypstTemplate"] =!= None,
      flatOpts,
      Append[flatOpts, "TypstTemplate" -> "Equation"]
    ];
    MathemeCompile[src, Sequence @@ merged]
  ];

(* Wolfram's SVG importer converts width/height from pt to px (4/3) when
   computing PlotRange while keeping path coordinates in viewBox user units.
   Strip the "pt" suffix on the root <svg> dimensions so PlotRange matches
   the actual geometry 1:1. *)
normalizeSvgUnits[svg_String] := StringReplace[svg,
  StartOfString ~~ head : ("<svg" ~~ Shortest[___] ~~ ">") :>
    StringReplace[head, {
      ("width=\"" ~~ w : NumberString ~~ "pt\"") :> ("width=\"" <> w <> "\""),
      ("height=\"" ~~ h : NumberString ~~ "pt\"") :> ("height=\"" <> h <> "\"")
    }]
];

(* Extract numeric {x, y} coordinate bounds from Graphics primitives as a
   fallback when AbsoluteOptions[_, PlotRange] fails to return a 2x2 matrix. *)
extractCoordBounds[g_Graphics] := Module[{pts},
  pts = Cases[First[g], {x_?NumericQ, y_?NumericQ} :> {N[x], N[y]}, Infinity];
  If[Length[pts] > 0,
    {MinMax[pts[[All, 1]]], MinMax[pts[[All, 2]]]},
    None
  ]
];

validBoundsQ[pr_] := MatrixQ[pr, NumericQ] && Dimensions[pr] === {2, 2};

(* PDF and SVG importers set PlotRange strictly to the page MediaBox/viewBox,
   which can clip glyph descenders or italic overhangs when margin is 0pt;
   switching blindly to PlotRange -> All would add default PlotRangePadding,
   leave ImageSize stale, and drop fixed page margins (e.g. A4 PDF pages).
   Unify all formats with "Format" -> "Graphics": take the union of the page
   box and the actual primitive content bounds via AbsoluteOptions (falling
   back to explicit coordinate extraction if AbsoluteOptions fails), and set
   PlotRangePadding -> None, ImagePadding -> None, and exact ImageSize -> {w, h}. *)
uncropGraphics[g_Graphics] := Module[
  {opts, origPR, absOpts, absPR, contentPR, boxes, x0, x1, y0, y1, w, h},
  opts = Association[Rest[List @@ g]];
  origPR = Lookup[opts, PlotRange, None];
  absOpts = Check[
    Quiet[
      AbsoluteOptions[
        Graphics[First[g], PlotRange -> All, PlotRangePadding -> None],
        PlotRange
      ]
    ],
    $Failed
  ];
  absPR = If[absOpts === $Failed, None, PlotRange /. absOpts];
  contentPR = If[validBoundsQ[absPR], absPR, extractCoordBounds[g]];
  boxes = Select[{origPR, contentPR}, validBoundsQ];
  If[Length[boxes] > 0,
    {x0, x1} = MinMax[Flatten[boxes[[All, 1]]]];
    {y0, y1} = MinMax[Flatten[boxes[[All, 2]]]];
    w = Max[1., x1 - x0];
    h = Max[1., y1 - y0];
    Graphics[
      First[g],
      Sequence @@ Normal[Join[opts, <|
        PlotRange -> {{x0, x1}, {y0, y1}},
        PlotRangePadding -> None,
        ImagePadding -> None,
        ImageSize -> {w, h},
        AspectRatio -> Automatic
      |>]]
    ],
    Message[
      MathemeDiagnostic::warning,
      "MathemeToGraphics",
      "Could not determine numeric PlotRange bounds; falling back to PlotRange -> All."
    ];
    Graphics[
      First[g],
      Sequence @@ Normal[Join[opts, <|
        PlotRange -> All,
        PlotRangePadding -> None,
        ImagePadding -> None,
        AspectRatio -> Automatic
      |>]]
    ]
  ]
];
uncropGraphics[other_] := other;

validateGraphicsFormat[fmt_] := If[
  StringQ[fmt] && MemberQ[{"graphics", "vector", "svg", "pdf"}, ToLowerCase[fmt]],
  fmt,
  raiseCallError[
    "MathemeToGraphics only supports vector formats \"Graphics\", \"SVG\", or \"PDF\"; got `Format`.",
    <|"Format" -> fmt|>
  ]
];

toGraphics[result_] := Module[{pdf, svg, graphics, info},
  If[FailureQ[result] || !AssociationQ[result],
    result,
    graphics = Lookup[result, "Graphics", Null];
    pdf = Lookup[result, "PDF", Null];
    svg = Lookup[result, "SVG", Null];
    info = KeyDrop[result, {"PDF", "SVG", "Graphics"}];
    Which[
      ListQ[graphics] && AllTrue[graphics, Head[#] === Graphics &],
        result,
      ListQ[svg],
        graphics = Map[ImportString[normalizeSvgUnits[#], {"SVG", "Graphics"}] &, svg];
        If[!AllTrue[graphics, Head[#] === Graphics &],
          raiseCallError["Failed to import SVG output as Graphics."],
          Join[<|"Graphics" -> Map[uncropGraphics, graphics]|>, info]
        ],
      Head[pdf] === ByteArray,
        graphics = ImportByteArray[pdf, {"PDF", "PageGraphics"}];
        If[!ListQ[graphics],
          raiseCallError["Failed to import PDF bytes as Graphics."],
          Join[<|"Graphics" -> Map[uncropGraphics, graphics]|>, info]
        ],
      True,
        raiseCallError["MathemeToGraphics received an unsupported compilation result format."]
    ]
  ]
];

Options[MathemeToGraphics] = {
  "Inputs" -> <||>,
  "BasePath" -> Automatic,
  "AllowDownloads" -> False,
  "TypstTemplate" -> None,
  "Format" -> "Graphics",
  "Pages" -> All,
  "PDFStandard" -> Automatic,
  "PDFTags" -> True,
  "CreationTimestamp" -> Automatic,
  "Features" -> {},
  "Evaluate" -> None
};

MathemeToGraphics[source : (_String | _TypstForm), opts : OptionsPattern[]] :=
  Handle[_Failure] @
    Module[{flatOpts = Flatten[{opts}], explicitAssoc, defaultsAssoc, effectiveAssoc, fmt},
      explicitAssoc = Association[flatOpts];
      defaultsAssoc = Association[Options[MathemeToGraphics]];
      effectiveAssoc = Join[defaultsAssoc, explicitAssoc];
      fmt = Lookup[effectiveAssoc, "Format", "Graphics"];
      validateGraphicsFormat[fmt];
      (* For TypstForm inputs, preserve MathemeCompile's automatic "Equation"
         default unless the caller or SetOptions[MathemeToGraphics, ...]
         explicitly configured a non-None "TypstTemplate". *)
      If[Head[source] === TypstForm &&
          !KeyExistsQ[explicitAssoc, "TypstTemplate"] &&
          Lookup[defaultsAssoc, "TypstTemplate", None] === None,
        effectiveAssoc = KeyDrop[effectiveAssoc, "TypstTemplate"]
      ];
      toGraphics[MathemeCompile[source, Sequence @@ Normal[effectiveAssoc]]]
    ];

(* ------------------------------------------------------------------------ *)
(* State, queries, and cache lifecycle                                      *)
(* ------------------------------------------------------------------------ *)

MathemeVersion[] := callFunction["version"];

SetMatheme[] := With[{res = callFunction["settings"]},
  If[AssociationQ[res] && KeyExistsQ[res, "Settings"],
    res["Settings"],
    res
  ]
];
SetMatheme[opts : ((_Rule | _RuleDelayed) .. | {(_Rule | _RuleDelayed) ...})] :=
  callFunction["setSettings", Association[Flatten[{opts}]]];
SetMatheme[other__] := Handle[_Failure] @
  With[{bad = SelectFirst[{other}, !MatchQ[#, _Rule | _RuleDelayed] &, First[{other}]]},
    raiseCallError[
      "SetMatheme expects rules (e.g. SetMatheme[\"SystemFonts\" -> False]); got `Type`.",
      <|"Type" -> Head[bad]|>
    ]
  ];

ResetMatheme[] := callFunction["resetSettings"];

MathemeReloadFonts[] := callFunction["reloadFonts"];
MathemeReloadPackages[] := callFunction["reloadPackages"];

ClearMathemeCache[] := callFunction["clearCache", <|"Targets" -> All|>];
ClearMathemeCache[All | "All"] := callFunction["clearCache", <|"Targets" -> All|>];
ClearMathemeCache[target_String] := callFunction["clearCache", <|"Targets" -> {target}|>];
ClearMathemeCache[targets : {___String}] := callFunction["clearCache", <|"Targets" -> targets|>];
ClearMathemeCache[other_] := Handle[_Failure] @
  raiseCallError[
    "ClearMathemeCache expects \"Fonts\", \"Packages\", \"Memoization\", All, or a list of targets; got `Type`.",
    <|"Type" -> Head[other]|>
  ];

MathemeQueryFonts[] := callFunction["queryFonts"];
MathemeQueryFonts[arg__] := callWithRules["queryFonts", "MathemeQueryFonts", arg];

MathemeQueryPackages[] := callFunction["queryPackages"];
MathemeQueryPackages[arg__] := callWithRules["queryPackages", "MathemeQueryPackages", arg];

Options[MathemeWarmup] = {"Fonts" -> True, "Packages" -> True, "PackageIndex" -> True};
MathemeWarmup[OptionsPattern[]] := callFunction["warmup", <|
  "Fonts" -> OptionValue["Fonts"],
  "Packages" -> OptionValue["Packages"],
  "PackageIndex" -> OptionValue["PackageIndex"]
|>];

(* ------------------------------------------------------------------------ *)
(* Package conventions                                                      *)
(* ------------------------------------------------------------------------ *)

Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {_, OptionsPattern[]}}],
  {MathemeCompile, MathemeToGraphics, TypstForm, MathemeToTypst, BoxesToTypst}
];
SyntaxInformation[TypstRaw] = {"ArgumentsPattern" -> {_}};
SyntaxInformation[MakeTypst] = {"ArgumentsPattern" -> {_, _.}};
SyntaxInformation[MathemeWarmup] = {"ArgumentsPattern" -> {OptionsPattern[]}};
SyntaxInformation[SetMatheme] = {"ArgumentsPattern" -> {OptionsPattern[]}};
Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {}}],
  {MathemeVersion, ResetMatheme, MathemeReloadFonts,
   MathemeReloadPackages, MathemeClose}
];
Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {___}}],
  {MathemeQueryFonts, MathemeQueryPackages, ClearMathemeCache}
];

Get[FileNameJoin[{DirectoryName[$InputFileName], "TypstForm.wl"}]];

End[];

(* Public functions and the public failure/message symbols are read-only. *)
Scan[
  SetAttributes[#, {Protected, ReadProtected}] &,
  {MathemeCompile, MathemeToGraphics, TypstForm, MathemeToTypst, BoxesToTypst,
   TypstRaw, MakeTypst,
   MathemeVersion, SetMatheme, ResetMatheme, MathemeReloadFonts,
   MathemeReloadPackages, MathemeWarmup, MathemeQueryFonts,
   MathemeQueryPackages, ClearMathemeCache, MathemeClose}
];
Scan[Protect, {MathemeLibraryError, MathemeCallError, MathemeCompileError,
  MathemeInternalError, MathemeDiagnostic}];

EndPackage[];

(* Register FrontEnd argument completions when loaded via Get["Runge`Matheme`"]
   (which resolves directly to Kernel/Matheme.wl) or via direct Get. *)
Get[FileNameJoin[{DirectoryName[$InputFileName], "Completions.wl"}]];
