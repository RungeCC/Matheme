(* ::Package:: *)
(* Correctness and customization suite for the Wolfram-only conversion layer:
   MathemeToTypst / BoxesToTypst / TypstForm.  No matheme.dll or WSTP call is
   made here -- loading Get["Runge`Matheme`"] is enough.  Tests that compile Typst
   or render Graphics live in ../backend/TypstFormCompileTests.wl. *)

BeginPackage["Runge`Matheme`TypstFormTests`", {"Runge`Matheme`"}];

RunMathemeTypstFormTests::usage = "RunMathemeTypstFormTests[] executes the TypstForm correctness and customization test suite (Wolfram-only, no backend).";

Begin["`Private`"];

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
      MathemeToTypst[EllipticF[Global`phi, Global`m]],
      "F(phi mid(|) m)",
      TestID -> "Regression-EllipticF-VerticalSeparator"
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
      Module[{orig = Options[TypstForm, "ConstantStyle"], res1},
        SetOptions[TypstForm, "ConstantStyle" -> "Upright"];
        res1 = ToString[TypstForm[Exp[I Global`x]]];
        SetOptions[TypstForm, orig];
        res1 === "upright(e)^(upright(i) x)"
      ],
      True,
      TestID -> "Custom-SetOptions-TypstForm-ToString"
    ],
    VerificationTest[
      ToString[TypstForm[Global`x^2 + 1/Global`y]],
      "x^2 + frac(1, y)",
      TestID -> "TypstForm-ToString"
    ],
    VerificationTest[
      BoxesToTypst[FractionBox["a", SuperscriptBox["b", "2"]]],
      "frac(a, b^2)",
      TestID -> "BoxesToTypst-Fraction"
    ],
    (* Inactive heads box as TemplateBox[..., "InactiveHead"]; the payload is
       either a bare name or the typeset head of an inverse function. *)
    VerificationTest[
      MathemeToTypst[Inactive[Sin][Global`x]],
      "sin(x)",
      TestID -> "Inactive-Head-Plain"
    ],
    VerificationTest[
      MathemeToTypst[Inactive[ArcSin][Global`x]],
      "sin^(-1)(x)",
      TestID -> "Inactive-Head-Superscript"
    ],
    VerificationTest[
      MathemeToTypst[Inactive[f][Global`a, Global`b]],
      "f[a, b]",
      TestID -> "Inactive-Head-Brackets"
    ],
    (* Held special functions box as semantic TemplateBox tags, resolved through
       the generated `TemplateBoxToDisplayBoxes` table rather than a whitelist. *)
    VerificationTest[
      MathemeToTypst[HoldForm[BesselY[2, Global`x]]],
      "Y_2(x)",
      TestID -> "Template-HoldForm-BesselY"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[EllipticK[Global`m]]],
      "K(m)",
      TestID -> "Template-HoldForm-EllipticK"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[PolyLog[2, Global`z]]],
      "op(\"Li\")_2(z)",
      TestID -> "Template-HoldForm-PolyLog"
    ],
    VerificationTest[
      MathemeToTypst[HoldForm[Inverse[{{Global`a, Global`b}, {Global`c, Global`d}}]]],
      "mat(a, b; c, d)^(-1)",
      TestID -> "Template-HoldForm-Inverse"
    ],

    (* --- TypstTemplate: native string slots, distinct from StringTemplate --- *)
    VerificationTest[
      BoxesToTypst[TemplateBox[{"x", "y"}, "MyTag"],
        "TemplateRules" -> {"MyTag" -> TypstTemplate["f({1}, {2})"]}],
      "f(x, y)",
      TestID -> "TypstTemplate-Fixed-Slots"
    ],
    VerificationTest[
      BoxesToTypst[TemplateBox[{"x", "y", "z"}, "MyTag"],
        "TemplateRules" -> {"MyTag" -> TypstTemplate["row({1}; {rest})", "; "]}],
      "row(x; y; z)",
      TestID -> "TypstTemplate-Rest-Slot"
    ],
    VerificationTest[
      BoxesToTypst[TemplateBox[{"x"}, "MyTag"],
        "TemplateRules" -> {"MyTag" -> StringTemplate["{{1}}"]}],
      "{{1}}",
      TestID -> "TypstTemplate-Wolfram-Template-Is-Distinct"
    ],

    (* --- Native config patch (R1 P0): fail-closed, generated capability --- *)
    VerificationTest[
      Runge`Matheme`Private`TypstForm`nativeConfigPatch[
        Association[Runge`Matheme`Private`TypstForm`$typstBoxOptionDefaults]
      ],
      <|
        "Version" -> 2,
        "CharOverrides" -> <||>,
        "OperatorOverrides" -> <||>,
        "TemplateOverrides" -> <||>,
        "Switches" -> <||>
      |>,
      TestID -> "NativeConfig-Default-Patch-Is-Empty"
    ],
    VerificationTest[
      Head[Runge`Matheme`Private`TypstForm`nativeConfigPatch[
        Association[Append[
          Runge`Matheme`Private`TypstForm`$typstBoxOptionDefaults,
          "BoxRules" -> {FractionBox[n_, d_] :> "myfrac(n, d)"}
        ]]
      ]],
      Failure,
      TestID -> "NativeConfig-BoxRules-Is-Code-Plane"
    ],
    VerificationTest[
      (Runge`Matheme`Private`TypstForm`nativeConfigPatch[
        Association[Append[
          Runge`Matheme`Private`TypstForm`$typstBoxOptionDefaults,
          "FractionStlye" -> "Slash"
        ]]
      ][[2]])["Field"],
      "FractionStlye",
      TestID -> "NativeConfig-Typo-Reports-Field"
    ],
    VerificationTest[
      KeyExistsQ[
        Lookup[
          Runge`Matheme`Private`TypstForm`nativeConfigPatch[
            Association[Append[
              Runge`Matheme`Private`TypstForm`$typstBoxOptionDefaults,
              "FractionStyle" -> "Slash"
            ]]
          ],
          "Switches"
        ],
        "FractionStyle"
      ],
      True,
      TestID -> "NativeConfig-NonDefault-Switch-Is-Patched"
    ],
    (* The generated schema (single source: matheme_box::OPTIONS) and the
       hand-written WL defaults must describe exactly the same option set;
       a drift here would let an option exist without a native plane or
       without a default. *)
    VerificationTest[
      Sort[Keys[Runge`Matheme`Private`TypstForm`$typstNativeOptionSchema]],
      Sort[Keys[Association[Runge`Matheme`Private`TypstForm`$typstExprOptionDefaults]]],
      TestID -> "NativeConfig-Schema-Covers-Every-Option"
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
