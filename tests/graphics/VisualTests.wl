(* ::Package:: *)
(* Visual equivalence verification suite for Matheme's native Typst Frame -> WL Graphics exporter.
   Compares rasterized output of native "Format" -> "Graphics" against Wolfram's SVG importer
   ground truth across math, text (including descenders), and vector shapes. *)

Begin["Matheme`VisualTests`"];

compareVisualCase[name_String, source_, template_String : "Equation"] := Module[
  {resNat, resSvg, gNat, gSvg, imgNat, imgSvg, cropNat, cropSvg, dims,
   dNat, dSvg, mae, rmse, inkNat, inkSvg, inter, union, iou, arNat, arSvg, arDiff, passed},

  resNat = If[Head[source] === Matheme`TypstForm,
    Matheme`MathemeToGraphics[source, "Format" -> "Graphics"],
    Matheme`MathemeToGraphics[source, "TypstTemplate" -> template, "Format" -> "Graphics"]
  ];
  resSvg = If[Head[source] === Matheme`TypstForm,
    Matheme`MathemeToGraphics[source, "Format" -> "SVG"],
    Matheme`MathemeToGraphics[source, "TypstTemplate" -> template, "Format" -> "SVG"]
  ];

  gNat = First[resNat["Graphics"]];
  gSvg = First[resSvg["Graphics"]];

  imgNat = Rasterize[Show[gNat, PlotRange -> All, PlotRangePadding -> Scaled[0.02]], ImageSize -> 400];
  imgSvg = Rasterize[Show[gSvg, PlotRange -> All, PlotRangePadding -> Scaled[0.02]], ImageSize -> 400];

  cropNat = ImageCrop[imgNat];
  cropSvg = ImageCrop[imgSvg];

  arNat = Divide @@ N[ImageDimensions[cropNat]];
  arSvg = Divide @@ N[ImageDimensions[cropSvg]];
  arDiff = Abs[arNat - arSvg] / Max[1.0, arSvg];

  dims = ImageDimensions[cropSvg];
  cropNat = ImageResize[cropNat, dims];

  dNat = ImageData[ColorConvert[cropNat, "RGB"]];
  dSvg = ImageData[ColorConvert[cropSvg, "RGB"]];
  mae = N[Mean[Flatten[Abs[dNat - dSvg]]]];
  rmse = N[Sqrt[Mean[Flatten[(dNat - dSvg)^2]]]];

  (* Foreground mask IoU: pixels differing from white (1,1,1) *)
  inkNat = UnitStep[Total[Abs[1.0 - dNat], {3}] - 0.1];
  inkSvg = UnitStep[Total[Abs[1.0 - dSvg], {3}] - 0.1];
  inter = Total[inkNat * inkSvg, 2];
  union = Total[UnitStep[inkNat + inkSvg - 0.5], 2];
  iou = If[union == 0, 0.0, N[inter / union]];

  passed = TrueQ[
    Total[inkNat, 2] > 100 &&
    Total[inkSvg, 2] > 100 &&
    mae < 0.02 &&
    iou >= 0.94 &&
    arDiff < 0.04
  ];

  <|
    "Name" -> name,
    "Passed" -> passed,
    "Dimensions" -> dims,
    "InkPixelsNat" -> Total[inkNat, 2],
    "InkPixelsSvg" -> Total[inkSvg, 2],
    "MAE" -> mae,
    "RMSE" -> rmse,
    "InkIoU" -> iou,
    "AspectRatioDiff" -> arDiff
  |>
];

$VisualTestCases = {
  {"Visual-Math-Power", "x^y", "Equation"},
  {"Visual-Math-Fraction", "frac(a + 1, b^2 - 1)", "Equation"},
  {"Visual-Math-Integral", "integral_0^infinity e^(-x^2) dif x = sqrt(pi)/2", "Equation"},
  {"Visual-Math-Matrix", "mat(1, 2, 3; 4, 5, 6; 7, 8, x + y)", "Equation"},
  {"Visual-Math-SumProd", "sum_(n=1)^infinity 1/n^2 = pi^2/6, quad product_(k=1)^n k = n!", "Equation"},
  {"Visual-Text-Basic", "Hello, Wolfram & Typst!", "Text"},
  {"Visual-Text-Descenders", "xxyyzz gjpqy", "Text"},
  {"Visual-Shapes-Rect", "#rect(width: 40pt, height: 25pt, fill: rgb(\"#3b82f6\"), stroke: 2pt + rgb(\"#1d4ed8\"), radius: 4pt)", "Text"},
  {"Visual-Shapes-CircleLine", "#stack(dir: ltr, spacing: 6pt, circle(radius: 10pt, fill: rgb(\"#ef4444\"), stroke: 1.5pt + black), line(length: 25pt, angle: 30deg, stroke: 2pt + rgb(\"#10b981\")))", "Text"}
};

RunMathemeVisualTests[] := Module[{rows, tests, report, passedCount, totalCount},
  rows = Map[Apply[compareVisualCase], $VisualTestCases];
  tests = Map[
    Function[row,
      VerificationTest[
        row["Passed"],
        True,
        TestID -> row["Name"]
      ]
    ],
    rows
  ];
  report = TestReport[tests];
  passedCount = report["TestsSucceededCount"];
  totalCount = Length[tests];
  <|
    "AllPassed" -> (passedCount === totalCount),
    "Succeeded" -> passedCount,
    "Failed" -> report["TestsFailedCount"],
    "Metrics" -> rows
  |>
];

End[];
