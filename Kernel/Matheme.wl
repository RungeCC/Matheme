(* The failure representation follows the convention in
   Wolfram/ErrorTools: private code raises Failure objects, public functions
   catch them and return them normally. *)

(* A reload must be able to replace definitions protected by the previous
   load, so drop the public symbols first. *)
If[MemberQ[$Packages, "Matheme`"],
  Unprotect["Matheme`*"];
  ClearAll["Matheme`*"]
];

BeginPackage["Matheme`", {"Wolfram`ErrorTools`"}];

MathemeCompile::usage = "MathemeCompile[source, opts] compiles Typst source to PDF and returns an Association with PDF, Pages, Diagnostics and Formatted (a typst-cli style report). On failure it returns a Failure tagged MathemeCompileError carrying structured diagnostics. Warnings are issued as messages. Options: \"Inputs\" (default <||>), \"BasePath\" (default: current directory), \"AllowDownloads\" (default: False), \"TypstTemplate\" (default: None; a key of $TypstTemplates or a template).";
MathemeToGraphics::usage = "MathemeToGraphics[source, opts] compiles Typst source and imports the PDF pages as vector Graphics. Accepts the same options as MathemeCompile and returns failures unchanged.";
MathemeVersion::usage = "MathemeVersion[] returns <|\"Matheme\" -> version, \"Typst\" -> version, \"TypstCommit\" -> commit|>. Configuration is reported by MathemeSettings[].";
MathemeSettings::usage = "MathemeSettings[] returns the effective compiler settings: FontPaths, SystemFonts, EmbeddedFonts, PackagePath, PackageCache, Proxy, Downloads and HostFontPaths. This call is read-only; use MathemeSetSettings or MathemeResetSettings to change state.";
MathemeSetSettings::usage = "MathemeSetSettings[association] applies a settings patch and returns <|\"Changed\" -> ..., \"Settings\" -> ...|>. Omitted keys are left untouched; a key with value Null resets that field.";
MathemeResetSettings::usage = "MathemeResetSettings[] restores every user-facing setting to its default (host-provided font paths survive) and returns the same shape as MathemeSetSettings.";
MathemeReloadFonts::usage = "MathemeReloadFonts[] rescans all configured font sources and returns the number of faces found.";
MathemeReloadPackages::usage = "MathemeReloadPackages[] rebuilds the package loader (dropping the cached Universe index) and returns package information.";
MathemeWarmup::usage = "MathemeWarmup[] eagerly initializes the font cache, package loader and Universe index. Options: \"Fonts\", \"Packages\", \"PackageIndex\" (all True by default). Returns per-stage timings; PackageIndex needs network access.";
MathemeQueryFonts::usage = "MathemeQueryFonts[] and MathemeQueryFonts[filters] return the cached font-book metadata (family, variant, flags, source, path, collection index and residency). Filters: Family, Source, Resident, Character; paging: Offset, MaxResults (default 50, All for everything). Queries never load a font file.";
MathemeQueryPackages::usage = "MathemeQueryPackages[] and MathemeQueryPackages[filters] return the cached package metadata (locally installed packages plus the Universe index once fetched). Filters: Name, Namespace, Origin, Version; paging: Offset, MaxResults; opt-ins: Refresh (rescan local directories), AllowDownloads (fetch the Universe index).";
MathemeClose::usage = "MathemeClose[] drops every library cache (fonts, packages, memoization), closes the WSTP LinkObject, unloads the library function and releases the DLL image. The library is reloaded automatically on the next call.";

MathemeLibraryError::usage = "MathemeLibraryError is the Failure tag used when the WSTP library cannot be loaded or returns an unexpected result.";
MathemeCallError::usage = "MathemeCallError is the Failure tag used when a library call reports a failure (invalid arguments, bad configuration, network errors).";
MathemeCompileError::usage = "MathemeCompileError is the Failure tag used when Typst compilation fails. The Failure carries Diagnostics and Formatted.";
MathemeDiagnostic::usage = "MathemeDiagnostic::warning is the message issued for every non-fatal diagnostic; use Off[MathemeDiagnostic::warning] to silence warnings.";

$TypstTemplates::usage = "$TypstTemplates is a protected Association of Typst source templates. Pass one of its keys as \"TypstTemplate\" -> key to MathemeCompile, or supply a Function / StringTemplate / TemplateObject directly. Templates are applied to the source string before compilation; MiTeX templates need an explicit \"AllowDownloads\" -> True until the package is cached.";

$TypstTemplates = <|
"Equation" -> StringTemplate["#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$ `` $"],
"InlineEquation" -> StringTemplate["#set page(width: auto, height: auto, margin: 0pt, fill: none)\n$``$"],
"Text" -> StringTemplate["#set page(width: auto, height: auto, margin: 0pt, fill: none)\n``"],
"MiTeXEquation" -> (
TemplateObject[{
"#set page(width: auto, height: auto, margin: 0pt, fill: none)\n",
"#import \"@preview/mitex:0.2.4\": mitex\n",
"#mitex(`", TemplateSlot[1], "`)"},
CombinerFunction -> StringJoin,
InsertionFunction -> TextString,
MetaInformation -> <| |>]
),
"MiTeXInlineEquation" -> (
TemplateObject[{
"#set page(width: auto, height: auto, margin: 0pt, fill: none)\n",
"#import \"@preview/mitex:0.2.4\": mi\n",
"#mi(`", TemplateSlot[1], "`)"},
CombinerFunction -> StringJoin,
InsertionFunction -> TextString,
MetaInformation -> <| |>]
),
"MiTeXText" -> (
TemplateObject[{
"#set page(width: auto, height: auto, margin: 0pt, fill: none)\n",
"#import \"@preview/mitex:0.2.4\": mitext\n",
"#mitext(`", TemplateSlot[1], "`)"},
CombinerFunction -> StringJoin,
InsertionFunction -> TextString,
MetaInformation -> <| |>]
)
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
  If[Head[$call] === LibraryFunction, Return[True]];
  before = Links[];
  fn = LibraryFunctionLoad[$library, "matheme_call", LinkObject, LinkObject];
  If[Head[fn] =!= LibraryFunction, Return[False]];
  $call = fn;
  $links = Complement[Links[], before];
  True
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

callUnchecked[name_String, arguments_Association] := Module[{result, tag},
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
  KeyDrop[result, "Status"]
];

(* Every public call goes through the same pipeline: success returns the
   association without its internal Status field, failure returns a Failure. *)
callFunction[name_String] := callFunction[name, <||>];
callFunction[name_String, arguments_Association] :=
  Handle[callUnchecked[name, arguments], _Failure];

(* ------------------------------------------------------------------------ *)
(* Templates                                                                 *)
(* ------------------------------------------------------------------------ *)

raiseUnknownTemplate[name_] := With[{t = MathemeCallError},
  Raise[t, <|
    "MessageTemplate" -> "Unknown \"TypstTemplate\" `Name`. Available keys: `Keys`.",
    "MessageParameters" -> <|
      "Name" -> name,
      "Keys" -> StringRiffle[Keys[$TypstTemplates], ", "]
    |>
  |>]
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
applyTemplate[other_, source_String] := With[{t = MathemeCallError},
  Raise[t, <|
    "MessageTemplate" -> "\"TypstTemplate\" must be None, a key of $TypstTemplates, or a template; got `Type`.",
    "MessageParameters" -> <|"Type" -> Head[other]|>
  |>]
];

(* ------------------------------------------------------------------------ *)
(* Compilation                                                               *)
(* ------------------------------------------------------------------------ *)

compileWith[source_, inputs_, base_, allowDownloads_] := callFunction["compile", <|
  "Source" -> source,
  "Inputs" -> inputs,
  "BasePath" -> base,
  "AllowDownloads" -> allowDownloads
|>];

Options[MathemeCompile] = {
  "Inputs" -> <||>,
  "BasePath" -> Automatic,
  "AllowDownloads" -> False,
  "TypstTemplate" -> None
};
Options[MathemeToGraphics] = Options[MathemeCompile];
Options[compileChecked] = Options[MathemeCompile];

compileChecked[source_String, opts : OptionsPattern[]] := compileWith[
  applyTemplate[OptionValue["TypstTemplate"], source],
  OptionValue["Inputs"],
  If[OptionValue["BasePath"] === Automatic, Directory[], OptionValue["BasePath"]],
  TrueQ[OptionValue["AllowDownloads"]]
];

(* Template validation also raises through ErrorTools, so the public entry
   must handle failures that happen before the library is ever called. *)
MathemeCompile[source_String, opts : OptionsPattern[]] :=
  Handle[compileChecked[source, opts], _Failure];

toGraphics[result_] := Module[{pdf},
  If[FailureQ[result] || !AssociationQ[result], Return[result]];
  pdf = Lookup[result, "PDF", Null];
  If[Head[pdf] =!= ByteArray, Return[Join[result, <|"Graphics" -> {}|>]]];
  Join[result, <|"Graphics" -> ImportByteArray[pdf, {"PDF", "PageGraphics"}]|>]
];

MathemeToGraphics[source_String, opts : OptionsPattern[]] :=
  toGraphics[MathemeCompile[source, opts]];

(* ------------------------------------------------------------------------ *)
(* State and queries                                                         *)
(* ------------------------------------------------------------------------ *)

MathemeVersion[] := callFunction["version"];

MathemeSettings[] := callFunction["settings"];
MathemeSetSettings[settings_Association] := callFunction["setSettings", settings];
MathemeResetSettings[] := callFunction["resetSettings"];

MathemeReloadFonts[] := callFunction["reloadFonts"];
MathemeReloadPackages[] := callFunction["reloadPackages"];

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
  If[Head[$call] === LibraryFunction,
    (* LibraryFunctionUnload already drops the last module reference on
       Wolfram 15, so a later LibraryUnload would report "cannot be unloaded"
       even though the DLL was released. Pin one extra reference here so the
       final LibraryUnload has a real mapping to release and returns Null. *)
    LibraryLoad[$library];
    released = callFunction["close"];
    If[!AssociationQ[released], released = <||>];
    Quiet[LibraryFunctionUnload[$call]];
    unloaded = 1;
    (* LibraryFunctionUnload normally closes the link already; the explicit
       close keeps the frontend's own LinkObject reference from lingering. *)
    Scan[Quiet[LinkClose[#]] &, links];
    $call = Null;
    $links = {};
    dllUnloaded = TrueQ[Quiet@LibraryUnload[$library] =!= $Failed];
  ];
  Join[released, <|
    "Unloaded" -> unloaded,
    "LinksClosed" -> Length[Complement[before, Links[]]],
    "LibraryUnloaded" -> dllUnloaded
  |>]
];

(* ------------------------------------------------------------------------ *)
(* Package conventions                                                       *)
(* ------------------------------------------------------------------------ *)

Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {_, OptionsPattern[]}}],
  {MathemeCompile, MathemeToGraphics}
];
SyntaxInformation[MathemeWarmup] = {"ArgumentsPattern" -> {OptionsPattern[]}};
SyntaxInformation[MathemeSetSettings] = {"ArgumentsPattern" -> {_Association}};
Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {}}],
  {MathemeVersion, MathemeSettings, MathemeResetSettings, MathemeReloadFonts,
   MathemeReloadPackages, MathemeClose}
];
Scan[
  Function[f, SyntaxInformation[f] = {"ArgumentsPattern" -> {___}}],
  {MathemeQueryFonts, MathemeQueryPackages}
];

End[];

(* Public functions and the public failure/message symbols are read-only. *)
Scan[
  SetAttributes[#, {Protected, ReadProtected}] &,
  {MathemeCompile, MathemeToGraphics, MathemeVersion, MathemeSettings,
   MathemeSetSettings, MathemeResetSettings, MathemeReloadFonts,
   MathemeReloadPackages, MathemeWarmup, MathemeQueryFonts,
   MathemeQueryPackages, MathemeClose}
];
Scan[Protect, {MathemeLibraryError, MathemeCallError, MathemeCompileError,
  MathemeDiagnostic}];

EndPackage[];
