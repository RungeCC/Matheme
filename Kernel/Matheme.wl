(* The failure representation follows the convention in
   Wolfram/ErrorTools: private code raises Failure objects, public functions
   catch them and return them normally. *)

(* A reload must close any open WSTP link / DLL reference first, then drop
   both public and private symbols before re-defining them. *)
If[MemberQ[$Packages, "Matheme`"],
  Quiet[Matheme`MathemeClose[]];
  Unprotect["Matheme`*", "Matheme`Private`*", "Matheme`Private`TypstForm`*"];
  ClearAll["Matheme`*", "Matheme`Private`*", "Matheme`Private`TypstForm`*"]
];

BeginPackage["Matheme`", {"Wolfram`ErrorTools`"}];

MathemeCompile::usage = "MathemeCompile[source, opts] compiles Typst source to \"PDF\", \"SVG\", \"HTML\", or \"Graphics\" (with optional \"Evaluate\", \"Pages\", \"PDFStandard\", \"PDFTags\", \"CreationTimestamp\", \"Features\", \"Inputs\", \"BasePath\", \"AllowDownloads\", and \"TypstTemplate\"). Paged formats return \"Pages\" (exported page count) and \"TotalPages\" (full compiled page count).";
MathemeToGraphics::usage = "MathemeToGraphics[source, opts] compiles Typst source and returns vector Graphics pages (directly from Typst's Frame tree by default, or imported when \"Format\" is \"SVG\" or \"PDF\"). Rejects non-vector formats such as \"HTML\".";
TypstForm::usage = "TypstForm[expr, opts] prints as a customizable Typst math expression representation of expr.";
MathemeToTypst::usage = "MathemeToTypst[expr, opts] converts expr to a Typst math String. Supports customization via \"Form\", \"ExpressionRules\", \"BoxRules\", \"TemplateRules\", \"CharacterRules\", \"OperatorNames\", \"DelimiterStyle\", \"FractionStyle\", \"ConstantStyle\", \"DifferentialStyle\", \"TransposeSymbol\", \"ConjugateStyle\", \"BlackboardStyle\", \"ScriptStyle\", \"MultiLetterStyle\", \"MatrixDelimiter\", \"PreserveStyle\", \"StripContext\", \"UnknownCharacterHandler\", and \"UnsupportedBoxHandler\".";
BoxesToTypst::usage = "BoxesToTypst[boxes, opts] converts a Wolfram Language Box expression to a Typst math String.";
TypstRaw::usage = "TypstRaw[str] embeds a literal Typst math fragment verbatim inside expressions converted by TypstForm or MathemeToTypst.";
MakeTypst::usage = "MakeTypst[expr] (or MakeTypst[expr, conv]) is an extensible UpValue formatting hook checked by TypstForm and MathemeToTypst before boxing expr.";
MathemeVersion::usage = "MathemeVersion[] returns version metadata for Matheme, Typst, LibraryLink, and WSTP.";
MathemeSettings::usage = "MathemeSettings[] returns the active compiler settings.";
MathemeSetSettings::usage = "MathemeSetSettings[patch] updates settings (Null resets a field) and returns <|\"Changed\" -> ..., \"Settings\" -> ...|>.";
MathemeResetSettings::usage = "MathemeResetSettings[] restores user settings to defaults and returns <|\"Changed\" -> ..., \"Settings\" -> ...|>.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans configured font sources and returns face counts.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] rebuilds the package loader and clears the cached Universe index.";
MathemeWarmup::usage = "MathemeWarmup[opts] eagerly initializes \"Fonts\", \"Packages\", and \"PackageIndex\" caches.";
MathemeQueryFonts::usage = "MathemeQueryFonts[filters] queries cached font metadata without loading font files.";
MathemeQueryPackages::usage = "MathemeQueryPackages[filters] queries cached local and Universe package metadata.";
ClearMathemeCache::usage = "ClearMathemeCache[targets] drops \"Fonts\", \"Packages\", \"Memoization\", or All caches without unloading the library.";
MathemeClose::usage = "MathemeClose[] clears all caches, closes the WSTP link, and unloads the DLL.";

MathemeLibraryError::usage = "MathemeLibraryError is the Failure tag for library load or protocol errors.";
MathemeCallError::usage = "MathemeCallError is the Failure tag for invalid arguments, settings, or network errors.";
MathemeCompileError::usage = "MathemeCompileError is the Failure tag for Typst compilation and evaluation errors.";
MathemeDiagnostic::usage = "MathemeDiagnostic::warning is issued for non-fatal diagnostics.";
TypstForm::unspt = "TypstForm of `1` is not supported.";
TypstForm::unkchar = "Unmapped Wolfram private-use character `1` (`2`) encountered in TypstForm; falling back to text literal.";

$TypstTemplates::usage = "$TypstTemplates is a protected Association of Typst source templates (pass a key to \"TypstTemplate\").";

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
Protect[$TypstTemplates];

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
  "setSettings" -> "MathemeSetSettings",
  "resetSettings" -> "MathemeResetSettings",
  "warmup" -> "MathemeWarmup",
  "reloadFonts" -> "MathemeReloadFonts",
  "reloadPackages" -> "MathemeReloadPackages",
  "queryFonts" -> "MathemeQueryFonts",
  "queryPackages" -> "MathemeQueryPackages",
  "clearCache" -> "ClearMathemeCache",
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
   the DLL open. One exported function means one link to manage; load lazily
   so MathemeClose can release it and later calls reload the library. *)
$call = Null;
$links = {};

loadFunctions[] := Module[{before, fn},
  If[Head[$call] === LibraryFunction,
    True,
    before = Links[];
    fn = LibraryFunctionLoad[$library, "matheme_call", LinkObject, LinkObject];
    If[Head[fn] === LibraryFunction,
      $call = fn;
      $links = Complement[Links[], before];
      True,
      False
    ]
  ]
];

raiseCallError[template_String, params_Association : <||>] := With[{t = MathemeCallError},
  Raise[t, <|
    "MessageTemplate" -> template,
    "MessageParameters" -> params
  |>]
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

(* Mathematica's FrontEnd uses narrow math glyphs for Unicode box-drawing
   characters even in monospace Code cells, misaligning snippet borders and
   caret markers. Normalize diagnostic borders to ASCII and wrap non-empty
   reports in a Code-styled Cell so Notebook alignment stays exact. *)
normalizeDiagnosticBorders[text_String] := Module[{lines},
  lines = StringSplit[text, "\n", All];
  lines = Map[
    Function[line,
      With[{step = StringReplace[line, {
        StartOfString ~~ ws : WhitespaceCharacter ... ~~ "\:250c\:2500" :> ws <> "-->",
        StartOfString ~~ ws : (WhitespaceCharacter ... ~~ (DigitCharacter ... ~~ WhitespaceCharacter ...)) ~~ "\:2502" :> ws <> "|",
        StartOfString ~~ ws : WhitespaceCharacter ... ~~ "\:00b7" :> ws <> "."
      }]},
        If[StringMatchQ[step, WhitespaceCharacter ... ~~ "|" ~~ ("\:256d" | "\:2570" | "\:2500" | "^" | "-" | "'" | WhitespaceCharacter) ...],
          StringReplace[step, {"\:256d" -> "/", "\:2570" -> "\\", "\:2500" -> "-"}],
          step
        ]
      ]
    ],
    lines
  ];
  StringTrim[StringRiffle[lines, "\n"]]
];

formatDiagnosticCell[""] := "";
formatDiagnosticCell[text_String] := Module[{formatted = normalizeDiagnosticBorders[text]},
  If[formatted === "",
    "",
    DisplayForm[Cell[formatted, "Code",
      CellFrame -> 0.5,
      CellFrameColor -> GrayLevel[0.5, 0.25],
      CellFrameMargins -> {{10, 10}, {8, 8}}
    ]]
  ]
];
formatDiagnosticCell[other_] := other;

failureDetails[name_, result_] := With[{formatted = Lookup[result, "Formatted", ""]},
  <|
    "MessageTemplate" -> "Matheme `Operation` failed: `Detail`",
    "MessageParameters" -> <|
      "Operation" -> Lookup[$displayNames, name, name],
      "Detail" -> Lookup[result, "Message", "unknown error"]
    |>,
    "Message" -> Lookup[result, "Message", "unknown error"],
    "Operation" -> Lookup[$displayNames, name, name],
    "ErrorTag" -> Lookup[result, "ErrorTag", "Call"],
    "Diagnostics" -> Lookup[result, "Diagnostics", {}],
    "Formatted" -> formatted,
    "FormattedCell" -> formatDiagnosticCell[formatted]
  |>
];

callUnchecked[name_String, arguments_Association] := Module[{result, tag, dropped},
  If[!loadFunctions[],
    With[{t = MathemeLibraryError},
      Raise[t, <|
        "MessageTemplate" -> "Could not load the matheme library: `Library`.",
        "MessageParameters" -> <|"Library" -> $library|>
      |>]]
  ];
  result = $call[<|"Action" -> name, "Arguments" -> arguments|>];
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
  dropped = KeyDrop[result, "Status"];
  If[KeyExistsQ[dropped, "Formatted"],
    Append[dropped, "FormattedCell" -> formatDiagnosticCell[dropped["Formatted"]]],
    dropped
  ]
];

(* Every public call goes through the same pipeline: success returns the
   association without its internal Status field, failure returns a Failure. *)
callFunction[name_String] := callFunction[name, <||>];
callFunction[name_String, arguments_Association] :=
  Handle[callUnchecked[name, arguments], _Failure];

(* ------------------------------------------------------------------------ *)
(* Templates                                                                *)
(* ------------------------------------------------------------------------ *)

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
  "\"TypstTemplate\" must be None, a key of $TypstTemplates, or a template; got `Type`.",
  <|"Type" -> Head[other]|>
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
Options[MathemeToGraphics] = Normal[Join[
  Association[Options[MathemeCompile]],
  <|"Format" -> "Graphics"|>
]];
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
  Handle[compileChecked[source, opts], _Failure];
MathemeCompile[TypstForm[expr_, tOpts : OptionsPattern[]], opts : OptionsPattern[]] :=
  Module[{src = Matheme`Private`TypstForm`typstFormToString[expr, tOpts], flatOpts = Flatten[{opts}], merged},
    merged = If[KeyExistsQ[Association[flatOpts], "TypstTemplate"],
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

toGraphics[result_] := Module[{pdf, svg, graphics},
  If[FailureQ[result] || !AssociationQ[result],
    result,
    graphics = Lookup[result, "Graphics", Null];
    pdf = Lookup[result, "PDF", Null];
    svg = Lookup[result, "SVG", Null];
    Which[
      ListQ[graphics] && AllTrue[graphics, Head[#] === Graphics &],
        result,
      ListQ[svg],
        graphics = Map[ImportString[normalizeSvgUnits[#], {"SVG", "Graphics"}] &, svg];
        If[!AllTrue[graphics, Head[#] === Graphics &],
          raiseCallError["Failed to import SVG output as Graphics."],
          Join[result, <|"Graphics" -> Map[uncropGraphics, graphics]|>]
        ],
      Head[pdf] === ByteArray,
        graphics = ImportByteArray[pdf, {"PDF", "PageGraphics"}];
        If[!ListQ[graphics],
          raiseCallError["Failed to import PDF bytes as Graphics."],
          Join[result, <|"Graphics" -> Map[uncropGraphics, graphics]|>]
        ],
      True,
        raiseCallError["MathemeToGraphics received an unsupported compilation result format."]
    ]
  ]
];

MathemeToGraphics[source : (_String | _TypstForm), opts : OptionsPattern[]] :=
  Handle[
    Module[{flatOpts = Flatten[{opts}], optAssoc, fmt, merged},
      optAssoc = Association[flatOpts];
      fmt = Lookup[optAssoc, "Format", OptionValue[MathemeToGraphics, "Format"]];
      validateGraphicsFormat[fmt];
      merged = If[KeyExistsQ[optAssoc, "Format"],
        flatOpts,
        Append[flatOpts, "Format" -> fmt]
      ];
      toGraphics[MathemeCompile[source, Sequence @@ merged]]
    ],
    _Failure
  ];

(* ------------------------------------------------------------------------ *)
(* State, queries, and cache lifecycle                                      *)
(* ------------------------------------------------------------------------ *)

callWithRules[action_String, _String, arg_Association] :=
  callFunction[action, arg];
callWithRules[action_String, _String, opts : ((_Rule | _RuleDelayed) .. | {(_Rule | _RuleDelayed) ...})] :=
  callFunction[action, Association[opts]];
callWithRules[_String, displayName_String, other_] := Handle[
  raiseCallError[
    "`Name` expects an Association or rules; got `Type`.",
    <|"Name" -> displayName, "Type" -> Head[other]|>
  ],
  _Failure
];

MathemeVersion[] := callFunction["version"];

MathemeSettings[] := callFunction["settings"];
MathemeSetSettings[arg__] := callWithRules["setSettings", "MathemeSetSettings", arg];
MathemeResetSettings[] := callFunction["resetSettings"];

MathemeReloadFonts[] := callFunction["reloadFonts"];
MathemeReloadPackages[] := callFunction["reloadPackages"];

ClearMathemeCache[] := callFunction["clearCache", <|"Targets" -> All|>];
ClearMathemeCache[All | "All"] := callFunction["clearCache", <|"Targets" -> All|>];
ClearMathemeCache[target_String] := callFunction["clearCache", <|"Targets" -> {target}|>];
ClearMathemeCache[targets : {___String}] := callFunction["clearCache", <|"Targets" -> targets|>];
ClearMathemeCache[other_] := Handle[
  raiseCallError[
    "ClearMathemeCache expects \"Fonts\", \"Packages\", \"Memoization\", All, or a list of targets; got `Type`.",
    <|"Type" -> Head[other]|>
  ],
  _Failure
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

MathemeClose[] := Module[
  {before = Links[], unloaded = 0, links = $links, released = <||>,
   dllUnloaded = False},
  If[Head[$call] === LibraryFunction,
    released = callFunction["close"];
    If[!AssociationQ[released], released = <||>];
    (* LibraryFunctionUnload already drops the last module reference on
       Wolfram 15, so a later LibraryUnload would report "cannot be unloaded"
       even though the DLL was released. Pin one extra reference here (after
       the close call so an Abort[] during close cannot leak it) so the final
       LibraryUnload has a real mapping to release and returns Null. *)
    CheckAbort[
      LibraryLoad[$library];
      Quiet[LibraryFunctionUnload[$call]];
      unloaded = 1;
      (* LibraryFunctionUnload normally closes the link already; the explicit
         close keeps the frontend's own LinkObject reference from lingering. *)
      Scan[Quiet[LinkClose[#]] &, links];
      $call = Null;
      $links = {};
      dllUnloaded = TrueQ[Quiet@LibraryUnload[$library] =!= $Failed],
      Quiet[LibraryUnload[$library]];
      Abort[]
    ];
  ];
  Join[released, <|
    "Unloaded" -> unloaded,
    "LinksClosed" -> Length[Complement[before, Links[]]],
    "LibraryUnloaded" -> dllUnloaded
  |>]
];

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
SyntaxInformation[MathemeSetSettings] = {"ArgumentsPattern" -> {___}};
Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {}}],
  {MathemeVersion, MathemeSettings, MathemeResetSettings, MathemeReloadFonts,
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
   MathemeVersion, MathemeSettings,
   MathemeSetSettings, MathemeResetSettings, MathemeReloadFonts,
   MathemeReloadPackages, MathemeWarmup, MathemeQueryFonts,
   MathemeQueryPackages, ClearMathemeCache, MathemeClose}
];
Scan[Protect, {MathemeLibraryError, MathemeCallError, MathemeCompileError,
  MathemeDiagnostic}];

EndPackage[];
