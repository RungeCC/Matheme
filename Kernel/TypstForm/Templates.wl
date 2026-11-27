(* ::Package:: *)

(* TypstForm/Templates.wl: TemplateBox dispatch (user TemplateRules + built-in Wolfram templates). *)

Begin["`TypstForm`"];

(* ------------------------------------------------------------------------ *)
(* Semantic vs Explicit delimiter helpers                                   *)
(* ------------------------------------------------------------------------ *)

formatDelimitedExpr[semFn_String, leftDelim_String, rightDelim_String, innerBox_] := Module[{inner},
  inner = convertInsideDelim[innerBox];
  If[Lookup[$typstActiveConfig, "DelimiterStyle", "Semantic"] === "Explicit",
    If[leftDelim === "|" && rightDelim === "|",
      "lr(|" <> inner <> "|)",
      "lr(" <> leftDelim <> " " <> inner <> " " <> rightDelim <> ")"
    ],
    semFn <> "(" <> inner <> ")"
  ]
];

formatScriptBaseBox[box_] := Module[{stripped = stripIdentityBoxes[box], s},
  s = StringTrim[convertBox[box]];
  Which[
    MatchQ[stripped, RowBox[{_, "+" | "-" | "\:2212" | "\:00b1" | "\:2213", __}]],
      "(" <> s <> ")",
    True,
      s
  ]
];

(* ------------------------------------------------------------------------ *)
(* Built-in Wolfram TemplateBox handlers                                    *)
(* ------------------------------------------------------------------------ *)

convertBuiltinTemplate["MathemeTypstRaw", {raw_String}, _] := raw;
convertBuiltinTemplate["MathemeTypstBox", {box_}, _] := convertBox[box];

convertBuiltinTemplate["Abs", {x_}, _] :=
  formatDelimitedExpr["abs", "|", "|", x];

convertBuiltinTemplate["Norm", {x_}, _] :=
  formatDelimitedExpr["norm", "bar.v.double", "bar.v.double", x];

convertBuiltinTemplate["Norm2", {x_, p_}, _] :=
  formatDelimitedExpr["norm", "bar.v.double", "bar.v.double", x] <> "_" <> makeScript[p];

convertBuiltinTemplate["Floor", {x_}, _] :=
  formatDelimitedExpr["floor", "floor.l", "floor.r", x];

convertBuiltinTemplate["Ceiling", {x_}, _] :=
  formatDelimitedExpr["ceil", "ceil.l", "ceil.r", x];

convertBuiltinTemplate["Round", {x_}, _] :=
  "round(" <> convertInsideDelim[x] <> ")";

convertBuiltinTemplate["Binomial", {n_, k_}, _] :=
  "binom(" <> convertInsideDelim[n] <> ", " <> convertInsideDelim[k] <> ")";

convertBuiltinTemplate["QBinomial", {n_, m_, q_}, _] :=
  "binom(" <> convertInsideDelim[n] <> ", " <> convertInsideDelim[m] <> ")_" <> makeScript[q];

convertBuiltinTemplate["Limit2Arg", {expr_, var_, val_}, _] :=
  "lim_(" <> convertInsideDelim[var] <> " -> " <> convertInsideDelim[val] <> ") " <>
  StringTrim[convertBox[expr]];

convertBuiltinTemplate["LimitWithSuperscript", {expr_, var_, val_, _, dir_}, _] :=
  "lim_(" <> convertInsideDelim[var] <> " -> " <> convertInsideDelim[val] <> "^" <>
  makeScript[dir] <> ") " <> StringTrim[convertBox[expr]];

convertBuiltinTemplate["ConditionalExpression", {expr_, cond_}, _] :=
  StringTrim[convertBox[expr]] <> " quad \"if\" quad " <> StringTrim[convertBox[cond]];

convertBuiltinTemplate["Det", {arg_}, _] := Module[{stripped = stripIdentityBoxes[arg]},
  If[MatchQ[stripped, GridBox[_List, ___]],
    formatMatrixBox[First[stripped], "\"|\"", Rest[List @@ stripped]],
    "det(" <> convertInsideDelim[arg] <> ")"
  ]
];

convertBuiltinTemplate["Conjugate", {base_}, _] :=
  If[Lookup[$typstActiveConfig, "ConjugateStyle", "Superscript"] === "Overline",
    "overline(" <> convertInsideDelim[base] <> ")",
    formatScriptBaseBox[base] <> "^*"
  ];

convertBuiltinTemplate["Transpose", {base_}, _] := Module[{sym},
  sym = Lookup[$typstActiveConfig, "TransposeSymbol", "upright(T)"];
  formatScriptBaseBox[base] <> "^" <> makeScriptString[sym, False]
];

convertBuiltinTemplate["ConjugateTranspose" | "HermitianConjugate", {base_}, _] :=
  formatScriptBaseBox[base] <> "^dagger";

convertBuiltinTemplate["Superscript", {base_, super_}, _] :=
  convertBox[SuperscriptBox[base, super]];

convertBuiltinTemplate["Subscript", {base_, sub_}, _] :=
  convertBox[SubscriptBox[base, sub]];

convertBuiltinTemplate["Subsuperscript", {base_, sub_, super_}, _] :=
  convertBox[SubsuperscriptBox[base, sub, super]];

convertBuiltinTemplate["Quantity" | "QuantityTF", {val_, unit_, ___}, _] := Module[{v, u},
  v = StringTrim[convertBox[val]];
  u = StringTrim[Block[{$typstInQuantityUnit = True}, convertBox[unit]]];
  If[u === "", v, v <> " thin " <> u]
];

convertBuiltinTemplate["QuantityPostfix" | "QuantityPostfixTF", {val_, unit_, ___}, _] := Module[{v, u},
  v = StringTrim[convertBox[val]];
  u = StringTrim[Block[{$typstInQuantityUnit = True}, convertBox[unit]]];
  v <> u
];

convertBuiltinTemplate["QuantityUnit", {unit_, ___}, _] :=
  StringTrim[Block[{$typstInQuantityUnit = True}, convertBox[unit]]];

convertBuiltinTemplate["Spacer1", {w_?NumericQ}, _] :=
  "#h(" <> ToString[N[w]] <> "pt)";
convertBuiltinTemplate["Spacer2", {w_?NumericQ, _}, _] :=
  "#h(" <> ToString[N[w]] <> "pt)";

convertBuiltinTemplate["RowDefault" | "RowNoSeparators", args_List, _] :=
  joinTypstTokens[convertBox /@ args];

convertBuiltinTemplate["RowWithSeparators", {sep_, _, items___}, _] := Module[{sepStr},
  sepStr = convertBox[sep];
  joinTypstTokens[Riffle[convertBox /@ {items}, sepStr]]
];

convertBuiltinTemplate["IconizedObject", args_List, _] :=
  convertBox[makeIconizedObject[args]];

convertBuiltinTemplate["NumericalApproximation", {disp_, ___}, _] :=
  convertBox[disp];

(* Fallback: expand via Wolfram's TemplateBoxToDisplayBoxes *)
convertBuiltinTemplate[_, _, tbox_TemplateBox] := Module[{expanded},
  expanded = Quiet[BoxForm`TemplateBoxToDisplayBoxes[tbox]];
  If[expanded === tbox || Head[expanded] === BoxForm`TemplateBoxToDisplayBoxes,
    handleUnsupportedBox[tbox],
    convertBox[stripIdentityBoxes[expanded]]
  ]
];

(* ------------------------------------------------------------------------ *)
(* TypstTemplate: a native data value, not Wolfram StringTemplate            *)
(* ------------------------------------------------------------------------ *)

(* The body is a tinytemplate value-interpolation subset: {n} addresses an
   already-rendered string argument, {rest} inserts every argument not claimed
   by a fixed slot, and \{ is a literal opening brace. The Rust side compiles
   the same subset; blocks such as {{ if ... }} are rejected there. *)
typstTemplateApply[TypstTemplate[body_String], args_List] :=
  typstTemplateApply[TypstTemplate[body, ", "], args];

typstTemplateApply[TypstTemplate[body_String, sep_String], args_List] := Module[
  {escaped, slots, used, rest, result},
  escaped = StringReplace[body, "\\{" -> "\:FDD0"];
  slots = FromDigits /@ StringCases[escaped, "{" ~~ d : DigitCharacter .. ~~ "}" :> d];
  If[AnyTrue[slots, # > Length[args] &],
    Message[TypstForm::unspt, body];
    Return[""]
  ];
  used = DeleteDuplicates[slots];
  rest = Select[Range[Length[args]], !MemberQ[used, #] &];
  result = StringReplace[escaped, {
    "{" ~~ d : DigitCharacter .. ~~ "}" :> args[[FromDigits[d]]],
    "{rest}" :> StringRiffle[args[[rest]], sep]
  }];
  If[StringContainsQ[result, "{"],
    Message[TypstForm::unspt, body];
    Return[""]
  ];
  StringReplace[result, "\:FDD0" -> "{"]
];

(* ------------------------------------------------------------------------ *)
(* Public template dispatcher (user TemplateRules take priority)            *)
(* ------------------------------------------------------------------------ *)

applyUserTemplateRule[handler_, args : (_List | _Association)] := Module[{res},
  res = Which[
    StringQ[handler], handler,
    MatchQ[handler, _TypstTemplate], typstTemplateApply[handler, convertInsideDelim /@ args],
    MatchQ[handler, _StringTemplate | _TemplateObject],
      TemplateApply[handler, convertInsideDelim /@ args],
    True,
      handler[args, convertInsideDelim]
  ];
  Which[
    StringQ[res], res,
    MatchQ[res, TypstRaw[_String]], First[res],
    True, convertBox[res]
  ]
];

convertTemplateBox[tbox : TemplateBox[args : (_List | _Association), tag_String, ___]] := Module[{userHandler},
  (* Exact tag/arity wins over the tag-wide key, matching the native
     renderer's Config::template lookup. *)
  userHandler = Lookup[$typstTemplateMap,
    tag <> "/" <> ToString[Length[args]], Missing["NotFound"]];
  If[MissingQ[userHandler],
    userHandler = Lookup[$typstTemplateMap, tag, Missing["NotFound"]]
  ];
  If[!MissingQ[userHandler],
    applyUserTemplateRule[userHandler, args],
    convertBuiltinTemplate[tag, args, tbox]
  ]
];

convertTemplateBox[tbox_TemplateBox] :=
  convertBuiltinTemplate["", {}, tbox];

End[];
