(* ::Package:: *)

(* Core WSTP bridge, library lifecycle, error handling, and diagnostics.
   Loaded inside Runge`Matheme`Private` by Kernel/Matheme.wl. *)

(* Register the four failure tags once, when the package is loaded. *)
Scan[CreateErrorType[#, {}] &,
  {MathemeLibraryError, MathemeCallError, MathemeCompileError, MathemeInternalError}];

$errorTags = <|
  "Library" -> MathemeLibraryError,
  "Call" -> MathemeCallError,
  "Compile" -> MathemeCompileError,
  "Internal" -> MathemeInternalError
|>;

$displayNames = <|
  "compile" -> "MathemeCompile",
  "version" -> "MathemeVersion",
  "settings" -> "SetMatheme",
  "setSettings" -> "SetMatheme",
  "resetSettings" -> "ResetMatheme",
  "warmup" -> "MathemeWarmup",
  "reloadFonts" -> "MathemeReloadFonts",
  "reloadPackages" -> "MathemeReloadPackages",
  "queryFonts" -> "MathemeQueryFonts",
  "queryPackages" -> "MathemeQueryPackages",
  "clearCache" -> "ClearMathemeCache",
  "close" -> "MathemeClose"
|>;

MathemeDiagnostic::warning = "Matheme `1`: `2`";

$libraryFileName = Switch[$OperatingSystem,
  "Windows", "matheme.dll",
  "MacOSX", "matheme.dylib",
  _, "matheme.so"
];

$library = Module[{found = Quiet[FindLibrary["matheme"]]},
  If[TrueQ[found =!= $Failed],
    found,
    FileNameJoin[{DirectoryName[$InputFileName, 2], "LibraryResources",
      $SystemID, $libraryFileName}]
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

(* Warnings are issued as Messages; hints stay in Diagnostics / FormattedCell,
   and errors are carried by the Failure object. *)
issueDiagnostics[name_, diagnostics_] := Scan[
  Function[diag,
    If[AssociationQ[diag] && Lookup[diag, "Severity", None] === "Warning",
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

failureDetails[name_, result_] := Module[{report = Lookup[result, "DiagnosticReport", None], base},
  base = <|
    "MessageTemplate" -> "Matheme `Operation` failed: `Detail`",
    "MessageParameters" -> <|
      "Operation" -> Lookup[$displayNames, name, name],
      "Detail" -> Lookup[result, "Message", "unknown error"]
    |>,
    "Message" -> Lookup[result, "Message", "unknown error"],
    "Operation" -> Lookup[$displayNames, name, name],
    "ErrorKind" -> Lookup[result, "ErrorKind", "Library"],
    "Diagnostics" -> Lookup[result, "Diagnostics", {}]
  |>;
  If[StringQ[report] && StringTrim[report] =!= "",
    Append[base, "FormattedCell" -> formatDiagnosticCell[report]],
    base
  ]
];

callUnchecked[name_String, arguments_Association] := Module[{result, tag, report, dropped},
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
    tag = Lookup[$errorTags, Lookup[result, "ErrorKind", "Library"], MathemeLibraryError];
    With[{t = tag, details = failureDetails[name, result]}, Raise[t, details]]
  ];
  report = Lookup[result, "DiagnosticReport", None];
  dropped = KeyDrop[result, {"Status", "DiagnosticReport"}];
  If[StringQ[report] && StringTrim[report] =!= "",
    Append[dropped, "FormattedCell" -> formatDiagnosticCell[report]],
    dropped
  ]
];

(* Every public call goes through the same pipeline: success returns the
   association without its internal Status field, failure returns a Failure. *)
callFunction[name_String] := callFunction[name, <||>];
callFunction[name_String, arguments_Association] :=
  Handle[callUnchecked[name, arguments], _Failure];

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
