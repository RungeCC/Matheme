(* ::Package:: *)

(* TypstForm.wl: Public entry points and expression preprocessing for TypstForm.
   Loads modular subsystems from Kernel/TypstForm/. *)

With[{dir = FileNameJoin[{DirectoryName[$InputFileName], "TypstForm"}]},
  Get[FileNameJoin[{dir, "Config.wl"}]];
  Get[FileNameJoin[{dir, "Characters.wl"}]];
  Get[FileNameJoin[{dir, "Templates.wl"}]];
  Get[FileNameJoin[{dir, "Boxes.wl"}]];
];

Begin["`TypstForm`"];

(* Generated from matheme_box::OPTIONS. Defaults are fail-closed: before the
   generator has run there is no schema, so no patch can be claimed. Keep this
   inside the TypstForm context so the symbols match the functions below. *)
$typstNativeConfigVersion = 0;
$typstNativeOptionSchema = <||>;
Quiet[Check[
  Get[FileNameJoin[{DirectoryName[$InputFileName], "TypstForm", "NativeCapabilities.wl"}]],
  Null
]];

(* ------------------------------------------------------------------------ *)
(* TypstRaw & BoxWrapper integration with MakeBoxes                         *)
(* ------------------------------------------------------------------------ *)

TypstRaw /: MakeBoxes[TypstRaw[s_String], StandardForm | TraditionalForm] :=
  TemplateBox[{s}, "MathemeTypstRaw"];

typstBoxWrapper /: MakeBoxes[typstBoxWrapper[box_], StandardForm | TraditionalForm] :=
  TemplateBox[{box}, "MathemeTypstBox"];

(* ------------------------------------------------------------------------ *)
(* Expression preprocessing (ExpressionRules + MakeTypst UpValues)          *)
(* ------------------------------------------------------------------------ *)

$typstBoxHeads = Alternatives[
  RowBox, FractionBox, SuperscriptBox, SubscriptBox, SubsuperscriptBox,
  OverscriptBox, UnderscriptBox, UnderoverscriptBox, SqrtBox, RadicalBox,
  GridBox, StyleBox, TemplateBox, FrameBox, TagBox, AdjustmentBox,
  InterpretationBox, FormBox, BoxData, TextData, Cell, Notebook
];

wrapPreprocessedResult[res_, rules_, depth_] := Which[
  StringQ[res],
    HoldComplete[TypstRaw[res]],
  MatchQ[res, TypstRaw[_String]],
    HoldComplete[res],
  MatchQ[Head[res], $typstBoxHeads],
    HoldComplete[typstBoxWrapper[res]],
  True,
    preprocessHeld[HoldComplete[res], rules, depth + 1]
];

tryMakeTypst[HoldComplete[e_]] := Module[{res, conv},
  conv = Function[arg, MathemeToTypst[HoldForm[arg]], HoldFirst];
  res = MakeTypst[e, conv];
  If[res === Missing["NotDefined"],
    res = MakeTypst[e]
  ];
  res
];

symbolHasMakeTypstQ[sym_Symbol] := Quiet[UpValues[sym] =!= {}];
symbolHasMakeTypstQ[_] := False;

exprMayHaveMakeTypstQ[HoldComplete[sym_Symbol]] := symbolHasMakeTypstQ[sym];
exprMayHaveMakeTypstQ[HoldComplete[h_Symbol[___]]] := symbolHasMakeTypstQ[h];
exprMayHaveMakeTypstQ[_] := False;

(* Top-down held traversal so HoldForm[...] and unevaluated subexpressions are
   never evaluated prematurely while applying ExpressionRules or MakeTypst. *)
preprocessHeld[h : HoldComplete[_TypstRaw | _typstBoxWrapper], _, _] := h;
preprocessHeld[h_HoldComplete, _, depth_] /; depth > $typstMaxEvalDepth := h;

preprocessHeld[h : HoldComplete[e_], rules_List, depth_] := Module[{matched, mtRes},
  (* 1. User per-call ExpressionRules take highest priority *)
  If[rules =!= {},
    matched = Replace[Unevaluated[e], Append[rules, _ :> $typstNoRuleMatch]];
    If[matched =!= $typstNoRuleMatch,
      Return[wrapPreprocessedResult[matched, rules, depth], Module]
    ]
  ];

  (* 2. User UpValues attached to MakeTypst *)
  If[exprMayHaveMakeTypstQ[h],
    mtRes = tryMakeTypst[h];
    If[mtRes =!= Missing["NotDefined"],
      Return[wrapPreprocessedResult[mtRes, rules, depth], Module]
    ]
  ];

  (* 3. Recurse into compound expressions without evaluating arguments *)
  preprocessChildren[h, rules, depth]
];

(* Recombine HoldComplete[hd] and {HoldComplete[a1], HoldComplete[a2], ...} safely
   into HoldComplete[hd[a1, a2, ...]] without evaluating hd or any argument. *)
combineHeld[HoldComplete[hd_], {}] := HoldComplete[hd[]];
combineHeld[HoldComplete[hd_], argHelds_List] :=
  With[{joined = Join @@ argHelds},
    Replace[joined, HoldComplete[args___] :> HoldComplete[hd[args]]]
  ];

preprocessChildren[h : HoldComplete[e_], rules_List, depth_] :=
  If[AtomQ[Unevaluated[e]],
    h,
    Module[{headHeld, argHelds, newHead, newArgs},
      headHeld = Extract[h, {1, 0}, HoldComplete];
      argHelds = Table[Extract[h, {1, i}, HoldComplete], {i, Length[Unevaluated[e]]}];
      newHead = preprocessHeld[headHeld, rules, depth];
      newArgs = preprocessHeld[#, rules, depth] & /@ argHelds;
      combineHeld[newHead, newArgs]
    ]
  ];

(* Fast check whether an expression contains any symbol with UpValues *)
containsUpValueSymbolQ[h_HoldComplete] :=
  !FreeQ[h, s_Symbol /; Quiet[UpValues[s] =!= {}]];

makeBoxesPreprocessed[expr_, form_, rules_List] := Module[{held},
  held = HoldComplete[expr];
  If[rules =!= {} || containsUpValueSymbolQ[held],
    held = preprocessHeld[held, rules, 0]
  ];
  Replace[held, HoldComplete[e_] :> MakeBoxes[e, form]]
];

(* ------------------------------------------------------------------------ *)
(* Native Box DSL fast path (R1: four planes, fail-closed diagnostics)       *)
(* ------------------------------------------------------------------------ *)

nativeStateQ[] :=
  $typstInMatrixCell === False && $typstInCasesCell === False &&
    $typstInQuantityUnit === False && $typstInDelimiterDepth === 0;

(* Keep only entries whose value differs from the WL default. The patch is
   computed from the *effective* config, so SetOptions and nested inheritance
   need no special case and a misspelled key is caught explicitly. *)
nativeMapDiff[built_Association, default_Association] := Association[
  Select[Normal[built],
    !KeyExistsQ[default, First[#]] || Lookup[default, First[#]] =!= Last[#] &
  ]
];

(* Every decision below is driven by the generated option schema: plane,
   wire kind, claim status and switch key are declared once in Rust. *)
nativeConfigPatch[cfg_Association] := Module[
  {defaults, known, bad, spec, value, name, templateMap, charMap, opMap, switches = <||>},
  defaults = Association[$typstBoxOptionDefaults];

  If[$typstNativeOptionSchema === <||>,
    Return[Failure["Config", <|
      "Field" -> "Config", "Reason" -> "native option schema is unavailable"
    |>]]
  ];

  known = Keys[$typstNativeOptionSchema];
  bad = Complement[Keys[cfg], known];
  If[bad =!= {},
    Return[Failure["Config", <|
      "Field" -> First[bad], "Reason" -> "unknown native config key"
    |>]]
  ];

  templateMap = normalizeRuleAssoc[Lookup[cfg, "TemplateRules", <||>]];
  (* A String or a TypstTemplate is data; Wolfram StringTemplate/TemplateObject
     and functions need evaluation, so they stay on the reference path. *)
  If[AnyTrue[Values[templateMap], !(StringQ[#] || MatchQ[#, _TypstTemplate]) &],
    Return[Failure["Config", <|
      "Field" -> "TemplateRules", "Reason" -> "template handler needs WL evaluation"
    |>]]
  ];

  switches = Catch[
    Do[
      spec = Lookup[$typstNativeOptionSchema, name];
      value = Lookup[cfg, name, Lookup[defaults, name]];
      Which[
        Lookup[spec, "Plane"] === "Code",
          If[value =!= Lookup[defaults, name],
            Throw[Failure["Config", <|
              "Field" -> name, "Reason" -> name <> " needs WL evaluation"
            |>], "nativePatchFailure"]
          ],
        Lookup[spec, "Kind"] === "Switch",
          If[value =!= Lookup[defaults, name],
            If[!TrueQ[Lookup[spec, "Claimed"]],
              Throw[Failure["Config", <|
                "Field" -> name, "Reason" -> "switch is not claimed by the native renderer"
              |>], "nativePatchFailure"]
            ];
            switches = Append[switches, Lookup[spec, "Key"] -> value]
          ],
        True, Null
      ],
      {name, known}
    ];
    switches,
    "nativePatchFailure"
  ];
  If[Head[switches] === Failure, Return[switches]];

  (* withTypstConfig has already built the effective maps for the WL engine;
     rebuilding them here would tax every conversion twice. Direct callers
     outside withTypstConfig (tests, the parity suite) build them from cfg. *)
  charMap = If[AssociationQ[$typstCharMap], $typstCharMap, buildTypstCharMap[cfg]];
  opMap = If[AssociationQ[$typstOperatorMap], $typstOperatorMap, buildTypstOperatorMap[cfg]];

  (* buildTypstCharMap returns the default map itself when nothing overrides
     it, so SameQ makes the default configuration free; only a customized map
     pays for the diff against the ~800-entry default. *)
  charOverrides =
    If[SameQ[charMap, $typstDefaultCharMap],
      <||>,
      nativeMapDiff[charMap, $typstDefaultCharMap]
    ];
  operatorOverrides =
    If[SameQ[opMap, $typstDefaultOperatorMap],
      <||>,
      nativeMapDiff[opMap, $typstDefaultOperatorMap]
    ];

  <|
    "Version" -> $typstNativeConfigVersion,
    "CharOverrides" -> charOverrides,
    "OperatorOverrides" -> operatorOverrides,
    "TemplateOverrides" -> templateMap,
    "Switches" -> switches
  |>
];

nativeFallback[boxes_, stage_String, field_String, reason_String] := Module[{},
  If[TrueQ[$typstNativeDiagnostics],
    Message[TypstForm::nativefallback, stage, field, reason]
  ];
  convertBox[boxes]
];

convertBoxNative[boxes_] := Module[{patch, detail, res, stage, field, reason},
  If[$typstNativeLinkFailed,
    Return[nativeFallback[boxes, "State", "nativeLinkFailed",
      "the native link failed earlier; MathemeClose[] clears this state"]]
  ];
  If[!nativeStateQ[],
    Return[nativeFallback[boxes, "State", "nativeStateQ",
      "conversion state is not the default"]]
  ];
  patch = If[KeyExistsQ[$typstNativePatchCache, $typstActiveConfig],
    $typstNativePatchCache[$typstActiveConfig],
    Module[{fresh = nativeConfigPatch[$typstActiveConfig]},
      $typstNativePatchCache = <|$typstActiveConfig -> fresh|>;
      fresh
    ]  ];
  If[Head[patch] === Failure,
    detail = patch[[2]];
    Return[nativeFallback[boxes, "Config", Lookup[detail, "Field", "Config"],
      Lookup[detail, "Reason", "config cannot be claimed"]]]
  ];
  res = Quiet[Check[
    Runge`Matheme`Private`callFunction[
      "boxesToTypst", <|"Boxes" -> boxes, "Config" -> patch|>],
    $Failed
  ]];
  If[AssociationQ[res] && TrueQ[Lookup[res, "Supported", False]] &&
      StringQ[Lookup[res, "Result", Null]],
    Return[Lookup[res, "Result"]]
  ];
  stage = If[AssociationQ[res], Lookup[res, "Stage", "Render"], "Call"];
  field = If[AssociationQ[res], Lookup[res, "Field", "Render"], "Call"];
  reason = If[AssociationQ[res], Lookup[res, "Reason", "native conversion failed"],
    "native call failed"];
  (* A Call-stage failure means the library or the transport, not this tree:
     latch it so the remaining conversions of the session skip the attempt
     instead of re-raising the same failure on every call. *)
  If[stage === "Call", $typstNativeLinkFailed = True];
  nativeFallback[boxes, stage, field, reason]
];

(* ------------------------------------------------------------------------ *)
(* Public options & entry points                                            *)
(* ------------------------------------------------------------------------ *)

Options[BoxesToTypst] = $typstBoxOptionDefaults;
Options[TypstForm] = $typstExprOptionDefaults;
Options[MathemeToTypst] = $typstExprOptionDefaults;

SetAttributes[MakeTypst, HoldFirst];
MakeTypst[_, ___] := Missing["NotDefined"];

BoxesToTypst[boxes_, opts : OptionsPattern[]] :=
  withTypstConfig[
    Options[BoxesToTypst],
    {opts},
    Function[cfg,
      StringTrim[convertBoxNative[stripIdentityBoxes[boxes]]]
    ]
  ];

MathemeToTypst[expr_, opts : OptionsPattern[]] :=
  withTypstConfig[
    Options[MathemeToTypst],
    {opts},
    Function[cfg,
      Module[{form, exprRules, boxes},
        form = Lookup[cfg, "Form", TraditionalForm];
        exprRules = normalizeRuleList[Lookup[cfg, "ExpressionRules", {}]];
        boxes = makeBoxesPreprocessed[expr, form, exprRules];
        StringTrim[convertBoxNative[stripIdentityBoxes[boxes]]]
      ]
    ]
  ];

(* Used by TypstForm formatting and MathemeCompile[TypstForm[...]] so that
   SetOptions[TypstForm, ...] is respected consistently everywhere. *)
typstFormToString[expr_, opts : OptionsPattern[]] :=
  withTypstConfig[
    Options[TypstForm],
    {opts},
    Function[cfg,
      Module[{form, exprRules, boxes},
        form = Lookup[cfg, "Form", TraditionalForm];
        exprRules = normalizeRuleList[Lookup[cfg, "ExpressionRules", {}]];
        boxes = makeBoxesPreprocessed[expr, form, exprRules];
        StringTrim[convertBoxNative[stripIdentityBoxes[boxes]]]
      ]
    ]
  ];

Format[TypstForm[expr_, opts : OptionsPattern[]], OutputForm] :=
  typstFormToString[expr, opts];

TypstForm /: MakeBoxes[t : TypstForm[expr_, opts : OptionsPattern[]], StandardForm | TraditionalForm] :=
  With[{str = typstFormToString[expr, opts]},
    InterpretationBox[StyleBox[str, "Output", ShowStringCharacters -> False], t]
  ];

End[];
