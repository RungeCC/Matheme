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
      StringTrim[convertBox[stripIdentityBoxes[boxes]]]
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
        StringTrim[convertBox[stripIdentityBoxes[boxes]]]
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
        StringTrim[convertBox[stripIdentityBoxes[boxes]]]
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
