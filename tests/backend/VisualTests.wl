(* ::Package:: *)
(* Backend suite (needs matheme.dll / WSTP).  Visual equivalence verification
   for Matheme's native Typst Frame -> WL Graphics exporter.
   Compares both uncropped absolute-scale rasterization (144 DPI) and cropped high-resolution
   rasterization against ground truth ("SVG" via Wolfram's SVG importer for standard text/math/
   shapes, and "PDFImage" via Typst PDF rasterization for features where Wolfram's SVG importer
   itself is broken, such as clip paths and non-zero winding fills). *)

BeginPackage["Matheme`VisualTests`", {"Matheme`"}];

RunMathemeVisualTests::usage = "RunMathemeVisualTests[opts] runs rasterized visual equivalence tests across offline (and optional network) cases.";
$VisualTestCases::usage = "$VisualTestCases lists the offline visual equivalence test cases.";
$NetworkVisualTestCases::usage = "$NetworkVisualTestCases lists visual equivalence test cases that require package downloads.";

Begin["`Private`"];

compareMetrics[name_String, absNat_Image, absSvg_Image, imgNat_Image, imgSvg_Image, gNat_Graphics, refByteCount_Integer, tNat_Real, tRef_Real] := Module[
  {absDimsNat, absDimsSvg, absSizeDiff, dAbsNat, dAbsSvg, absMae,
   cropNat, cropSvg, dims, dNat, dSvg, mae, rmse,
   inkNat, inkSvg, inter, union, iou, arNat, arSvg, arDiff, passed},

  (* 1. Uncropped absolute-scale comparison (catches PlotRange / ImageSize / offset regressions) *)
  absDimsNat = ImageDimensions[absNat];
  absDimsSvg = ImageDimensions[absSvg];
  absSizeDiff = Max[Abs[absDimsNat - absDimsSvg]];
  dAbsNat = ImageData[ColorConvert[ImageResize[absNat, absDimsSvg], "RGB"]];
  dAbsSvg = ImageData[ColorConvert[absSvg, "RGB"]];
  absMae = N[Mean[Flatten[Abs[dAbsNat - dAbsSvg]]]];

  (* 2. Cropped high-resolution comparison (catches glyph shape, fill, stroke, and color fidelity) *)
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

  inkNat = UnitStep[Total[Abs[1.0 - dNat], {3}] - 0.1];
  inkSvg = UnitStep[Total[Abs[1.0 - dSvg], {3}] - 0.1];
  inter = Total[inkNat * inkSvg, 2];
  union = Total[UnitStep[inkNat + inkSvg - 0.5], 2];
  iou = If[union == 0, 0.0, N[inter / union]];

  passed = TrueQ[
    Total[inkNat, 2] > 100 &&
    Total[inkSvg, 2] > 100 &&
    absSizeDiff <= 2 &&
    absMae < 0.025 &&
    mae < 0.02 &&
    iou >= 0.97 &&
    arDiff < 0.04
  ];

  <|
    "Name" -> name,
    "Passed" -> passed,
    "AbsDimsNat" -> absDimsNat,
    "AbsDimsSvg" -> absDimsSvg,
    "AbsSizeDiff" -> absSizeDiff,
    "AbsMAE" -> absMae,
    "Dimensions" -> dims,
    "NativeMs" -> N[tNat * 1000.0],
    "RefMs" -> N[tRef * 1000.0],
    "NativeByteCount" -> ByteCount[gNat],
    "RefByteCount" -> refByteCount,
    "InkPixelsNat" -> Total[inkNat, 2],
    "InkPixelsRef" -> Total[inkSvg, 2],
    "MAE" -> mae,
    "RMSE" -> rmse,
    "InkIoU" -> iou,
    "AspectRatioDiff" -> arDiff
  |>
];

compareVisualCase[name_String, source_, template_ : "Equation", allowDownloads_ : False, oracle_String : "SVG"] := Module[
  {resNat, resRef, tNat, tRef, gNats, gRefs, absRefs, hiRefs, count, pdfBytes, pageW, hiDpi},
  {tNat, resNat} = AbsoluteTiming[
    MathemeToGraphics[source, "TypstTemplate" -> template, "Format" -> "Graphics", "AllowDownloads" -> allowDownloads]
  ];
  If[oracle === "PDFImage",
    {tRef, resRef} = AbsoluteTiming[
      MathemeCompile[source, "TypstTemplate" -> template, "Format" -> "PDF", "AllowDownloads" -> allowDownloads]
    ];
    If[FailureQ[resNat] || FailureQ[resRef],
      Return[{<|"Name" -> name, "Passed" -> False, "Error" -> {resNat, resRef}|>}]
    ];
    gNats = resNat["Graphics"];
    If[!ListQ[gNats] || Length[gNats] == 0,
      Return[{<|"Name" -> name, "Passed" -> False, "PageCountMismatch" -> {Length[gNats], "Unknown"}|>}]
    ];
    pageW = Max[1.0, First[ImageSize /. Options[First[gNats], ImageSize]]];
    hiDpi = Min[1440, Max[288, Round[72.0 * 600.0 / pageW]]];
    pdfBytes = resRef["PDF"];
    absRefs = Map[RemoveAlphaChannel[#, White] &, ImportByteArray[pdfBytes, {"PDF", "PageImages"}, ImageResolution -> 144]];
    hiRefs = Map[RemoveAlphaChannel[#, White] &, ImportByteArray[pdfBytes, {"PDF", "PageImages"}, ImageResolution -> hiDpi]];
    If[Length[gNats] =!= Length[absRefs],
      Return[{<|"Name" -> name, "Passed" -> False, "PageCountMismatch" -> {Length[gNats], Length[absRefs]}|>}]
    ];
    count = Length[gNats];
    Return[Table[
      With[{label = If[count == 1, name, name <> "-Page" <> IntegerString[idx]]},
        compareMetrics[
          label,
          Rasterize[gNats[[idx]], ImageResolution -> 144],
          absRefs[[idx]],
          Rasterize[gNats[[idx]], ImageResolution -> hiDpi],
          hiRefs[[idx]],
          gNats[[idx]],
          ByteCount[pdfBytes],
          tNat / count,
          tRef / count
        ]
      ],
      {idx, 1, count}
    ]];
  ];

  (* Default "SVG" oracle *)
  {tRef, resRef} = AbsoluteTiming[
    MathemeToGraphics[source, "TypstTemplate" -> template, "Format" -> "SVG", "AllowDownloads" -> allowDownloads]
  ];
  If[FailureQ[resNat] || FailureQ[resRef],
    Return[{<|"Name" -> name, "Passed" -> False, "Error" -> {resNat, resRef}|>}]
  ];
  gNats = resNat["Graphics"];
  gRefs = resRef["Graphics"];
  If[!ListQ[gNats] || !ListQ[gRefs] || Length[gNats] =!= Length[gRefs] || Length[gNats] == 0,
    Return[{<|"Name" -> name, "Passed" -> False, "PageCountMismatch" -> {Length[gNats], Length[gRefs]}|>}]
  ];
  count = Length[gNats];
  Table[
    With[{label = If[count == 1, name, name <> "-Page" <> IntegerString[idx]]},
      compareMetrics[
        label,
        Rasterize[gNats[[idx]], ImageResolution -> 144],
        Rasterize[gRefs[[idx]], ImageResolution -> 144],
        Rasterize[Show[gNats[[idx]], PlotRange -> All, PlotRangePadding -> Scaled[0.02]], ImageSize -> 400],
        Rasterize[Show[gRefs[[idx]], PlotRange -> All, PlotRangePadding -> Scaled[0.02]], ImageSize -> 400],
        gNats[[idx]],
        ByteCount[gRefs[[idx]]],
        tNat / count,
        tRef / count
      ]
    ],
    {idx, 1, count}
  ]
];

$VisualTestCases = {
  {"Visual-Math-Power", "x^y", "Equation", False, "SVG"},
  {"Visual-Math-Fraction", "frac(a + 1, b^2 - 1)", "Equation", False, "SVG"},
  {"Visual-Math-Integral", "integral_0^infinity e^(-x^2) dif x = sqrt(pi)/2", "Equation", False, "SVG"},
  {"Visual-Math-Matrix", "mat(1, 2, 3; 4, 5, 6; 7, 8, x + y)", "Equation", False, "SVG"},
  {"Visual-Math-SumProd", "sum_(n=1)^infinity 1/n^2 = pi^2/6, quad product_(k=1)^n k = n!", "Equation", False, "SVG"},
  {"Visual-Text-Basic", "Hello, Wolfram & Typst!", "Text", False, "SVG"},
  {"Visual-Text-Descenders", "xxyyzz gjpqy", "Text", False, "SVG"},
  {"Visual-Text-Lorem", "#block(width: 240pt, lorem(80))", "Text", False, "SVG"},
  (* \:xxxx escapes keep this file pure ASCII: Get[] reads it with
     $CharacterEncoding == "CP936" on Windows, which garbles literal CJK. *)
  {"Visual-Text-Chinese-Offline", "\:6c38\:548c\:4e5d\:5e74\:ff0c\:5c81\:5728\:7678\:4e11\:ff0c\:66ae\:6625\:4e4b\:521d\:ff0c\:4f1a\:4e8e\:4f1a\:7a3d\:5c71\:9634\:4e4b\:5170\:4ead\:3002", "Text", False, "SVG"},
  {"Visual-Clip-Box", "#box(width: 24pt, height: 20pt, clip: true, rect(width: 60pt, height: 60pt, fill: red))", "Text", False, "PDFImage"},
  {"Visual-NonZero-Star", "#pad(2pt, polygon(fill: rgb(\"#2563eb\"), fill-rule: \"non-zero\", (15pt, 0pt), (25pt, 30pt), (0pt, 12pt), (30pt, 12pt), (5pt, 30pt)))", "Text", False, "PDFImage"},
  {"Visual-NonZero-Overlap", "#pad(2pt, curve(fill: rgb(\"#dc2626\"), fill-rule: \"non-zero\", curve.move((0pt, 0pt)), curve.line((20pt, 0pt)), curve.line((20pt, 20pt)), curve.line((0pt, 20pt)), curve.close(), curve.move((10pt, 10pt)), curve.line((30pt, 10pt)), curve.line((30pt, 30pt)), curve.line((10pt, 30pt)), curve.close()))", "Text", False, "PDFImage"},
  {"Visual-Text-Stroked", "#pad(2pt, text(size: 18pt, fill: rgb(\"#facc15\"), stroke: 0.8pt + black)[Hi])", "Text", False, "SVG"},
  {"Visual-Transform-Rotated", "#pad(2pt, rotate(25deg, reflow: true, rect(width: 30pt, height: 16pt, fill: rgb(\"#16a34a\"), stroke: 1pt + black)))", "Text", False, "SVG"},
  {"Visual-MultiPage", "#set page(width: 100pt, height: 60pt, margin: 8pt, fill: white)\nPage 1 $x^2$\n#pagebreak()\nPage 2 $y^3$", None, False, "SVG"},
  {"Visual-Shapes-Rect", "#pad(2pt, rect(width: 40pt, height: 25pt, fill: rgb(\"#3b82f6\"), stroke: 2pt + rgb(\"#1d4ed8\"), radius: 4pt))", "Text", False, "SVG"},
  {"Visual-Shapes-CircleLine", "#pad(2pt, stack(dir: ltr, spacing: 6pt, circle(radius: 10pt, fill: rgb(\"#ef4444\"), stroke: 1.5pt + black), line(length: 25pt, angle: 30deg, stroke: 2pt + rgb(\"#10b981\"))))", "Text", False, "SVG"},
  {"Visual-Gradient-Linear", "#set page(width: 100pt, height: 50pt, margin: 5pt, fill: white)\n#rect(width: 90pt, height: 40pt, fill: gradient.linear(red, yellow, blue, angle: 45deg))", None, False, "PDFImage"},
  {"Visual-Gradient-Parent", "#set page(width: 100pt, height: 60pt, margin: 5pt, fill: white)\n#let g = gradient.linear(red, blue, relative: \"parent\")\n#stack(dir: ltr, spacing: 10pt, rect(width: 40pt, height: 50pt, fill: g), rect(width: 40pt, height: 50pt, fill: g))", None, False, "PDFImage"},
  {"Visual-Gradient-TextStroke", "#set page(width: 120pt, height: 55pt, margin: 5pt, fill: white)\n#stack(dir: ttb, spacing: 4pt, text(size: 18pt, weight: \"bold\", fill: gradient.linear(red, blue))[Matheme], rect(width: 90pt, height: 18pt, stroke: 3pt + gradient.linear(red, blue)))", None, False, "PDFImage"},
  {"Visual-Gradient-RadialFocal", "#set page(width: 90pt, height: 90pt, margin: 5pt, fill: white)\n#circle(radius: 40pt, fill: gradient.radial(white, red, black, focal-center: (25%, 30%), focal-radius: 5%))", None, False, "PDFImage"},
  {"Visual-Gradient-Conic", "#set page(width: 90pt, height: 90pt, margin: 5pt, fill: white)\n#circle(radius: 40pt, fill: gradient.conic(red, green, blue, yellow, red, angle: 45deg))", None, False, "PDFImage"},
  {"Visual-Tiling-OffsetRotated", "#set page(width: 100pt, height: 90pt, margin: 12pt, fill: white)\n#let p = tiling(size: (16pt, 16pt), spacing: (4pt, 4pt), offset: (50%, 25%), { place(rect(width: 16pt, height: 16pt, fill: rgb(\"#3b82f6\"))); place(dx: 2pt, dy: 2pt, circle(radius: 6pt, fill: rgb(\"#22c55e\"))) })\n#rotate(20deg, rect(width: 60pt, height: 45pt, fill: p, stroke: 2pt + black))", None, False, "PDFImage"},
  {"Visual-Tiling-ConicWideRect", "#set page(width: 240pt, height: 100pt, margin: 10pt, fill: white)\n#let pat = tiling(size: (30pt, 30pt), spacing: (10pt, 20pt), square(size: 30pt, fill: gradient.conic(..color.map.rainbow)))\n#rect(width: 100%, height: 80pt, fill: pat, stroke: (thickness: 1pt, dash: \"dotted\"))", None, False, "PDFImage"},
  {"Visual-DashPhase", "#set page(width: 110pt, height: 55pt, margin: 6pt, fill: white)\n#stack(dir: ttb, spacing: 8pt, line(start: (0pt, 0pt), end: (95pt, 0pt), stroke: (paint: black, thickness: 4pt, cap: \"round\", dash: (array: (12pt, 8pt), phase: 6pt))), rect(width: 90pt, height: 28pt, stroke: (paint: blue, thickness: 3pt, dash: (array: (10pt, 6pt), phase: 5pt))))", None, False, "PDFImage"},
  {"Visual-Clip-NonRectNested", "#set page(width: 100pt, height: 100pt, margin: 10pt, fill: white)\n#box(width: 80pt, height: 80pt, radius: 28pt, clip: true)[#place(rect(width: 80pt, height: 40pt, fill: blue))#place(dy: 40pt, rect(width: 80pt, height: 40pt, fill: red))#place(dx: 15pt, dy: 15pt, box(width: 50pt, height: 50pt, radius: 25pt, clip: true, rect(width: 60pt, height: 60pt, fill: gradient.linear(yellow, orange))))]", None, False, "PDFImage"},
  {"Visual-EmbeddedSVG-Advanced", "#set page(width: 120pt, height: 120pt, margin: 0pt, fill: white)\n#image(bytes(\"<svg xmlns=\\\"http://www.w3.org/2000/svg\\\" width=\\\"120\\\" height=\\\"120\\\"><defs><linearGradient id=\\\"lg\\\" x1=\\\"0%\\\" y1=\\\"0%\\\" x2=\\\"100%\\\" y2=\\\"100%\\\"><stop offset=\\\"0%\\\" stop-color=\\\"#ef4444\\\"/><stop offset=\\\"100%\\\" stop-color=\\\"#3b82f6\\\"/></linearGradient><radialGradient id=\\\"rg\\\" cx=\\\"50%\\\" cy=\\\"50%\\\" r=\\\"50%\\\"><stop offset=\\\"0%\\\" stop-color=\\\"#fde047\\\"/><stop offset=\\\"100%\\\" stop-color=\\\"#16a34a\\\"/></radialGradient><pattern id=\\\"pat\\\" width=\\\"12\\\" height=\\\"12\\\" patternUnits=\\\"userSpaceOnUse\\\"><circle cx=\\\"6\\\" cy=\\\"6\\\" r=\\\"3.5\\\" fill=\\\"#7c3aed\\\"/></pattern><clipPath id=\\\"cp\\\"><circle cx=\\\"90\\\" cy=\\\"90\\\" r=\\\"22\\\"/></clipPath></defs><rect x=\\\"5\\\" y=\\\"5\\\" width=\\\"50\\\" height=\\\"50\\\" fill=\\\"url(#lg)\\\"/><circle cx=\\\"90\\\" cy=\\\"30\\\" r=\\\"22\\\" fill=\\\"url(#rg)\\\"/><rect x=\\\"5\\\" y=\\\"65\\\" width=\\\"50\\\" height=\\\"50\\\" fill=\\\"url(#pat)\\\" stroke=\\\"#111827\\\" stroke-width=\\\"2\\\" stroke-dasharray=\\\"8,4\\\" stroke-dashoffset=\\\"3\\\"/><g clip-path=\\\"url(#cp)\\\"><rect x=\\\"65\\\" y=\\\"65\\\" width=\\\"50\\\" height=\\\"25\\\" fill=\\\"#0ea5e9\\\"/><rect x=\\\"65\\\" y=\\\"90\\\" width=\\\"50\\\" height=\\\"25\\\" fill=\\\"#f97316\\\"/></g></svg>\"), format: \"svg\", width: 120pt, height: 120pt)", None, False, "PDFImage"}
};

$NetworkVisualTestCases = {
  {"Visual-Text-Chinese-Kouhu", "#import \"@preview/kouhu:0.2.0\": kouhu\n#block(width: 240pt, kouhu(length: 120))", "Text", True, "SVG"}
};

Options[RunMathemeVisualTests] = {
  "IncludeNetworkTests" -> False
};

RunMathemeVisualTests[OptionsPattern[]] := Module[{cases, rows, tests, report, passedCount, totalCount},
  cases = If[TrueQ[OptionValue["IncludeNetworkTests"]],
    Join[$VisualTestCases, $NetworkVisualTestCases],
    $VisualTestCases
  ];
  rows = Flatten[Map[Apply[compareVisualCase], cases], 1];
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
EndPackage[];
