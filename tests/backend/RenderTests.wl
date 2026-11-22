(* ::Package:: *)
(* End-to-end rendering, sizing consistency, and regression suite for Matheme. *)

BeginPackage["Matheme`Tests`", {"Matheme`"}];

RunMathemeRenderTests::usage = "RunMathemeRenderTests[] executes the end-to-end Matheme rendering and sizing consistency test suite.";

Begin["`Private`"];

parseNumber[s_String] := Module[{st = StringToStream[s], val},
  val = Read[st, Number];
  Close[st];
  val
];

sizesConsistentQ[source_, template_] := Module[
  {formats = {"Graphics", "SVG", "PDF"}, recs, g, pr, pad, sz, dims, refSz, refDims},
  recs = Map[
    Function[fmt,
      g = First[MathemeToGraphics[source, "TypstTemplate" -> template, "Format" -> fmt]["Graphics"]];
      pr = PlotRange /. Options[g, PlotRange];
      pad = PlotRangePadding /. Options[g, PlotRangePadding];
      sz = ImageSize /. Options[g, ImageSize];
      dims = ImageDimensions[Rasterize[g]];
      <|"Graphics" -> g, "PlotRange" -> pr, "Padding" -> pad, "ImageSize" -> sz, "RasterDims" -> dims|>
    ],
    formats
  ];
  refSz = recs[[1, "ImageSize"]];
  refDims = recs[[1, "RasterDims"]];
  AllTrue[
    recs,
    Function[r,
      Head[r["Graphics"]] === Graphics &&
      MatrixQ[r["PlotRange"], NumericQ] &&
      Dimensions[r["PlotRange"]] === {2, 2} &&
      r["Padding"] === None &&
      ListQ[r["ImageSize"]] && Length[r["ImageSize"]] === 2 &&
      Max[Abs[r["ImageSize"] - refSz]] <= 0.5 &&
      Max[Abs[r["RasterDims"] - refDims]] <= 2
    ]
  ]
];

RunMathemeRenderTests[] := Module[{tests, results, passed, total},
  tests = {
    VerificationTest[
      Module[{res = MathemeCompile["x^y", "TypstTemplate" -> "Equation", "Format" -> "PDF"], img},
        img = First[ImportByteArray[res["PDF"], {"PDF", "PageImages"}]];
        ByteArrayQ[res["PDF"]] && ImageQ[img] && Min[ImageDimensions[img]] > 0
      ],
      True,
      TestID -> "PDF-Math-Rasterize-xy"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics["x^y", "TypstTemplate" -> "Equation"], g, curves},
        g = First[res["Graphics"]];
        curves = Cases[g, _FilledCurve, Infinity];
        Head[g] === Graphics && Length[curves] >= 2 && !KeyExistsQ[res, "SVG"] && !KeyExistsQ[res, "PDF"]
      ],
      True,
      TestID -> "ToGraphics-Native-Math-xy-FilledCurves"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics["\nxxyyzz\n", "TypstTemplate" -> "Text"], g, pr, pts, xs, ys},
        g = First[res["Graphics"]];
        pr = PlotRange /. Options[g, PlotRange];
        pts = Flatten[Cases[g, FilledCurve[_, p_] :> p, Infinity], 2];
        {xs, ys} = {MinMax[pts[[All, 1]]], MinMax[pts[[All, 2]]]};
        Head[g] === Graphics &&
          pr[[2, 1]] < -1.0 &&
          xs[[1]] >= pr[[1, 1]] - 10^-3 && xs[[2]] <= pr[[1, 2]] + 10^-3 &&
          ys[[1]] >= pr[[2, 1]] - 10^-3 && ys[[2]] <= pr[[2, 2]] + 10^-3
      ],
      True,
      TestID -> "ToGraphics-Native-Text-No-Descender-Clipping"
    ],
    VerificationTest[
      sizesConsistentQ["Hello World", None],
      True,
      TestID -> "ToGraphics-AllFormats-SizeConsistency-A4"
    ],
    VerificationTest[
      sizesConsistentQ["\nxxyyzz\n", "Text"],
      True,
      TestID -> "ToGraphics-AllFormats-SizeConsistency-Descenders"
    ],
    VerificationTest[
      sizesConsistentQ["integral_0^infinity e^(-x^2) dif x", "Equation"],
      True,
      TestID -> "ToGraphics-AllFormats-SizeConsistency-Equation"
    ],
    VerificationTest[
      Module[{src = "integral_0^infinity e^(-x^2) dif x", resG, resS, g, svg, vb, pr, vbW, vbH, prW, prH},
        resG = MathemeToGraphics[src, "TypstTemplate" -> "Equation", "Format" -> "SVG"];
        resS = MathemeCompile[src, "TypstTemplate" -> "Equation", "Format" -> "SVG"];
        g = First[resG["Graphics"]];
        svg = First[resS["SVG"]];
        vb = parseNumber /@ StringSplit[First[StringCases[svg, "viewBox=\"" ~~ v : Shortest[___] ~~ "\"" :> v]]];
        {vbW, vbH} = vb[[3 ;; 4]];
        pr = PlotRange /. Options[g, PlotRange];
        prW = Abs[pr[[1, 2]] - pr[[1, 1]]];
        prH = Abs[pr[[2, 2]] - pr[[2, 1]]];
        !KeyExistsQ[resG, "SVG"] && MatrixQ[pr, NumericQ] && prW >= vbW - 10^-4 && prH >= vbH - 10^-4
      ],
      True,
      TestID -> "ToGraphics-SVG-Covers-ViewBox"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics["#rect(width: 40pt, height: 25pt, stroke: 4pt + red)", "TypstTemplate" -> "Text"], g, pr},
        g = First[res["Graphics"]];
        pr = PlotRange /. Options[g, PlotRange];
        pr[[1, 1]] <= -1.9 && pr[[1, 2]] >= 41.9 && pr[[2, 1]] <= -1.9 && pr[[2, 2]] >= 26.9
      ],
      True,
      TestID -> "ToGraphics-Native-Stroke-BBox"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics["#box(width: 20pt, height: 15pt, clip: true, rect(width: 60pt, height: 60pt, fill: red))", "TypstTemplate" -> "Text"], g, sz},
        g = First[res["Graphics"]];
        sz = ImageSize /. Options[g, ImageSize];
        Abs[sz[[1]] - 20.0] < 1.0 && Abs[sz[[2]] - 15.0] < 1.0 && Length[Cases[g, _Inset, Infinity]] >= 1
      ],
      True,
      TestID -> "ToGraphics-Native-Clip-Box"
    ],
    VerificationTest[
      Module[{res, g, img, dims, px},
        res = MathemeToGraphics[
          "#pad(2pt, polygon(fill: rgb(\"#2563eb\"), fill-rule: \"non-zero\", (15pt, 0pt), (25pt, 30pt), (0pt, 12pt), (30pt, 12pt), (5pt, 30pt)))",
          "TypstTemplate" -> "Text"
        ];
        g = First[res["Graphics"]];
        img = Rasterize[g, ImageResolution -> 144];
        dims = ImageDimensions[img];
        (* Center of the pentagram must be filled (not white background). *)
        px = PixelValue[ColorConvert[img, "RGB"], {dims[[1]] / 2.0, dims[[2]] * 0.45}];
        Total[Abs[1.0 - px]] > 0.5
      ],
      True,
      TestID -> "ToGraphics-Native-NonZero-Star"
    ],
    VerificationTest[
      Module[{res},
        res = Quiet[
          MathemeToGraphics[
            "#rect(width: 20pt, height: 10pt, fill: gradient.linear(red, blue))",
            "TypstTemplate" -> "Text"
          ],
          {MathemeDiagnostic::warning}
        ];
        Length[Select[Lookup[res, "Diagnostics", {}], #["Severity"] === "Warning" &]] >= 1
      ],
      True,
      TestID -> "ToGraphics-Native-Gradient-Warning"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics[TypstForm[HoldForm[Integrate[Exp[-Global`x^2], {Global`x, 0, Infinity}]]]]},
        Head[First[res["Graphics"]]] === Graphics &&
          Length[Cases[First[res["Graphics"]], _FilledCurve, Infinity]] >= 5
      ],
      True,
      TestID -> "ToGraphics-TypstForm-Integral"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics[TypstForm[{{1, 2}, {3, Global`a + Global`b}}]]},
        Head[First[res["Graphics"]]] === Graphics
      ],
      True,
      TestID -> "ToGraphics-TypstForm-Matrix"
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
    VerificationTest[
      Module[{src = "Page 1\n#pagebreak()\nPage 2\n#pagebreak()\nPage 3", rG, rS, rSC, rP},
        rG = MathemeToGraphics[src, "Pages" -> 2];
        rS = MathemeToGraphics[src, "Format" -> "SVG", "Pages" -> {1, 3}];
        rSC = MathemeCompile[src, "Format" -> "SVG", "Pages" -> {1, 3}];
        rP = MathemeCompile[src, "Format" -> "PDF", "Pages" -> 2, "PDFTags" -> False];
        rG["Pages"] === 1 && rG["TotalPages"] === 3 && Length[rG["Graphics"]] === 1 &&
          rS["Pages"] === 2 && rS["TotalPages"] === 3 && !KeyExistsQ[rS, "SVG"] && Length[rS["Graphics"]] === 2 &&
          rSC["Pages"] === 2 && rSC["TotalPages"] === 3 && Length[rSC["SVG"]] === 2 &&
          rP["Pages"] === 1 && rP["TotalPages"] === 3
      ],
      True,
      TestID -> "Pages-Filter-Exported-And-Total-Count-Consistency"
    ],
    VerificationTest[
      Module[{rC, rG, rErr},
        rC = MathemeCompile["x^2", "TypstTemplate" -> "Equation", "Format" -> "PDF"];
        rG = MathemeToGraphics["x^2", "TypstTemplate" -> "Equation"];
        rErr = MathemeCompile["#let x ="];
        !KeyExistsQ[rC, "Formatted"] && !KeyExistsQ[rC, "FormattedCell"] &&
          !KeyExistsQ[rG, "Formatted"] && !KeyExistsQ[rG, "FormattedCell"] &&
          FailureQ[rErr] && StringQ[rErr["Formatted"]] && StringLength[rErr["Formatted"]] > 0
      ],
      True,
      TestID -> "Diagnostics-Omit-Formatted-When-Clean"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics["= Hello", "Format" -> "HTML"]},
        FailureQ[res] && res[[1]] === MathemeCallError
      ],
      True,
      TestID -> "ToGraphics-Rejects-HTML-Format"
    ],
    VerificationTest[
      Module[{res = MathemeCompile["= Hello", "Format" -> "HTML", "Pages" -> {1}]},
        FailureQ[res] && res[[1]] === MathemeCompileError
      ],
      True,
      TestID -> "Compile-HTML-Rejects-Pages-Option"
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
    ]
  |>
];

End[];
EndPackage[];
