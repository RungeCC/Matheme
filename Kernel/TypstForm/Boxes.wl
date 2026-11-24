(* ::Package:: *)

(* TypstForm/Boxes.wl: Context-aware Box -> Typst conversion engine. *)

Begin["`TypstForm`"];

(* ------------------------------------------------------------------------ *)
(* Box normalization & predicates                                           *)
(* ------------------------------------------------------------------------ *)

makeIconizedObject[args_] := Replace[args, {
  {_, StyleBox[custom_, "IconizedCustomName", ___], ___} :> custom,
  {_, "ListIcon", ___} :> RowBox[{"{", "\:2026", "}"}],
  {_, "AssociationIcon", ___} :> RowBox[{"\:f113", "\:2026", "\:f114"}],
  {_, "StringIcon", ___} :> "\"\:2026\"",
  {_, "SequenceIcon", ___} :> "\:2026",
  {_, other_, ___} :> (other //. DynamicBox[FEPrivate`FrontEndResource["FEBitmaps", "IconizeEllipsis"], ___] :> "\:2026"),
  _ :> "\:2026"
}];

stripIdentityBoxes[boxes_] := boxes //. {
  RowBox[{x_}] :> x,
  InterpretationBox[TemplateBox[_, "NumericalApproximation", ___], x_, ___] :>
    Block[{BoxForm`UseApproximations = False},
      MakeBoxes[x, Lookup[$typstActiveConfig, "Form", TraditionalForm]]
    ],
  InterpretationBox[DynamicModuleBox[_, TemplateBox[args_, "IconizedObject", ___], ___], __] :>
    makeIconizedObject[args],
  (* Preserve InactiveToken TagBoxes so infix ^ / operators are handled cleanly *)
  TagBox[x_, tag_, ___] /; tag =!= "InactiveToken" :> x,
  (AdjustmentBox | InterpretationBox | DynamicBox | TooltipBox | Dynamic)[x_, ___] :> x
};

delimiterBoxQ[boxes_List] := !FreeQ[boxes,
  GridBox | SuperscriptBox | SubscriptBox | SubsuperscriptBox |
  OverscriptBox | UnderscriptBox | UnderoverscriptBox |
  FractionBox | SqrtBox | RadicalBox | FrameBox | ButtonBox | PaneBox | PanelBox
];
delimiterBoxQ[boxes___] := delimiterBoxQ[{boxes}];

invisibleSpacerQ[s_String] := MemberQ[{"\:2060", "\:f360", "\:f760", "\:f76d", "\:f3a0", ""}, s];
invisibleSpacerQ[_] := False;

matrixPaddingSpacerQ[s_String] :=
  invisibleSpacerQ[s] || MemberQ[{" ", "\:205f", "\:2009", "\:200a", "\:2005", "\:f3b3", "\:f3b4", "\:f3ad"}, s];
matrixPaddingSpacerQ[_] := False;

explicitParenthesizedBoxQ[box_] := Module[{s = stripIdentityBoxes[box]},
  MatchQ[s, RowBox[{"(", ___, ")"}]]
];

(* ------------------------------------------------------------------------ *)
(* Number & String formatting (no ToExpression!)                            *)
(* ------------------------------------------------------------------------ *)

stripOuterQuotes[s_String] :=
  If[StringLength[s] >= 2 && StringStartsQ[s, "\""] && StringEndsQ[s, "\""],
    StringTake[s, {2, -2}],
    s
  ];

(* Parse Wolfram box number strings cleanly without ToExpression:
   "2.5`*^-6", "1.23`*^15", "3.14`5.", "3.14`5.*^-2", "3.14`" *)
formatWolframNumberString[str_String] := Block[{m, mant, exp},
  (* Mantissa with precision mark and scientific exponent *)
  m = StringCases[str,
    StartOfString ~~ mant : (("-" | "+" | "") ~~ DigitCharacter .. ~~ ("." ~~ DigitCharacter ...) | "." ~~ DigitCharacter ..) ~~
      "`" ~~ (DigitCharacter ... ~~ ("." ~~ DigitCharacter ...) | "") ~~
      "*^" ~~ exp : (("-" | "+" | "") ~~ DigitCharacter ..) ~~ EndOfString :>
      {mant, exp}
  ];
  If[Length[m] === 1,
    {mant, exp} = First[m];
    If[StringEndsQ[mant, "."], mant = StringDrop[mant, -1]];
    Return[mant <> " times 10^" <> makeScriptString[exp, False], Module]
  ];

  (* Mantissa with bare scientific exponent *^ without precision mark *)
  m = StringCases[str,
    StartOfString ~~ mant : (("-" | "+" | "") ~~ DigitCharacter .. ~~ ("." ~~ DigitCharacter ...) | "." ~~ DigitCharacter ..) ~~
      "*^" ~~ exp : (("-" | "+" | "") ~~ DigitCharacter ..) ~~ EndOfString :>
      {mant, exp}
  ];
  If[Length[m] === 1,
    {mant, exp} = First[m];
    If[StringEndsQ[mant, "."], mant = StringDrop[mant, -1]];
    Return[mant <> " times 10^" <> makeScriptString[exp, False], Module]
  ];

  (* Mantissa with precision mark only: 3.14` or 3.14`5. *)
  m = StringCases[str,
    StartOfString ~~ mant : (("-" | "+" | "") ~~ DigitCharacter .. ~~ ("." ~~ DigitCharacter ...) | "." ~~ DigitCharacter ..) ~~
      "`" ~~ (DigitCharacter ... ~~ ("." ~~ DigitCharacter ...) | "") ~~ EndOfString :>
      mant
  ];
  If[Length[m] === 1,
    mant = First[m];
    If[StringEndsQ[mant, "."], mant = StringDrop[mant, -1]];
    Return[mant, Module]
  ];

  None
];

formatMultiLetterIdent[str_String] := Switch[
  Lookup[$typstActiveConfig, "MultiLetterStyle", "Quoted"],
  "Italic", "italic(\"" <> escapeTypstString[str] <> "\")",
  "Upright", "upright(\"" <> escapeTypstString[str] <> "\")",
  _, "\"" <> escapeTypstString[str] <> "\""
];

formatStringToken[str_String] := Module[{num, unquoted, mapped},
  (* 1. Quoted string literal from WL, e.g. "\"hello\"" or "\"m\"" or "\"/\"" *)
  If[StringLength[str] >= 2 && StringStartsQ[str, "\""] && StringEndsQ[str, "\""],
    unquoted = StringReplace[StringTake[str, {2, -2}], "\\\"" -> "\""];
    (* Inside Quantity units, "/" is a literal slash, degree is symbol *)
    If[TrueQ[$typstInQuantityUnit],
      Which[
        unquoted === "/", Return["\\/", Module],
        unquoted === "\:00b0", Return["degree", Module]
      ]
    ];
    Return["\"" <> escapeTypstString[unquoted] <> "\"", Module]
  ];

  (* 2. Exact whitespace or plain number *)
  If[str === " ", Return[" ", Module]];
  If[StringMatchQ[str, NumberString], Return[str, Module]];

  (* 3. Wolfram precision / scientific notation string *)
  If[!StringFreeQ[str, "`" | "*^"],
    num = formatWolframNumberString[str];
    If[StringQ[num], Return[num, Module]]
  ];

  (* 3b. Context-qualified Wolfram symbol name, e.g. "MyPkg`Private`x" *)
  If[!StringFreeQ[str, "`"] &&
      StringMatchQ[str, RegularExpression["([A-Za-z$][A-Za-z0-9$]*`)+[^`]+"]],
    If[TrueQ[Lookup[$typstActiveConfig, "StripContext", True]],
      Return[formatStringToken[Last[StringSplit[str, "`"]]], Module],
      Return["\"" <> escapeTypstString[str] <> "\"", Module]
    ]
  ];

  (* 4. Single character *)
  If[StringLength[str] === 1,
    Return[mapTypstChar[str], Module]
  ];

  (* 5. Operator lookup (builtin + custom + user OperatorNames) *)
  If[KeyExistsQ[$typstOperatorMap, str],
    Return[$typstOperatorMap[str] <> " ", Module]
  ];

  (* 6. Greek letter word *)
  If[KeyExistsQ[$typstGreekWords, str],
    Return[$typstGreekWords[str], Module]
  ];

  (* 7. Pure ASCII multi-letter identifier *)
  If[StringMatchQ[str, RegularExpression["[A-Za-z][A-Za-z0-9]*"]],
    Return[formatMultiLetterIdent[str], Module]
  ];

  (* 8. Multi-word ASCII prose / phrase containing spaces (e.g. "hello ", " world", "if ") *)
  If[StringMatchQ[str, RegularExpression["[A-Za-z0-9 ,.:;!?()\\-]+"]] &&
      StringContainsQ[str, RegularExpression["[A-Za-z]{2,}"]],
    Return["\"" <> escapeTypstString[str] <> "\"", Module]
  ];

  (* 9. Mixed character sequence (e.g., Unicode symbols / primes / dots) *)
  mapped = StringJoin[mapTypstChar /@ Characters[str]];
  mapped
];

(* ------------------------------------------------------------------------ *)
(* Token joining & whitespace normalization                                 *)
(* ------------------------------------------------------------------------ *)

testNextNumber[RowBox[{}]] := False;
testNextNumber[elem_String] := StringMatchQ[elem, NumberString];
testNextNumber[(RowBox | SuperscriptBox | SubscriptBox | OverscriptBox |
  UnderscriptBox | SubsuperscriptBox | UnderoverscriptBox | StyleBox |
  AdjustmentBox | InterpretationBox | TagBox | FormBox | Cell |
  BoxData | TextData | TooltipBox | ValueBox | OptionValueBox)[elem_, ___]] :=
  testNextNumber[elem];
testNextNumber[_] := False;

(* Separate adjacent numbers like 2 3 -> 2 med 3 so Typst does not merge them *)
fixWhitespace[elem_List] := Module[{n = Length[elem]},
  Table[
    If[elem[[i]] === " " && i > 1 && i < n &&
        StringQ[elem[[i - 1]]] && StringMatchQ[elem[[i - 1]], NumberString] &&
        testNextNumber[elem[[i + 1]]],
      "\:205f",
      elem[[i]]
    ],
    {i, n}
  ]
];

$typstInfixSpacedOps = {
  "=", "<", ">", "<=", ">=", "!=", "==", "->", ":->", "<->", "=>", "|->",
  "<=>", "<--", "-->", "<-->", "<==", "==>", "<==>", "mid(|)",
  "times", "div", "plus.minus", "minus.plus", "and", "or", "xor", "in", "in.not",
  "in.rev", "in.rev.not", "subset", "supset", "subset.not", "supset.not",
  "subset.eq", "supset.eq", "subset.eq.not", "supset.eq.not",
  "subset.sq", "supset.sq", "subset.sq.eq", "supset.sq.eq",
  "union", "inter", "union.big", "inter.big", "union.plus", "union.sq", "inter.sq",
  "equiv", "equiv.not", "approx", "approx.not", "prop", "tilde.op", "tilde.eq",
  "tilde.eq.not", "tilde.equiv", "tilde.equiv.not", "asymp", "asymp.not",
  "plus.o", "minus.o", "times.o", "dot.o", "dot.op", "compose", "without",
  "divides", "divides.not", "parallel", "parallel.not", "prec", "succ",
  "prec.eq", "succ.eq", "models", "forces"
};

$typstNoLeadingSpaceBefore = {")", "]", "}", ".", "'", ":", "!"};
$typstNoTrailingSpaceAfter = {"(", "[", "{"};

(* Prefix-like tokens after which + or - is unary *)
unaryContextTokenQ[""] := True;
unaryContextTokenQ[prev_String] := Module[{t = StringTrim[prev]},
  t === "" ||
  MemberQ[{"(", "[", "{", ",", ";", "=", "<", ">", "+", "-", "*", "/", ":",
           "<=", ">=", "!=", "==", "->", ":->", "<->", "=>", "|->", "mid(|)",
           "and", "or", "not", "in", "equiv", "approx"}, t] ||
  StringEndsQ[t, "(" | "[" | "{" | "," | ";"]
];

(* Function-like prefix tokens that attach tightly to "(" -> sin(x), Gamma(z), op("erf")(x) *)
tightCallPrefixQ[accTrim_String] :=
  StringMatchQ[accTrim,
    RegularExpression[
      ".*(arccos|arcsin|arctan|arg|cos|cosh|cot|coth|csc|csch|ctg|deg|det|dim|exp|gcd|hom|id|im|inf|ker|lcm|lg|lim|liminf|limsup|ln|log|max|min|mod|Pr|sec|sech|sin|sinc|sinh|sup|tan|tanh|tg|tr|Re|Im|pee|op\\([^()]+\\)|alpha|beta|gamma|delta|epsilon|epsilon\\.alt|zeta|eta|theta|theta\\.alt|iota|kappa|kappa\\.alt|lambda|mu|nu|xi|pi|pi\\.alt|rho|rho\\.alt|sigma|sigma\\.alt|tau|upsilon|phi|phi\\.alt|chi|psi|omega|Gamma|Delta|Theta|Lambda|Xi|Pi|Sigma|Upsilon|Phi|Psi|Omega|Digamma|digamma)$"
    ]
  ];

joinTypstTokens[tokens_List] := Fold[
  Function[{acc, rawTok},
    Module[{tok = rawTok, accTrim, tokTrim, tokCore, accLast, tokFirst},
      Which[
        tok === "", acc,
        acc === "",
          If[tok === " ", "", StringTrim[tok, RegularExpression["^ +"]]],
        True,
          accTrim = StringTrim[acc, RegularExpression[" +$"]];
          tokTrim = StringTrim[tok, RegularExpression["^ +"]];
          tokCore = StringTrim[tok];
          If[tokCore === "",
            acc,
            accLast = StringTake[accTrim, -1];
            tokFirst = StringTake[tokCore, 1];
            Which[
              (* Infix binary relations/arrows/operators: space on both sides *)
              MemberQ[$typstInfixSpacedOps, tokCore],
                accTrim <> " " <> tokCore <> " ",
              (* No space before closing delims / postfix punctuation *)
              MemberQ[$typstNoLeadingSpaceBefore, tokCore] ||
                MemberQ[{")", "]", "}", "'", "!"}, tokFirst],
                accTrim <> tokTrim,
              (* Comma/semicolon: no space before, single space after *)
              tokCore === "," || tokCore === ";",
                accTrim <> tokCore <> " ",
              (* Escaped comma/semicolon inside mat/cases *)
              tokCore === "\\," || tokCore === "\\;",
                accTrim <> tokCore <> " ",
              (* No space right after opening delims *)
              MemberQ[$typstNoTrailingSpaceAfter, accLast],
                accTrim <> tokTrim,
              (* Function / operator / Greek symbol followed by (x) -> sin(x), Gamma(z) *)
              tokFirst === "(" && StringEndsQ[acc, " "] && tightCallPrefixQ[accTrim],
                accTrim <> tokTrim,
              (* Explicit space token or trailing/leading space on adjacent tokens *)
              StringEndsQ[acc, " "] || StringStartsQ[tok, " "],
                accTrim <> " " <> tokTrim,
              (* Space between quoted string and adjacent word/number (except inside Quantity units) *)
              !TrueQ[$typstInQuantityUnit] &&
                ((accLast === "\"" && StringMatchQ[tokFirst, WordCharacter]) ||
                 (StringMatchQ[accLast, WordCharacter | ")"] && tokFirst === "\"")),
                accTrim <> " " <> tokTrim,
              (* Prevent identifier merging: word/digit/")" followed by letter *)
              StringMatchQ[accLast, WordCharacter | ")"] && StringMatchQ[tokFirst, LetterCharacter],
                accTrim <> " " <> tokTrim,
              (* Prevent number merging *)
              StringMatchQ[accLast, DigitCharacter] && StringMatchQ[tokFirst, DigitCharacter],
                accTrim <> " " <> tokTrim,
              True,
                accTrim <> tokTrim
            ]
          ]
      ]
    ]
  ],
  "",
  tokens
];

(* ------------------------------------------------------------------------ *)
(* Script formatting                                                        *)
(* ------------------------------------------------------------------------ *)

makeScriptString[raw_String, forceKeepParens : True | False] := Module[{s = StringTrim[raw]},
  Which[
    s === "", "()",
    forceKeepParens, "(" <> s <> ")",
    StringLength[s] === 1 && StringMatchQ[s, WordCharacter | "*" | "+" | "-"], s,
    StringMatchQ[s, RegularExpression["[A-Za-z]+(\\.[A-Za-z]+)*"]], s,
    StringMatchQ[s, RegularExpression["(upright|bold|italic|bb|cal|scr|frak|sans|mono)\\([^()]+\\)"]], s,
    True, "(" <> s <> ")"
  ]
];

makeScript[in_] := Module[{keepParens, s},
  keepParens = explicitParenthesizedBoxQ[in];
  s = convertInsideDelim[in];
  makeScriptString[s, keepParens]
];

(* ------------------------------------------------------------------------ *)
(* Context helpers                                                          *)
(* ------------------------------------------------------------------------ *)

convertInsideDelim[box_] := StringTrim[
  Block[
    {
      $typstInMatrixCell = False,
      $typstInCasesCell = False,
      $typstInDelimiterDepth = $typstInDelimiterDepth + 1
    },
    convertBox[box]
  ]
];

formatMatrixBox[grid_List, delimSpec_, opts___] := Module[{rows, delimArg, alignOpt, alignArg},
  rows = Map[
    Function[row,
      StringRiffle[
        Map[
          Function[cell,
            StringTrim[
              Block[{$typstInMatrixCell = True, $typstInCasesCell = False},
                convertBox[cell]
              ]
            ]
          ],
          row
        ],
        ", "
      ]
    ],
    grid
  ];
  delimArg = Switch[delimSpec,
    Automatic, "",
    None, "delim: #none, ",
    _String, "delim: " <> delimSpec <> ", ",
    _, ""
  ];
  (* Preserve explicit uniform column alignment when requested *)
  alignOpt = Lookup[Flatten[{opts}], ColumnAlignments, Automatic];
  alignArg = Switch[alignOpt,
    Left | {Left}, "align: #left, ",
    Right | {Right}, "align: #right, ",
    _, ""
  ];
  "mat(" <> delimArg <> alignArg <> StringRiffle[rows, "; "] <> ")"
];

formatPiecewiseBox[grid_List] := Module[{rows},
  rows = Map[
    Function[row,
      StringRiffle[
        Map[
          Function[cell,
            StringTrim[
              Block[{$typstInCasesCell = True, $typstInMatrixCell = False},
                convertBox[cell]
              ]
            ]
          ],
          row
        ],
        " & "
      ]
    ],
    grid
  ];
  "cases(" <> StringRiffle[rows, ", "] <> ")"
];

(* ------------------------------------------------------------------------ *)
(* Color & StyleBox helpers                                                 *)
(* ------------------------------------------------------------------------ *)

wolframColorToTypst[c : (_RGBColor | _GrayLevel | _Hue | _CMYKColor | _XYZColor | _LABColor | _LCHColor | _LUVColor)] := Module[
  {rgb, comps, r, g, b, a},
  rgb = Quiet[ColorConvert[c, "RGB"]];
  If[Head[rgb] =!= RGBColor, Return[None, Module]];
  comps = List @@ rgb;
  If[!AllTrue[comps, NumericQ], Return[None, Module]];
  {r, g, b} = Round[255 * Clip[#, {0, 1}]] & /@ Take[comps, UpTo[3]];
  If[Length[comps] >= 4,
    a = Round[255 * Clip[comps[[4]], {0, 1}]];
    "#rgb(" <> ToString[r] <> ", " <> ToString[g] <> ", " <> ToString[b] <> ", " <> ToString[a] <> ")",
    "#rgb(" <> ToString[r] <> ", " <> ToString[g] <> ", " <> ToString[b] <> ")"
  ]
];
wolframColorToTypst[_] := None;

applyTypstStyles[inner_String, boldQ_, italicQ_, underlineQ_, colorSpec_] := Module[{out = inner, col},
  If[TrueQ[italicQ], out = "italic(" <> out <> ")"];
  If[TrueQ[underlineQ], out = "underline(" <> out <> ")"];
  If[TrueQ[boldQ], out = "bold(" <> out <> ")"];
  col = wolframColorToTypst[colorSpec];
  If[StringQ[col], out = "text(fill: " <> col <> ", " <> out <> ")"];
  out
];

convertStyleOrCell[boxes_, rest___] := Module[
  {args = {rest}, opts, nonOpts, styleNames, fv, boldQ = False, italicQ = False,
   underlineQ = False, colorSpec = None, inner},
  If[!TrueQ[Lookup[$typstActiveConfig, "PreserveStyle", True]],
    Return[convertBox[boxes], Module]
  ];
  opts = Cases[args, _Rule | _RuleDelayed];
  nonOpts = DeleteCases[args, _Rule | _RuleDelayed];
  styleNames = Cases[nonOpts, _String];

  (* Named Cell/StyleBox styles *)
  If[MemberQ[styleNames, "Input" | "InputOnly" | "InlineInput" | "BF" | "TB" | "MB"],
    boldQ = True
  ];
  If[MemberQ[styleNames, "IT" | "TI" | "MO" | "SB"],
    italicQ = True
  ];
  If[MemberQ[styleNames, "TBI" | "MBO" | "SBO"],
    boldQ = True; italicQ = True
  ];

  (* Positional directives from Style[expr, Bold, Italic, Underlined, Red] *)
  If[MemberQ[nonOpts, Bold], boldQ = True];
  If[MemberQ[nonOpts, Italic], italicQ = True];
  If[MemberQ[nonOpts, Underlined], underlineQ = True];
  With[{cols = Cases[nonOpts, _RGBColor | _GrayLevel | _Hue | _CMYKColor]},
    If[Length[cols] > 0, colorSpec = Last[cols]]
  ];

  (* Explicit options *)
  With[{fw = Lookup[opts, FontWeight, None]},
    If[fw === Bold || fw === "Bold", boldQ = True]
  ];
  With[{fs = Lookup[opts, FontSlant, None]},
    If[fs === Italic || fs === "Italic", italicQ = True]
  ];
  With[{fc = Lookup[opts, FontColor, None]},
    If[fc =!= None && fc =!= Automatic, colorSpec = fc]
  ];
  fv = Lookup[opts, FontVariations, {}];
  If[ListQ[fv] && TrueQ[Lookup[fv, "Underline", False]], underlineQ = True];

  inner = StringTrim[convertBox[boxes]];
  applyTypstStyles[inner, boldQ, italicQ, underlineQ, colorSpec]
];

(* ------------------------------------------------------------------------ *)
(* Linear stack-based delimiter pairing for RowBox                          *)
(* ------------------------------------------------------------------------ *)

findMatchingDelimiterSpan[elem_List] := Module[{stack = {}, i, tok, openIdx, openTok, mid},
  For[i = 1, i <= Length[elem], i++,
    tok = elem[[i]];
    If[StringQ[tok],
      If[Length[stack] > 0 && Lookup[$typstMatchingDelimiter, stack[[-1, 2]], None] === tok,
        {openIdx, openTok} = stack[[-1]];
        stack = Most[stack];
        mid = Take[elem, {openIdx + 1, i - 1}];
        (* Always pair semantic/special delims, matrix delims, or delims containing tall boxes *)
        If[!MemberQ[{"(", "[", "{"}, openTok] ||
            delimiterBoxQ[mid] ||
            singleGridInsideQ[mid],
          Return[{openIdx, i}, Module]
        ],
        If[KeyExistsQ[$typstMatchingDelimiter, tok],
          AppendTo[stack, {i, tok}]
        ]
      ]
    ]
  ];
  None
];

singleGridInsideQ[mid_List] := Module[{clean = DeleteCases[mid, _?matrixPaddingSpacerQ]},
  Length[clean] === 1 && MatchQ[First[clean], GridBox[_List, ___]]
];

formatRowElements[elem_List] := Module[{fixed = fixWhitespace[elem], out = {}, i, item, prev = "", tok},
  For[i = 1, i <= Length[fixed], i++,
    item = fixed[[i]];
    tok = Which[
      item === "+" || item === "-" || item === "\:2212",
        With[{sym = If[item === "+", "+", "-"]},
          If[unaryContextTokenQ[prev], sym, " " <> sym <> " "]
        ],
      item === "=" || item === "<" || item === ">",
        " " <> item <> " ",
      item === "," || item === ";",
        mapTypstChar[item],
      True,
        convertBox[item]
    ];
    AppendTo[out, tok];
    If[StringTrim[tok] =!= "", prev = tok]
  ];
  joinTypstTokens[out]
];

(* ------------------------------------------------------------------------ *)
(* Main Box -> Typst dispatcher (with BoxRules interception)                *)
(* ------------------------------------------------------------------------ *)

handleUnsupportedBox[expr_] := Module[{handler},
  handler = Lookup[$typstActiveConfig, "UnsupportedBoxHandler", Automatic];
  If[handler === Automatic,
    Message[TypstForm::unspt, expr]; "",
    With[{res = handler[expr]},
      If[StringQ[res], res, ToString[res]]
    ]
  ]
];

convertBox[box_] := Module[{boxRules, matched},
  If[$typstEvalDepth > $typstMaxEvalDepth,
    Message[TypstForm::unspt, box];
    Return["", Module]
  ];
  boxRules = normalizeRuleList[Lookup[$typstActiveConfig, "BoxRules", {}]];
  If[boxRules =!= {},
    matched = Replace[box, Append[boxRules, _ -> $typstNoRuleMatch]];
    If[matched =!= $typstNoRuleMatch,
      Return[
        Which[
          StringQ[matched], matched,
          MatchQ[matched, TypstRaw[_String]], First[matched],
          True, Block[{$typstEvalDepth = $typstEvalDepth + 1}, convertBox[matched]]
        ],
        Module
      ]
    ]
  ];
  convertBoxCore[box]
];

(* ------------------------------------------------------------------------ *)
(* Core Box rules                                                           *)
(* ------------------------------------------------------------------------ *)

(* Interactive / UI boxes *)
convertBoxCore[CheckBox[False]] := mapTypstChar["\:25a1"];
convertBoxCore[CheckBox[True]] := "\:2611";
convertBoxCore[CheckBox[Dynamic[expr_]]] := convertBox[CheckBox[expr]];
convertBoxCore[OpenerBox[False]] := mapTypstChar["\:25b6"];
convertBoxCore[OpenerBox[True]] := mapTypstChar["\:25bc"];
convertBoxCore[OpenerBox[Dynamic[expr_]]] := convertBox[OpenerBox[expr]];
convertBoxCore[RadioButtonBox[Dynamic[expr_], val_]] := convertBox[RadioButtonBox[expr, val]];
convertBoxCore[RadioButtonBox[val_, val_]] := "dot.o ";
convertBoxCore[RadioButtonBox[_, _]] := "circle.stroked ";
convertBoxCore[TooltipBox[boxes_, ___]] := convertBox[boxes];
convertBoxCore[ErrorBox[boxes_, ___]] := "underline(" <> StringTrim[convertBox[boxes]] <> ")";
convertBoxCore[AdjustmentBox[boxes_, ___]] := convertBox[boxes];
convertBoxCore[InterpretationBox[boxes_, ___]] := convertBox[boxes];
convertBoxCore[TagBox[op_String, "InactiveToken", ___]] := Which[
  op === "+", " + ",
  op === "-", " - ",
  op === "*", " times ",
  op === "/", "/",
  True, formatStringToken[op]
];
convertBoxCore[TagBox[boxes_, ___]] := convertBox[boxes];
convertBoxCore[DynamicBox[boxes_, ___]] := convertBox[boxes];
convertBoxCore[(AnimatorBox | SliderBox)[___]] := (Message[TypstForm::unspt, "Animator/Slider"]; "");
convertBoxCore[InputFieldBox[boxes_, ___]] := convertBox[boxes];
convertBoxCore[PopupMenuBox[val_, {labels__?OptionQ}, ___]] := convertBox[val /. {labels}];
convertBoxCore[PopupMenuBox[val_, ___]] := convertBox[ToBoxes[val]];
convertBoxCore[PaneSelectorBox[rules_, Dynamic[expr_], opts___]] :=
  convertBox[PaneSelectorBox[rules, expr, opts]];
convertBoxCore[PaneSelectorBox[{rules__?OptionQ}, val_, ___]] := convertBox[val /. {rules}];
convertBoxCore[FrameBox[boxes_, ___]] :=
  "#box(stroke: 0.5pt, inset: 2pt, $" <> StringTrim[convertBox[boxes]] <> "$)";
convertBoxCore[(PaneBox | PanelBox | ButtonBox)[boxes_, ___]] := convertBox[boxes];

(* Document / Container boxes *)
convertBoxCore[Notebook[{cell_}, ___]] := convertBox[cell];
convertBoxCore[Notebook[cells_List, ___]] := StringRiffle[StringTrim[convertBox[#]] & /@ cells, " \\\n\\\n"];
convertBoxCore[BoxData[boxes_List]] := StringRiffle[StringTrim[convertBox[#]] & /@ boxes, " \\\n"];
convertBoxCore[BoxData[boxes_]] := convertBox[boxes];
convertBoxCore[TextData[text_]] := convertBox[RowBox[Flatten[{text}]]];
convertBoxCore[FormBox[boxes_, _, ___]] := convertBox[boxes];
convertBoxCore[TemplateArgBox[lst_List]] := convertBox[RowBox[lst]];
convertBoxCore[TemplateArgBox[expr_]] := convertBox[expr];

(* StyleBox and Cell *)
convertBoxCore[(StyleBox | Cell)[boxes_, rest___]] := convertStyleOrCell[boxes, rest];

(* TemplateBox *)
convertBoxCore[tbox_TemplateBox] := convertTemplateBox[tbox];

(* Strings *)
convertBoxCore[str_String] := formatStringToken[str];

(* Special RowBox patterns *)
convertBoxCore[RowBox[{"T", AdjustmentBox["E", ___], "X"}]] := "\"TeX\"";
convertBoxCore[RowBox[{"L", StyleBox[AdjustmentBox["A", ___], ___], "T", AdjustmentBox["E", ___], "X"}]] := "\"LaTeX\"";

(* Piecewise *)
convertBoxCore[GridBox[{{"\:f361", GridBox[grid_List, ___]}}, ___?OptionQ]] :=
  formatPiecewiseBox[grid];
convertBoxCore[RowBox[{"\:f361", GridBox[grid_List, ___]}]] :=
  formatPiecewiseBox[grid];

(* Pre-subscripted operators like HypergeometricPFQ: _p F_q *)
convertBoxCore[RowBox[{SubscriptBox["\:f3b3" | "\:f360" | "", pre_, ___], SubscriptBox[base_, post_, ___]}]] :=
  "attach(" <> StringTrim[convertBox[base]] <> ", bl: " <> makeScript[pre] <> ", br: " <> makeScript[post] <> ")";
convertBoxCore[RowBox[{SubscriptBox["\:f3b3" | "\:f360" | "", pre_, ___], base_}]] :=
  "attach(" <> StringTrim[convertBox[base]] <> ", bl: " <> makeScript[pre] <> ")";

(* InactiveToken infix power: x ^ 2 -> x^2 *)
convertBoxCore[RowBox[{base_, TagBox["^", "InactiveToken", ___], super_}]] :=
  convertBox[SuperscriptBox[base, super]];

(* Integrals *)
convertBoxCore[RowBox[{StyleBox["\:222b", ___] | "\:222b", RowBox[{integrand_, RowBox[{StyleBox["\:f74c", ___] | "\:f74c", var_}]}]}]] :=
  "integral " <> StringTrim[convertBox[integrand]] <> " thin " <> StringTrim[mapTypstChar["\:f74c"]] <> " " <> StringTrim[convertBox[var]];
convertBoxCore[RowBox[{SubsuperscriptBox[StyleBox["\:222b", ___] | "\:222b", sub_, super_, ___], RowBox[{integrand_, RowBox[{StyleBox["\:f74c", ___] | "\:f74c", var_}]}]}]] :=
  "integral_" <> makeScript[sub] <> "^" <> makeScript[super] <> " " <>
  StringTrim[convertBox[integrand]] <> " thin " <> StringTrim[mapTypstChar["\:f74c"]] <> " " <> StringTrim[convertBox[var]];

(* mod *)
convertBoxCore[RowBox[{l__, " ", "mod", " ", r__}]] :=
  "(" <> StringTrim[convertBox[RowBox[{l}]]] <> " mod " <> StringTrim[convertBox[RowBox[{r}]]] <> ")";

(* Comments *)
convertBoxCore[RowBox[{"(*", c1___String, RowBox[{c2___String}], c3___String, "*)"}]] :=
  "\"(*" <> escapeTypstString[StringJoin[c1, c2, c3]] <> "*)\"";

(* General RowBox with linear stack delimiter pairing *)
convertBoxCore[RowBox[elem_List]] := Module[
  {span, openIdx, closeIdx, lb, rb, l, mid, r, cleanMid, gridBox, matDelimSpec,
   ld, rd, inner, wrapped, semFn, delimStyle},
  span = findMatchingDelimiterSpan[elem];
  If[span === None,
    Return[formatRowElements[elem], Module]
  ];
  {openIdx, closeIdx} = span;
  lb = elem[[openIdx]];
  rb = elem[[closeIdx]];
  l = Take[elem, openIdx - 1];
  mid = Take[elem, {openIdx + 1, closeIdx - 1}];
  r = Drop[elem, closeIdx];

  (* Fold ( GridBox ), [ GridBox ], | GridBox |, { GridBox } into idiomatic mat(...) *)
  cleanMid = DeleteCases[mid, _?matrixPaddingSpacerQ];
  If[Lookup[$typstActiveConfig, "MatrixDelimiter", Automatic] === Automatic &&
      Length[cleanMid] === 1 && MatchQ[First[cleanMid], GridBox[_List, ___]] &&
      KeyExistsQ[$typstMatrixDelimiterSpec, lb],
    gridBox = First[cleanMid];
    matDelimSpec = $typstMatrixDelimiterSpec[lb];
    wrapped = formatMatrixBox[First[gridBox], matDelimSpec, Rest[List @@ gridBox]];
    Return[
      joinTypstTokens[{
        If[l === {}, "", convertBox[RowBox[l]]],
        wrapped,
        If[r === {}, "", convertBox[RowBox[r]]]
      }],
      Module
    ]
  ];

  (* Semantic delimiter functions: abs(...), norm(...), floor(...), ceil(...) *)
  delimStyle = Lookup[$typstActiveConfig, "DelimiterStyle", "Semantic"];
  semFn = Lookup[$typstSemanticDelimiterFunctions, lb, None];
  If[delimStyle === "Semantic" && StringQ[semFn],
    inner = convertInsideDelim[RowBox[mid]];
    wrapped = semFn <> "(" <> inner <> ")";
    Return[
      joinTypstTokens[{
        If[l === {}, "", convertBox[RowBox[l]]],
        wrapped,
        If[r === {}, "", convertBox[RowBox[r]]]
      }],
      Module
    ]
  ];

  (* Standard and lr(...) delimiters *)
  ld = $typstLeftDelimiters[lb];
  rd = $typstRightDelimiters[rb];
  inner = convertInsideDelim[RowBox[mid]];
  wrapped = Which[
    MemberQ[{"(", "[", "{"}, lb] && MemberQ[{")", "]", "}"}, rb],
      ld <> inner <> rd,
    ld === "|" && rd === "|",
      "lr(|" <> inner <> "|)",
    True,
      "lr(" <> ld <> " " <> inner <> " " <> rd <> ")"
  ];
  joinTypstTokens[{
    If[l === {}, "", convertBox[RowBox[l]]],
    wrapped,
    If[r === {}, "", convertBox[RowBox[r]]]
  }]
];

(* Superscripts and Subscripts *)
convertBoxCore[SuperscriptBox[base_, super_String, ___]] /;
  StringLength[super] > 0 && Complement[Union[Characters[super]], {"'", "\:2032"}] === {} :=
  StringTrim[convertBox[base]] <> StringRepeat["'", StringLength[super]];

convertBoxCore[SuperscriptBox[base_, super_, ___]] :=
  StringTrim[convertBox[base]] <> "^" <> makeScript[super];

convertBoxCore[SuperscriptBox[base_, ___?OptionQ]] := convertBox[base];

convertBoxCore[SubscriptBox[base_, sub_, ___]] := Module[{b = StringTrim[convertBox[base]]},
  If[!StringFreeQ[b, "^"],
    "attach(" <> b <> ", b: " <> convertInsideDelim[sub] <> ")",
    b <> "_" <> makeScript[sub]
  ]
];

convertBoxCore[SubsuperscriptBox[base_, sub_, super_, ___]] :=
  StringTrim[convertBox[base]] <> "_" <> makeScript[sub] <> "^" <> makeScript[super];

convertBoxCore[SubsuperscriptBox[base_, ___?OptionQ]] := convertBox[base];

(* OverscriptBox: unwrap optional literal quotes on accent tokens like "\"^\"" or "\"\:fe37\"" *)
convertBoxCore[OverscriptBox[base_, rawOver_String, opts___]] /;
  StringLength[rawOver] >= 2 && StringStartsQ[rawOver, "\""] && StringEndsQ[rawOver, "\""] :=
  convertBoxCore[OverscriptBox[base, stripOuterQuotes[rawOver], opts]];

convertBoxCore[OverscriptBox[base_String /; StringLength[base] == 1, "_", ___]] :=
  "bar(" <> StringTrim[convertBox[base]] <> ")";
convertBoxCore[OverscriptBox[base_, "_" | "\:2500" | "\:00af", ___]] :=
  "overline(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "\:2190" | "\:27f5" | "\:f526", ___]] :=
  "accent(" <> convertInsideDelim[base] <> ", arrow.l)";
convertBoxCore[OverscriptBox[base_, "\:2192" | "\:27f6" | "\:f525" | "\:21c0", ___]] :=
  "arrow(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "\:2194" | "\:27f7", ___]] :=
  "accent(" <> convertInsideDelim[base] <> ", arrow.l.r)";
convertBoxCore[OverscriptBox[base_, "^" | "\:02c6", ___]] :=
  "hat(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "\:02c7", ___]] :=
  "caron(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "~" | "\:223c" | "\:02dc", ___]] :=
  "tilde(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "'" | "\:2032" | "\:00b4", ___]] :=
  "acute(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "`" | "\:2035", ___]] :=
  "grave(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "\:02d8", ___]] :=
  "breve(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "." | "\:2022" | "\:00b7", ___]] :=
  "dot(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, ".." | "\:2022\:2022" | "\:00b7\:00b7" | "\:00a8", ___]] :=
  "dot.double(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "..." | "\:2022\:2022\:2022" | "\:00b7\:00b7\:00b7" | "\:f758" | "\:2026" | "\:22ef", ___]] :=
  "dot.triple(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[base_, "...." | "\:2022\:2022\:2022\:2022" | "\:00b7\:00b7\:00b7\:00b7" | "\:00a8\:00a8", ___]] :=
  "dot.quad(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[OverscriptBox[base_, "\:fe37" | "\"\:fe37\"", ___], script_, ___]] :=
  "overbrace(" <> convertInsideDelim[base] <> ", " <> convertInsideDelim[script] <> ")";
convertBoxCore[OverscriptBox[base_, OverscriptBox["\:fe37" | "\"\:fe37\"", script_, ___], ___]] :=
  "overbrace(" <> convertInsideDelim[base] <> ", " <> convertInsideDelim[script] <> ")";
convertBoxCore[OverscriptBox[base_, "\:fe37", ___]] :=
  "overbrace(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[OverscriptBox[base_, "\:fe35" | "\"\:fe35\"", ___], script_, ___]] :=
  "overparen(" <> convertInsideDelim[base] <> ", " <> convertInsideDelim[script] <> ")";
convertBoxCore[OverscriptBox[base_, "\:fe35", ___]] :=
  "overparen(" <> convertInsideDelim[base] <> ")";
convertBoxCore[OverscriptBox[sp : "\:2211" | "\:220f" | "\:22c3" | "\:22c2" | "\:22c0" | "\:22c1" | "\:2210" | "\:f3d9", over_, ___]] :=
  StringTrim[convertBox[sp]] <> "^" <> makeScript[over] <> " ";
convertBoxCore[OverscriptBox[base_, over_, ___]] :=
  "attach(" <> convertInsideDelim[base] <> ", t: " <> convertInsideDelim[over] <> ")";

(* UnderscriptBox *)
convertBoxCore[UnderscriptBox[base_, rawUnder_String, opts___]] /;
  StringLength[rawUnder] >= 2 && StringStartsQ[rawUnder, "\""] && StringEndsQ[rawUnder, "\""] :=
  convertBoxCore[UnderscriptBox[base, stripOuterQuotes[rawUnder], opts]];

convertBoxCore[UnderscriptBox[base_, "_" | "\:2500", ___]] :=
  "underline(" <> convertInsideDelim[base] <> ")";
convertBoxCore[UnderscriptBox[base_, "\:2194" | "\:27f7", ___]] :=
  "attach(" <> convertInsideDelim[base] <> ", b: arrow.l.r)";
convertBoxCore[UnderscriptBox[base_, "\:2192" | "\:27f6" | "\:f525", ___]] :=
  "attach(" <> convertInsideDelim[base] <> ", b: arrow.r)";
convertBoxCore[UnderscriptBox[UnderscriptBox[base_, "\:fe38" | "\"\:fe38\"", ___], script_, ___]] :=
  "underbrace(" <> convertInsideDelim[base] <> ", " <> convertInsideDelim[script] <> ")";
convertBoxCore[UnderscriptBox[base_, UnderscriptBox["\:fe38" | "\"\:fe38\"", script_, ___], ___]] :=
  "underbrace(" <> convertInsideDelim[base] <> ", " <> convertInsideDelim[script] <> ")";
convertBoxCore[UnderscriptBox[base_, "\:fe38", ___]] :=
  "underbrace(" <> convertInsideDelim[base] <> ")";
convertBoxCore[UnderscriptBox[UnderscriptBox[base_, "\:fe36" | "\"\:fe36\"", ___], script_, ___]] :=
  "underparen(" <> convertInsideDelim[base] <> ", " <> convertInsideDelim[script] <> ")";
convertBoxCore[UnderscriptBox[base_, "\:fe36", ___]] :=
  "underparen(" <> convertInsideDelim[base] <> ")";
convertBoxCore[UnderscriptBox[sp : "\:2211" | "\:220f" | "\:22c3" | "\:22c2" | "\:22c0" | "\:22c1" | "\:2210" | "\:f3d9", under_, ___]] :=
  StringTrim[convertBox[sp]] <> "_" <> makeScript[under] <> " ";
convertBoxCore[UnderscriptBox[op : "lim" | "liminf" | "limsup" | "max" | "min" | "sup" | "inf" | "\:f438" | "\:f439" | "\:f43a", under_, ___]] :=
  StringTrim[convertBox[op]] <> "_" <> makeScript[under] <> " ";
convertBoxCore[UnderscriptBox[base_, under_, ___]] :=
  "attach(" <> convertInsideDelim[base] <> ", b: " <> convertInsideDelim[under] <> ")";

(* UnderoverscriptBox *)
convertBoxCore[UnderoverscriptBox[sp : "\:2211" | "\:220f" | "\:22c3" | "\:22c2" | "\:22c0" | "\:22c1" | "\:2210" | "\:f3d9", under_, over_, ___]] :=
  StringTrim[convertBox[sp]] <> "_" <> makeScript[under] <> "^" <> makeScript[over] <> " ";
convertBoxCore[UnderoverscriptBox[base_, under_, over_, ___]] :=
  "attach(" <> convertInsideDelim[base] <> ", b: " <> convertInsideDelim[under] <> ", t: " <> convertInsideDelim[over] <> ")";

(* Sqrt, Radical, Fraction *)
convertBoxCore[SqrtBox[x_, ___]] := "sqrt(" <> convertInsideDelim[x] <> ")";
convertBoxCore[RadicalBox[x_, y_, ___]] := "root(" <> convertInsideDelim[y] <> ", " <> convertInsideDelim[x] <> ")";
convertBoxCore[FractionBox[num_, den_, ___]] := Module[{n, d},
  n = convertInsideDelim[num];
  d = convertInsideDelim[den];
  If[Lookup[$typstActiveConfig, "FractionStyle", "Fraction"] === "Slash",
    "(" <> n <> ")/(" <> d <> ")",
    "frac(" <> n <> ", " <> d <> ")"
  ]
];

(* GridBox -> Typst mat(delim: #none, ...) *)
convertBoxCore[GridBox[grid_List, opts___?OptionQ]] :=
  formatMatrixBox[grid, None, opts];

(* List fallback *)
convertBoxCore[lst_List] := convertBox[RowBox[lst]];

(* Unsupported fallback *)
convertBoxCore[expr_] := handleUnsupportedBox[expr];

End[];
