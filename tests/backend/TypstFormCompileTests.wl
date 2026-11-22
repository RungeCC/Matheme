(* ::Package:: *)
(* Typst compilation / Graphics rendering checks for the TypstForm conversion
   tables.  These call MathemeCompile / MathemeToGraphics, so they need the
   built matheme.dll and a working WSTP link.  Pure string-conversion tests
   live in ../wolfram/TypstFormTests.wl. *)

BeginPackage["Matheme`TypstFormCompileTests`", {"Matheme`"}];

RunMathemeTypstFormCompileTests::usage = "RunMathemeTypstFormCompileTests[] verifies that TypstForm output compiles to Graphics without PUA leakage.";

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

RunMathemeTypstFormCompileTests[] := Module[{tests, results, passed, total},
  tests = {
    VerificationTest[
      Module[{res = MathemeToGraphics[TypstForm[MeijerG[{{Global`a, Global`b, Global`c}, {Global`d, Global`e, Global`f}}, {{Global`a, Global`b, Global`c}, {Global`d, Global`e, Global`f}}, Global`x]]]},
        !FailureQ[res] && Head[First[res["Graphics"]]] === Graphics &&
          Length[Cases[First[res["Graphics"]], _FilledCurve, Infinity]] >= 15
      ],
      True,
      TestID -> "Regression-MeijerG-EndToEnd-Graphics"
    ],
    VerificationTest[
      Module[{ts = MathemeToTypst[FoxH[{{{Global`a, Global`A}}, {{Global`b, Global`B}}}, {{{Global`c, Global`c1}}, {{Global`d, Global`d1}}}, Global`z]]},
        !hasPuaQ[ts] && StringContainsQ[ts, "mid(|)"] && compilesCleanlyQ[ts]
      ],
      True,
      TestID -> "Regression-FoxH-AssociationTemplateAndMid"
    ],
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
    VerificationTest[
      Module[{orig = Options[TypstForm, "ConstantStyle"], res2},
        SetOptions[TypstForm, "ConstantStyle" -> "Upright"];
        res2 = MathemeToGraphics[TypstForm[Exp[I Global`x]]];
        SetOptions[TypstForm, orig];
        !FailureQ[res2] && Head[First[res2["Graphics"]]] === Graphics
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
