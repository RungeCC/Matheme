(* ::Package:: *)
(* End-to-end rendering, sizing consistency, and regression suite for Matheme.
   Every case here calls MathemeCompile / MathemeToGraphics and needs the
   built matheme.dll.  Wolfram-only conversion tests live in
   ../wolfram/TypstFormTests.wl; visual equivalence in ./VisualTests.wl. *)

BeginPackage["Runge`Matheme`Tests`", {"Runge`Matheme`"}];

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
      Module[{res, g},
        res = MathemeToGraphics[
          "#rect(width: 20pt, height: 10pt, fill: gradient.linear(red, blue))",
          "TypstTemplate" -> "Text"
        ];
        g = First[res["Graphics"]];
        Head[g] === Graphics &&
          Length[Cases[g, _LinearGradientFilling, Infinity]] >= 1 &&
          Length[Select[Lookup[res, "Diagnostics", {}], #["Severity"] === "Warning" &]] === 0
      ],
      True,
      TestID -> "ToGraphics-Native-Gradient-Linear"
    ],
    VerificationTest[
      Module[{res, g, rasters, na},
        res = MathemeToGraphics[
          "#image(bytes((255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 128)), format: (encoding: \"rgba8\", width: 2, height: 2), width: 20pt, height: 20pt)",
          "TypstTemplate" -> "Text"
        ];
        g = First[res["Graphics"]];
        rasters = Cases[g, r_Raster :> r, Infinity];
        na = If[Length[rasters] >= 1, rasters[[1, 1]], None];
        Head[g] === Graphics &&
          Length[rasters] >= 1 &&
          NumericArrayQ[na] &&
          Dimensions[na] === {2, 2, 4} &&
          FreeQ[g, ImportByteArray]
      ],
      True,
      TestID -> "ToGraphics-Native-Raster-NumericArray"
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
      Module[{rC, rG, rErr, diags},
        rC = MathemeCompile["x^2", "TypstTemplate" -> "Equation", "Format" -> "PDF"];
        rG = MathemeToGraphics["x^2", "TypstTemplate" -> "Equation"];
        rErr = MathemeCompile["#let x ="];
        diags = rErr["Diagnostics"];
        !KeyExistsQ[rC, "Formatted"] && !KeyExistsQ[rC, "FormattedCell"] &&
          !KeyExistsQ[rG, "Formatted"] && !KeyExistsQ[rG, "FormattedCell"] &&
          FailureQ[rErr] && rErr[[1]] === MathemeCompileError &&
          rErr["ErrorKind"] === "Compile" &&
          !KeyExistsQ[rErr[[2]], "Formatted"] &&
          Head[rErr["FormattedCell"]] === DisplayForm &&
          Length[diags] >= 1 &&
          diags[[1, "Severity"]] === "Error" &&
          diags[[1, "Source"]] === "Typst"
      ],
      True,
      TestID -> "Diagnostics-FormattedCell-And-Source-Classification"
    ],
    VerificationTest[
      Module[{res = Quiet[MathemeCompile["= Hello", "Inputs" -> <|"bad" :> 1|>]], diags},
        !FailureQ[res] &&
          !KeyExistsQ[res, "Formatted"] &&
          Head[res["FormattedCell"]] === DisplayForm &&
          Length[res["Diagnostics"]] === 1 &&
          res["Diagnostics"][[1, "Severity"]] === "Warning" &&
          res["Diagnostics"][[1, "Source"]] === "Conversion"
      ],
      True,
      TestID -> "Diagnostics-Conversion-Warning-Includes-Source-And-FormattedCell"
    ],
    VerificationTest[
      Module[{res, diags, hints},
        res = MathemeCompile["#import \"@preview/matheme-nonexistent-pkg:0.0.1\": *", "AllowDownloads" -> False];
        diags = res["Diagnostics"];
        hints = Select[diags, #["Severity"] === "Hint" &];
        FailureQ[res] && res[[1]] === MathemeCompileError &&
          Length[hints] === 1 &&
          hints[[1, "Source"]] === "Package"
      ],
      True,
      TestID -> "Diagnostics-Offline-Package-Emits-Hint-Not-Warning"
    ],
    VerificationTest[
      Module[{res = MathemeToGraphics["= Hello", "Format" -> "HTML"]},
        FailureQ[res] && res[[1]] === MathemeCallError &&
          !KeyExistsQ[res[[2]], "Formatted"] && !KeyExistsQ[res[[2]], "FormattedCell"]
      ],
      True,
      TestID -> "ToGraphics-Rejects-HTML-Format"
    ],
    VerificationTest[
      Module[{res = MathemeCompile["= Hello", "Format" -> "HTML", "Pages" -> {1}]},
        FailureQ[res] && res[[1]] === MathemeCallError &&
          res["ErrorKind"] === "Call" &&
          !KeyExistsQ[res[[2]], "Formatted"] && !KeyExistsQ[res[[2]], "FormattedCell"]
      ],
      True,
      TestID -> "Compile-HTML-Rejects-Pages-Option"
    ],
    VerificationTest[
      Module[{initial, updated, after, reset, badAssoc},
        initial = SetMatheme[];
        updated = SetMatheme["Downloads" -> True];
        after = SetMatheme[];
        badAssoc = SetMatheme[<|"Downloads" -> False|>];
        reset = ResetMatheme[];
        AssociationQ[initial] && KeyExistsQ[initial, "FontPaths"] && !KeyExistsQ[initial, "Settings"] &&
          AssociationQ[updated] && KeyExistsQ[updated, "Changed"] && updated["Settings", "Downloads"] === True &&
          after["Downloads"] === True &&
          FailureQ[badAssoc] && badAssoc[[1]] === MathemeCallError &&
          AssociationQ[reset] && reset["Settings", "Downloads"] === False
      ],
      True,
      TestID -> "SetMatheme-Read-Update-RejectAssoc-Reset"
    ],
    VerificationTest[
      Module[{orig = Options[MathemeToGraphics, "TypstTemplate"], res1, res2},
        SetOptions[MathemeToGraphics, "TypstTemplate" -> "Equation"];
        res1 = MathemeToGraphics["x^2"];
        res2 = MathemeToGraphics[TypstForm[Global`x^2]];
        SetOptions[MathemeToGraphics, orig];
        !FailureQ[res1] && Head[First[res1["Graphics"]]] === Graphics &&
          !FailureQ[res2] && Head[First[res2["Graphics"]]] === Graphics
      ],
      True,
      TestID -> "ToGraphics-Honors-SetOptions-Defaults"
    ],
    VerificationTest[
      Module[{calls = 0, failures},
        Block[{Runge`Matheme`Private`loadFunctions = Function[Null, calls++; False]},
          failures = {
            MathemeCompile["hello", "PDFTagz" -> False],
            MathemeCompile["hello", "PDFTags" -> 0],
            MathemeCompile["hello", "Pages" -> 2],
            MathemeCompile["hello", "Format" -> "HTML", "Pages" -> 2],
            MathemeCompile["hello", "PDFStandard" -> {42}],
            MathemeCompile["hello", "PDFStandard" -> "unknown-standard"],
            MathemeCompile["hello", "PDFStandard" -> {"1.4", "unknown-standard"}],
            MathemeCompile["hello", "Features" -> "unknown-feature"],
            MathemeCompile["hello", "Format" -> "unknown-format"],
            MathemeCompile["hello", "Format" -> "SVG", "PDFStandard" -> "1.4"],
            MathemeToGraphics["hello", "Format" -> "HTML"],
            MathemeWarmup["PackageIndxe" -> False],
            MathemeQueryPackages[<|"Ofset" -> 1|>],
            MathemeQueryFonts["Source" -> "unknown-source"],
            SetMatheme["Proxy" -> False]
          }
        ];
        calls === 0 && AllTrue[failures, MatchQ[#, Failure[MathemeCallError, _Association]] &]
      ],
      True,
      TestID -> "Invalid-Options-Never-Load-Library"
    ],
    VerificationTest[
      Block[{Runge`Matheme`Private`loadFunctions = Function[Null, True],
          Runge`Matheme`Private`$call = Function[arg,
            <|"Status" -> "Success", "Arguments" -> arg["Arguments"]|>]},
        With[{res = MathemeCompile["hello", "PDFStandard" -> {"1.4", "A-1B"},
            "Features" -> "A11yExtras"]},
          AssociationQ[res] &&
            res["Arguments", "PDFStandard"] === {"1.4", "A-1B"} &&
            res["Arguments", "Features"] === {"A11yExtras"}
        ]
      ],
      True,
      TestID -> "Generated-Enum-Patterns-Allow-Serde-Names-And-Aliases"
    ],
    VerificationTest[
      Module[{orig = Options[MathemeCompile, "Format"], seen = 0, default, explicit},
        SetOptions[MathemeCompile, "Format" -> "SVG"];
        default = MathemeCompile["hello", "BasePath" :> (seen++; Directory[])];
        explicit = MathemeCompile["hello", "Format" -> "PDF", "Format" -> "SVG"];
        SetOptions[MathemeCompile, orig];
        seen === 1 && AssociationQ[default] && KeyExistsQ[default, "SVG"] &&
          AssociationQ[explicit] && KeyExistsQ[explicit, "PDF"]
      ],
      True,
      TestID -> "Compile-Resolves-Dynamic-Defaults-And-First-Explicit-Rule"
    ],
    VerificationTest[
      Module[{orig = Options[MathemeCompile, "TypstTemplate"], implicit, disabled},
        Block[{Runge`Matheme`Private`loadFunctions = Function[Null, True],
            Runge`Matheme`Private`$call = Function[arg,
              <|"Status" -> "Success", "Source" -> arg["Arguments", "Source"]|>]},
          implicit = MathemeCompile[TypstForm[Global`x^2]]["Source"];
          SetOptions[MathemeCompile, "TypstTemplate" -> None];
          disabled = MathemeCompile[TypstForm[Global`x^2]]["Source"];
          SetOptions[MathemeCompile, orig];
        ];
        StringStartsQ[implicit, "#set page("] && StringQ[disabled] && !StringStartsQ[disabled, "#set page("]
      ],
      True,
      TestID -> "SetOptions-None-Disables-Automatic-TypstForm-Template"
    ],
    VerificationTest[
      Module[{res, messages},
        res = Quiet[MathemeCompile["hello", "Format" -> "SVG",
          "Inputs" -> <|"numbers" -> {2^80, N[Pi, 40], 1/3}|>,
          "Evaluate" -> "sys.inputs.numbers"], MathemeDiagnostic::warning];
        messages = Lookup[res["Diagnostics"], "Message", {}];
        AssociationQ[res] && res["Evaluated"] === {Null, N[Pi], N[1/3]} &&
          Length[messages] === 3 && AllTrue[messages, StringContainsQ[#, "Inputs[\"numbers\"]"] &] &&
          AssociationQ[MathemeVersion[]]
      ],
      True,
      TestID -> "WSTP-Preserves-Numeric-Kinds-And-Remains-Usable"
    ],
    VerificationTest[
      Module[{dir = CreateDirectory[], file, res},
        file = FileNameJoin[{dir, "main.typ"}];
        Export[file, "#let marker = \"disk\"", "Text"];
        res = MathemeCompile["#import \"main.typ\": marker", "Format" -> "SVG", "BasePath" -> dir,
          "Evaluate" -> {"marker", "{ import \"main.typ\": marker; marker }"}];
        DeleteFile[file];
        DeleteDirectory[dir];
        AssociationQ[res] && res["Evaluated"] === {"disk", "disk"}
      ],
      True,
      TestID -> "Virtual-Main-File-And-Evaluate-Preserve-Project-Imports"
    ],
    VerificationTest[
      Module[{res = MathemeCompile["hello", "PDFTags" -> False, "Pages" -> 2]},
        FailureQ[res] && StringContainsQ[res["Message"], "page selection matches no pages"]
      ],
      True,
      TestID -> "PDF-Empty-Page-Selection-Fails"
    ],
    VerificationTest[
      MathemeCompile["hello", "Format" -> "SVG",
        "CreationTimestamp" -> <|"Unix" -> 0, "OffsetMinutes" -> -480|>,
        "Evaluate" -> "datetime.today().display()"]["Evaluated"],
      "1969-12-31 16:00:00",
      TestID -> "Fixed-Timestamp-Offset-Reaches-Typst-World"
    ],
    VerificationTest[
      Module[{pub = Names["Runge`Matheme`*"]},
        MemberQ[pub, "SetMatheme"] && MemberQ[pub, "ResetMatheme"] &&
          !MemberQ[pub, "MathemeSettings"] &&
          !MemberQ[pub, "MathemeSetSettings"] &&
          !MemberQ[pub, "MathemeResetSettings"] &&
          !MemberQ[pub, "$TypstTemplates"] &&
          !MemberQ[pub, "makeMiTeXTemplate"]
      ],
      True,
      TestID -> "Package-Public-Symbols-Clean"
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
