(* ::Package:: *)
(* End-to-end Wolfram Language benchmark suite for Matheme.
   Compares native "Graphics" export against "SVG" and "PDF" import pipelines
   across math, prose, multi-page filtering, winding resolution, and glyph churn.

   Run after `nu scripts/build.nu`, from anywhere inside the repository:
     wolframscript -file bench/wolfram/endtoend_bench.wl
*)

(* This script lives in bench/wolfram/, so the repository root is two levels up;
   nothing here depends on the working directory. *)
$repoRoot = DirectoryName[DirectoryName[DirectoryName[ExpandFileName[$InputFileName]]]];

PacletDirectoryLoad[FileNameJoin[{$repoRoot, "output", "Matheme"}]];
Needs["Runge`Matheme`"];

MathemeWarmup["Fonts" -> True, "Packages" -> False, "PackageIndex" -> False];

benchCase[name_String, iters_Integer, fn_] := Module[{times},
  (* Warmup *)
  fn[];
  times = Table[First[AbsoluteTiming[fn[]]] * 1000.0, {iters}];
  <|
    "Case" -> name,
    "Iters" -> iters,
    "MedianMs" -> Round[Median[times], 0.01],
    "MinMs" -> Round[Min[times], 0.01],
    "MaxMs" -> Round[Max[times], 0.01]
  |>
];

$equationSrc = "integral_0^infinity e^(-x^2) dif x = sqrt(pi)/2";
$academicSrc = StringJoin[
  "#set page(width: 420pt, height: 520pt, margin: 20pt)\n",
  "#for s in range(4) [= Sec #s\n#lorem(80)\n$sum_(n=1)^infinity 1/n^2 = pi^2/6$\n#if s < 3 [#pagebreak()]]"
];
$starSrc = "#pad(2pt, polygon(fill: rgb(\"#2563eb\"), fill-rule: \"non-zero\", (15pt, 0pt), (25pt, 30pt), (0pt, 12pt), (30pt, 12pt), (5pt, 30pt)))";

results = {
  benchCase["Equation -> Graphics (native)", 15,
    Function[Null, MathemeToGraphics[$equationSrc, "TypstTemplate" -> "Equation", "Format" -> "Graphics"]]
  ],
  benchCase["Equation -> SVG -> Graphics", 5,
    Function[Null, MathemeToGraphics[$equationSrc, "TypstTemplate" -> "Equation", "Format" -> "SVG"]]
  ],
  benchCase["Equation -> PDF -> Graphics", 5,
    Function[Null, MathemeToGraphics[$equationSrc, "TypstTemplate" -> "Equation", "Format" -> "PDF"]]
  ],
  benchCase["Academic 4p -> Graphics (native)", 10,
    Function[Null, MathemeToGraphics[$academicSrc, "Format" -> "Graphics"]]
  ],
  benchCase["Academic 4p Pages->{2,4} -> Graphics", 10,
    Function[Null, MathemeToGraphics[$academicSrc, "Format" -> "Graphics", "Pages" -> {2, 4}]]
  ],
  benchCase["NonZero Star -> Graphics (native)", 15,
    Function[Null, MathemeToGraphics[$starSrc, "TypstTemplate" -> "Text", "Format" -> "Graphics"]]
  ]
};

Print["\n=== Matheme End-to-End Wolfram Benchmark ==="];
Scan[
  Function[r,
    Print[
      StringPadRight[r["Case"], 40],
      " median=", StringPadLeft[ToString[r["MedianMs"]], 7], "ms",
      " min=", StringPadLeft[ToString[r["MinMs"]], 7], "ms",
      " max=", StringPadLeft[ToString[r["MaxMs"]], 7], "ms"
    ]
  ],
  results
];

MathemeClose[];
