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
  "boxesToTypst" -> "BoxesToTypst",
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

$library = With[{found = Quiet[FindLibrary["matheme"]]},
  If[TrueQ[found =!= $Failed],
    found,
    FileNameJoin[{DirectoryName[$InputFileName, 2], "LibraryResources",
      $SystemID, $libraryFileName}]
  ]
];

(* One exported LinkObject LibraryFunction handles all actions. Load lazily so
   MathemeClose can unload the shared library and later calls reload it. *)
$call = Null;

loadFunctions[] := Module[{fn},
  If[Head[$call] === LibraryFunction,
    True,
    fn = LibraryFunctionLoad[$library, "matheme_call", LinkObject, LinkObject];
    If[Head[fn] === LibraryFunction,
      $call = fn;
      (* The native TypstForm path latches link failures; a successful
         (re)load resumes it. *)
      TypstForm`$typstNativeLinkFailed = False;
      True,
      False
    ]
  ]
];

makeCallFailure[template_String, params_Association : <||>] :=
  CreateFailure[MathemeCallError, <|
    "MessageTemplate" -> template,
    "MessageParameters" -> params,
    "ErrorKind" -> "Call"
  |>];

raiseCallError[template_String, params_Association : <||>] := With[{t = MathemeCallError},
  Raise[t, <|
    "MessageTemplate" -> template,
    "MessageParameters" -> params,
    "ErrorKind" -> "Call"
  |>]
];

optionName[name_String] := name;
optionName[name_Symbol] := SymbolName[Unevaluated[name]];
optionName[_] := None;

enumOption[names_List] := _String ? (StringMatchQ[#, Alternatives @@ names, IgnoreCase -> True] &);

checkOptionRules[raw_List, allowed_List] := Module[{rules = Flatten[raw], names, unknown},
  If[!AllTrue[rules, MatchQ[#, _Rule | _RuleDelayed] &],
    raiseCallError["Expected option rules."]
  ];
  names = optionName /@ (First /@ rules);
  unknown = Complement[names, allowed];
  If[unknown =!= {},
    raiseCallError["Unknown option(s) `Names`.", <|"Names" -> unknown|>]
  ];
  {rules, names}
];

checkArgumentTypes[action_String, args_Association] := With[{types = $argumentTypes[action]},
  KeyValueMap[
    Function[{name, value}, With[{pattern = types[name]},
      If[!MatchQ[value, pattern],
        raiseCallError["Invalid type for option `Name`. Expected `Type`.",
          <|"Name" -> name, "Type" -> ToString[pattern, InputForm]|>]
      ]
    ]],
    args
  ]
];

diagnosticText[diag_Association] := With[
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
formatDiagnosticCell[text_String] := With[{formatted = normalizeDiagnosticBorders[text]},
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
    (* A transport failure can leave the cached LibraryFunction unusable. *)
    If[Head[$call] === LibraryFunction, Quiet[LibraryFunctionUnload[$call]]; $call = Null];
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
  Handle[_Failure] @ callUnchecked[name, arguments];

checkedArgumentRules[action_String, raw_List] :=
  Handle[_Failure] @ Module[{rules, names, args},
    {rules, names} = checkOptionRules[raw, Keys[$argumentTypes[action]]];
    names = DeleteDuplicates[names];
    args = If[names === {}, <||>, AssociationThread[names, OptionValue[rules, names]]];
    checkArgumentTypes[action, args];
    callUnchecked[action, args]
  ];

MathemeClose[] := (
  If[Head[$call] === LibraryFunction,
    Quiet[callFunction["close"]];
    Quiet[LibraryFunctionUnload[$call]];
    $call = Null
  ];
  (* Clear the latched native TypstForm link failure so the next load attempt
     can resume the fast path after the library is rebuilt or reinstalled. *)
  TypstForm`$typstNativeLinkFailed = False
);
