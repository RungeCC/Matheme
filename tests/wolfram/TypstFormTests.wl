(* ::Package:: *)
(* Comprehensive correctness, Typst 0.15.1 compilation, and customization test suite for TypstForm. *)

BeginPackage["Matheme`TypstFormTests`", {"Matheme`"}];

RunMathemeTypstFormTests::usage = "RunMathemeTypstFormTests[] executes the TypstForm correctness, compilation, and customization test suite.";

Begin["`Private`"];

hasPuaQ[s_String] :=
  AnyTrue[ToCharacterCode[s], (16^^e000 <= # <= 16^^f8ff) || (16^^f0000 <= # <= 16^^10ffff) &];

compilesCleanlyQ[typst_String] := Module[{res},
  If[hasPuaQ[typst], Return[False, Module]];
  res = Quiet[MathemeToGraphics[typst, "TypstTemplate" -> "Equation"]];
  !FailureQ[res] && AssociationQ[res] &&
    ListQ[res["Graphics"]] && Length[res["Graphics"]] >= 1 &&
    Head[First[res["Graphics"]]] === Graphics
];

RunMathemeTypstFormTests[] := Module[{tests, results, passed, total, symKet, symBra, symConst},
  ClearAll[symKet, symBra, symConst];
  symKet /: MakeTypst[symKet[x_]] := "lr(|" <> MathemeToTypst[x] <> " chevron.r)";
  symBra /: MakeTypst[symBra[x_], conv_] := "lr(chevron.l " <> conv[x] <> "|)";
  symConst /: MakeTypst[symConst] := "cal(C)";

  tests = {
    (* --- Critical regressions reported by user --- *)
    VerificationTest[
      MathemeToTypst[Abs[Global`x]],
      "abs(x)",
      TestID -> "Regression-Abs-Semantic"
    ],
    VerificationTest[
      MathemeToTypst[Abs[Global`x], "DelimiterStyle" -> "Explicit"],
      "lr(|x|)",
      TestID -> "Regression-Abs-Explicit"
    ],
    VerificationTest[
      MathemeToTypst[Abs[Global`x / Global`y]],
      "abs(frac(x, y))",
      TestID -> "Regression-Abs-Fraction"
    ],
    VerificationTest[
      MathemeToTypst[MeijerG[{{Global`a, Global`b, Global`c}, {Global`d, Global`e, Global`f}}, {{Global`a, Global`b, Global`c}, {Global`d, Global`e, Global`f}}, Global`x]],
      "G_(6, 6)^(3, 3)(x mid(|) mat(delim: #none, a\\, b\\, c\\, d\\, e\\, f; a\\, b\\, c\\, d\\, e\\, f))",
      TestID -> "Regression-MeijerG-MidAndEscapedCommas"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics[TypstForm[MeijerG[{{Global`a, Global`b, Global`c}, {Global`d, Global`e, Global`f}}, {{Global`a, Global`b, Global`c}, {Global`d, Global`e, Global`f}}, Global`x]]]},
        !FailureQ[res] && Head[First[res["Graphics"]]] === Graphics &&
          Length[Cases[First[res["Graphics"]], _FilledCurve, Infinity]] >= 15
      ],
      True,
      TestID -> "Regression-MeijerG-EndToEnd-Graphics"
    ],
    VerificationTest[
      MathemeToTypst[EllipticF[Global`phi, Global`m]],
      "F(phi mid(|) m)",
      TestID -> "Regression-EllipticF-VerticalSeparator"
    ],
    VerificationTest[
      Module[{ts = MathemeToTypst[FoxH[{{{Global`a, Global`A}}, {{Global`b, Global`B}}}, {{{Global`c, Global`c1}}, {{Global`d, Global`d1}}}, Global`z]]},
        !hasPuaQ[ts] && StringContainsQ[ts, "mid(|)"] && compilesCleanlyQ[ts]
      ],
      True,
      TestID -> "Regression-FoxH-AssociationTemplateAndMid"
    ],
    VerificationTest[
      MathemeToTypst[WeierstrassP[Global`z, {Global`g2, Global`g3}]],
      "pee(z; \"g2\", \"g3\")",
      TestID -> "SpecialFn-WeierstrassP"
    ],

    (* --- Delimiters, Norms, Floors, Ceilings --- *)
    VerificationTest[
      {
        MathemeToTypst[Norm[Global`x]],
        MathemeToTypst[Norm[Global`x, 2]],
        MathemeToTypst[Floor[Global`x]],
        MathemeToTypst[Ceiling[Global`x]]
      },
      {"norm(x)", "norm(x)_2", "floor(x)", "ceil(x)"},
      TestID -> "Delimiters-Semantic"
    ],
    VerificationTest[
      {
        MathemeToTypst[Norm[Global`x], "DelimiterStyle" -> "Explicit"],
        MathemeToTypst[Floor[Global`x], "DelimiterStyle" -> "Explicit"],
        MathemeToTypst[Ceiling[Global`x], "DelimiterStyle" -> "Explicit"]
      },
      {"lr(bar.v.double x bar.v.double)", "lr(floor.l x floor.r)", "lr(ceil.l x ceil.r)"},
      TestID -> "Delimiters-Explicit"
    ],
    VerificationTest[
      MathemeToTypst[BracketingBar[Global`x]],
      "abs(x)",
      TestID -> "Delimiters-BracketingBar"
    ],

    (* --- Calculus, Derivatives, Limits, Integrals --- *)
    VerificationTest[
      MathemeToTypst[HoldForm[D[Global`f[Global`x, Global`y], {Global`x, 2}, Global`y]]],
      "frac(partial^3 f(x, y), partial x^2 thin partial y)",
      TestID -> "Calculus-PartialDerivative-ValidTypst"
    ],
    VerificationTest[
      MathemeToTypst[Derivative[1, 2][Global`f][Global`x, Global`y]],
      "f^((1, 2))(x, y)",
      TestID -> "Calculus-DerivativeOrderParensPreserved"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[Limit[1/Global`x, Global`x -> 0, Direction -> "FromAbove"]]],
      "lim_(x -> 0^+) frac(1, x)",
      TestID -> "Calculus-LimitFromAbove"
    ],
    VerificationTest[
      MathemeToTypst[Inactivate[Integrate[Global`x^2, {Global`x, 0, 1}]]],
      "integral_0^1 x^2 thin dif x",
      TestID -> "Calculus-InactivateIntegrate"
    ],
    VerificationTest[
      MathemeToTypst[HypergeometricPFQ[{Global`a, Global`b}, {Global`c}, Global`z]],
      "attach(F, bl: 2, br: 1)(a, b; c; z)",
      TestID -> "SpecialFn-HypergeometricPFQ-PreSubscript"
    ],

    (* --- Matrices, Det, Piecewise --- *)
    VerificationTest[
      MathemeToTypst[MatrixForm[{{Global`a, Global`b}, {Global`c, Global`d}}]],
      "mat(a, b; c, d)",
      TestID -> "Matrix-FoldedDelimiter"
    ],
    VerificationTest[
      MathemeToTypst[MatrixForm[{{Global`a, Global`b}, {Global`c, Global`d}}], "MatrixDelimiter" -> None],
      "(mat(delim: #none, a, b; c, d))",
      TestID -> "Matrix-UnfoldedDelimiter"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[Det[{{Global`a, Global`b}, {Global`c, Global`d}}]]],
      "mat(delim: \"|\", a, b; c, d)",
      TestID -> "Matrix-DetBarDelimiter"
    ],
    VerificationTest[
      MathemeToTypst[Piecewise[{{Global`x^2, Global`x > 0}, {-Global`x, Global`x < 0}}]],
      "cases(x^2 & x > 0, -x & x < 0)",
      TestID -> "Piecewise-Cases"
    ],

    (* --- Numbers, Quantities, ConditionalExpression, Prose, Styles --- *)
    VerificationTest[
      {MathemeToTypst[1.23*^15], MathemeToTypst[2.5*^-6]},
      {"1.23 times 10^(15)", "2.5 times 10^(-6)"},
      TestID -> "Numbers-ScientificNotation"
    ],
    VerificationTest[
      MathemeToTypst[Quantity[9.8, "Meters"/"Seconds"^2]],
      "9.8 thin \"m\"\\/\"s\"^2",
      TestID -> "Quantity-LiteralSlash"
    ],
    VerificationTest[
      MathemeToTypst[ConditionalExpression[Global`x^2, Global`x > 0]],
      "x^2 quad \"if\" quad x > 0",
      TestID -> "ConditionalExpression-ValidTypst"
    ],
    VerificationTest[
      MathemeToTypst[Row[{"hello ", Global`x + 1, " world"}]],
      "\"hello \" x + 1 \" world\"",
      TestID -> "Prose-RowStringsQuoted"
    ],
    VerificationTest[
      MathemeToTypst[Style[Global`x + Global`y, Bold, Red]],
      "text(fill: #rgb(255, 0, 0), bold(x + y))",
      TestID -> "Style-BoldRedPreserved"
    ],
    VerificationTest[
      MathemeToTypst[Style[Global`x + Global`y, Bold, Red], "PreserveStyle" -> False],
      "x + y",
      TestID -> "Style-PreserveStyleFalse"
    ],
    VerificationTest[
      MathemeToTypst[Overscript[Global`a + Global`b, "\[OverBrace]"]],
      "overbrace(a + b)",
      TestID -> "Overscript-QuotedOverBrace"
    ],

    (* --- PUA Zero-Leak Checks --- *)
    VerificationTest[
      And @@ (compilesCleanlyQ[MathemeToTypst[#]] & /@ {
        Abs[Global`x],
        Norm[Global`x, 2],
        MeijerG[{{Global`a, Global`b}, {Global`c}}, {{Global`d}, {Global`e, Global`f}}, Global`x],
        EllipticF[Global`phi, Global`m],
        HypergeometricPFQ[{Global`a, Global`b}, {Global`c}, Global`z],
        HoldForm[D[Global`f[Global`x, Global`y], {Global`x, 2}, Global`y]],
        Derivative[1, 2][Global`f][Global`x, Global`y],
        ConditionalExpression[Global`x^2, Global`x > 0],
        Inactivate[Integrate[Global`x^2, {Global`x, 0, 1}]],
        Quantity[9.8, "Meters"/"Seconds"^2],
        Style[Global`x + Global`y, Bold, Red],
        TwoWayRule[Global`a, Global`b],
        UndirectedEdge[Global`a, Global`b],
        DirectedEdge[Global`a, Global`b],
        <|"a" -> 1, "b" -> 2|>,
        \[FormalX]^2 + \[FormalAlpha],
        2.5*^-6,
        Row[{"hello ", Global`x + 1, " world"}],
        ForAll[Global`x, Element[Global`x, Reals], Global`x^2 >= 0],
        Piecewise[{{Global`x^2, Global`x > 0}, {-Global`x, Global`x <= 0}}],
        MatrixForm[{{Global`a/Global`b, Global`c}, {Global`d, Global`e}}]
      }),
      True,
      TestID -> "EndToEnd-AllExpressionsCompileCleanlyWithZeroPUALeak"
    ],

    (* --- Style Options --- *)
    VerificationTest[
      MathemeToTypst[Exp[I Global`x], "ConstantStyle" -> "Upright"],
      "upright(e)^(upright(i) x)",
      TestID -> "Option-ConstantStyle-Upright"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[Integrate[Global`x, Global`x]], "DifferentialStyle" -> "Italic"],
      "integral x thin d x",
      TestID -> "Option-DifferentialStyle-Italic"
    ],
    VerificationTest[
      MathemeToTypst[Global`a / Global`b, "FractionStyle" -> "Slash"],
      "(a)/(b)",
      TestID -> "Option-FractionStyle-Slash"
    ],
    VerificationTest[
      MathemeToTypst[Transpose[Global`A], "TransposeSymbol" -> "top"],
      "A^top",
      TestID -> "Option-TransposeSymbol"
    ],
    VerificationTest[
      MathemeToTypst[Conjugate[Global`z], "ConjugateStyle" -> "Overline"],
      "overline(z)",
      TestID -> "Option-ConjugateStyle-Overline"
    ],
    VerificationTest[
      MathemeToTypst[Element[Global`x, Reals], "BlackboardStyle" -> "Shorthand"],
      "x in RR",
      TestID -> "Option-BlackboardStyle-Shorthand"
    ],
    VerificationTest[
      MathemeToTypst[Global`\[ScriptCapitalL], "ScriptStyle" -> "Script"],
      "scr(L)",
      TestID -> "Option-ScriptStyle-Script"
    ],
    VerificationTest[
      {
        BoxesToTypst["MyPkg`Private`x", "StripContext" -> True],
        BoxesToTypst["MyPkg`Private`x", "StripContext" -> False]
      },
      {"x", "\"MyPkg`Private`x\""},
      TestID -> "Option-StripContext"
    ],
    VerificationTest[
      {
        MathemeToTypst[Global`foo, "MultiLetterStyle" -> "Quoted"],
        MathemeToTypst[Global`foo, "MultiLetterStyle" -> "Italic"],
        MathemeToTypst[Global`foo, "MultiLetterStyle" -> "Upright"]
      },
      {"\"foo\"", "italic(\"foo\")", "upright(\"foo\")"},
      TestID -> "Option-MultiLetterStyle"
    ],

    (* --- Customizability: TypstRaw, MakeTypst, Rules --- *)
    VerificationTest[
      MathemeToTypst[1 / (1 + TypstRaw["hat(H)"])],
      "frac(1, hat(H) + 1)",
      TestID -> "Custom-TypstRaw-PreservesPrecedence"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[symKet[Exp[I Global`x]] + symConst], "ConstantStyle" -> "Upright"],
      "lr(|upright(e)^(upright(i) x) chevron.r) + cal(C)",
      TestID -> "Custom-MakeTypst-UpValues-InheritsConfig"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[symBra[1 + 1]]],
      "lr(chevron.l 1 + 1|)",
      TestID -> "Custom-MakeTypst-HoldForm-TwoArgConv"
    ],
    VerificationTest[
      MathemeToTypst[
        HoldForm[Global`myOp[Global`a, Global`b] + Global`c],
        "ExpressionRules" -> {
          Global`myOp[u_, v_] :> "lr(chevron.l " <> MathemeToTypst[u] <> " mid(|) " <> MathemeToTypst[v] <> " chevron.r)"
        }
      ],
      "lr(chevron.l a mid(|) b chevron.r) + c",
      TestID -> "Custom-ExpressionRules-StringResult"
    ],
    VerificationTest[
      MathemeToTypst[
        Global`myWrap[Global`x],
        "ExpressionRules" -> {Global`myWrap[u_] :> Sqrt[u + 1]}
      ],
      "sqrt(x + 1)",
      TestID -> "Custom-ExpressionRules-ExprRewrite"
    ],
    VerificationTest[
      BoxesToTypst[
        FractionBox["a", "b"],
        "BoxRules" -> {FractionBox[n_, d_, ___] :> "myfrac(" <> BoxesToTypst[n] <> ", " <> BoxesToTypst[d] <> ")"}
      ],
      "myfrac(a, b)",
      TestID -> "Custom-BoxRules"
    ],
    VerificationTest[
      MathemeToTypst[
        Abs[Global`x],
        "TemplateRules" -> <|"Abs" -> Function[{args, conv}, "myabs(" <> conv[args[[1]]] <> ")"]|>
      ],
      "myabs(x)",
      TestID -> "Custom-TemplateRules-OverrideBuiltin"
    ],
    VerificationTest[
      BoxesToTypst[
        TemplateBox[{"x", "y"}, "CustomPair"],
        "TemplateRules" -> <|"CustomPair" -> StringTemplate["angle.l ``, `` angle.r"]|>
      ],
      "angle.l x, y angle.r",
      TestID -> "Custom-TemplateRules-StringTemplate"
    ],
    VerificationTest[
      MathemeToTypst[Exp[I Global`x], "CharacterRules" -> <|"\[ImaginaryI]" -> "bold(i)"|>],
      "e^(bold(i) x)",
      TestID -> "Custom-CharacterRules"
    ],
    VerificationTest[
      MathemeToTypst[
        Global`myArgMax[Global`x],
        "OperatorNames" -> {"myArgMax" -> "op(\"arg max\", limits: #true)"}
      ],
      "op(\"arg max\", limits: #true)(x)",
      TestID -> "Custom-OperatorNames"
    ],
    VerificationTest[
      Module[{u = FromCharacterCode[16^^e999]},
        {
          BoxesToTypst[u, "UnknownCharacterHandler" -> "Ignore"],
          BoxesToTypst[u, "UnknownCharacterHandler" -> "Unicode"],
          BoxesToTypst[u, "UnknownCharacterHandler" -> Function[c, "custom_char"]]
        }
      ],
      {"", "#\"\\u{E999}\"", "custom_char"},
      TestID -> "Custom-UnknownCharacterHandler"
    ],
    VerificationTest[
      BoxesToTypst[
        Global`UnknownBox[123],
        "UnsupportedBoxHandler" -> Function[b, "\"fallback\""]
      ],
      "\"fallback\"",
      TestID -> "Custom-UnsupportedBoxHandler"
    ],
    VerificationTest[
      Module[{orig = Options[TypstForm, "ConstantStyle"], res1, res2},
        SetOptions[TypstForm, "ConstantStyle" -> "Upright"];
        res1 = ToString[TypstForm[Exp[I Global`x]]];
        res2 = MathemeToGraphics[TypstForm[Exp[I Global`x]]];
        SetOptions[TypstForm, orig];
        res1 === "upright(e)^(upright(i) x)" && !FailureQ[res2] && Head[First[res2["Graphics"]]] === Graphics
      ],
      True,
      TestID -> "Custom-SetOptions-TypstForm-HonoredByCompile"
    ],
    VerificationTest[
      Module[{vals, src, res},
        vals = DeleteDuplicates@Select[
          StringTrim /@ Join[
            Values[Matheme`Private`TypstForm`$typstDefaultCharMap],
            Values[Matheme`Private`TypstForm`$typstDoubleStruckShorthands],
            Values[Matheme`Private`TypstForm`$typstScriptStyleLetters],
            Values[Matheme`Private`TypstForm`$typstDefaultOperatorMap]
          ],
          # =!= "" &
        ];
        vals = Map[
          If[MemberQ[{"(", ")", "[", "]", "{", "}"}, #], "\\" <> #, #] &,
          vals
        ];
        src = StringRiffle[("$ " <> # <> " $") & /@ vals, "\n"];
        res = Quiet[MathemeCompile[src, "Format" -> "Graphics"]];
        !FailureQ[res]
      ],
      True,
      TestID -> "Tables-AllCharAndOperatorMappingsCompileInTypst"
    ]
  };

  ClearAll[symKet, symBra, symConst];

  results = TestReport[tests];
  passed = results["TestsSucceededCount"];
  total = Length[tests];
  <|
    "AllPassed" -> (passed === total),
    "Succeeded" -> passed,
    "Failed" -> results["TestsFailedCount"],
    "Results" -> Association[
      Map[
        Function[id, results["TestResults"][id]["TestID"] -> results["TestResults"][id]["Outcome"]],
        Keys[results["TestResults"]]
      ]
    ],
    "Failures" -> Select[
      Values[results["TestResults"]],
      #["Outcome"] =!= "Success" &
    ]
  |>
];

End[];
EndPackage[];
