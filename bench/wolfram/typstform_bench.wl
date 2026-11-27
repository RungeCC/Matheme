(* ::Package:: *)

(* Convert-level benchmark for the TypstForm layer.

   Three engines run on the same expressions:

     TeX      ToString[TeXForm[expr]]   -- the reference the speed claim uses
     Ref      the Wolfram-only box -> Typst converter (`convertBox`), fed the
              exact same boxes, i.e. the correctness oracle
     Native   MathemeToTypst            -- the LibraryLink `boxesToTypst` path

   Corpus: flat sums, balanced trees, and RandomExpression special-function
   trees as plain HoldForm values (real heads, nothing evaluated). Three of the
   latter are repeated in "Inactive" wrapping on purpose, to cover the
   TemplateBox "InactiveHead" path -- that is path coverage, not the normal
   shape of the corpus.

   Run from anywhere inside the repository, after `cargo build --release -p matheme`:

     wolframscript -file bench/wolfram/typstform_bench.wl

   Exits with status 1 when native output differs from the reference. *)

$HistoryLength = 0;

(* This script lives in bench/wolfram/, so the repository root is two levels up;
   nothing depends on the working directory. *)
$benchDir = DirectoryName[ExpandFileName[$InputFileName]];
$repoRoot = DirectoryName[DirectoryName[$benchDir]];
If[! FileExistsQ[FileNameJoin[{$repoRoot, "Kernel", "Matheme.wl"}]],
  Print["bench: cannot locate the repository root from ", $InputFileName];
  Exit[2]
];

Get[FileNameJoin[{$benchDir, "random_expression.wl"}]];
Get[FileNameJoin[{$repoRoot, "Kernel", "Matheme.wl"}]];

(* Load straight from the build tree: the converter is what is under test, so a
   stale assembled paclet would silently benchmark the wrong binary. The file
   name is the package's own, so this stays platform-agnostic. *)
$libraryFile = Runge`Matheme`Private`$libraryFileName;
If[! StringQ[$libraryFile],
  $libraryFile = If[$OperatingSystem === "Windows", "matheme.dll", "matheme.so"]];
$dll = FileNameJoin[{$repoRoot, "target", "release", $libraryFile}];
If[! FileExistsQ[$dll],
  Print["bench: missing ", $dll, " -- run `cargo build --release -p matheme` first"];
  Exit[2]
];
Runge`Matheme`Private`$library = $dll;

(* ------------------------------------------------------------------------ *)
(* Reference engine                                                         *)
(* ------------------------------------------------------------------------ *)

$typstContext = "Runge`Matheme`Private`TypstForm`";
typstSymbol[name_String] := Symbol[$typstContext <> name];
$typstBoxDefaults = typstSymbol["$typstBoxOptionDefaults"];
$typstExprDefaults = typstSymbol["$typstExprOptionDefaults"];
$typstPrepare = typstSymbol["makeBoxesPreprocessed"];
$typstStrip = typstSymbol["stripIdentityBoxes"];
$typstConvert = typstSymbol["convertBox"];
$typstWithConfig = typstSymbol["withTypstConfig"];

(* Line-for-line MathemeToTypst with the WL renderer in place of the native
   one, so the only difference in the measurement is the converter itself. *)
referenceTypst[expr_] := $typstWithConfig[$typstExprDefaults, {}, Function[cfg,
  Module[{boxes = $typstPrepare[expr, TraditionalForm, {}]},
    StringTrim[$typstConvert[$typstStrip[boxes]]]
  ]
]];

(* ------------------------------------------------------------------------ *)
(* Timing                                                                   *)
(* ------------------------------------------------------------------------ *)

(* RepeatedTiming's second argument is a *time budget* in seconds. Taking the
   median of three such rounds keeps one GC pause or a background thread from
   deciding the result, which matters for the multi-hundred-leaf cases. *)
SetAttributes[timeMs, HoldFirst];
timeMs[e_, budget_: 0.25] := Median[Table[1000.0 First[RepeatedTiming[e, budget]], {3}]];

(* ------------------------------------------------------------------------ *)
(* Corpus                                                                   *)
(* ------------------------------------------------------------------------ *)

$atoms = {x, y, z, a, b, c, \[Alpha], \[Beta], \[Gamma], Pi, n, m, p, q};

term[0] := RandomChoice[$atoms];
term[d_Integer] := Module[{r = RandomReal[], sub = term[d - 1]},
  Which[
    r < 0.28, RandomChoice[$atoms],
    r < 0.46, RandomChoice[{Sin, Cos, Tan, Log, Exp, ArcTan}][sub],
    r < 0.60, Sqrt[sub],
    r < 0.73, sub^RandomInteger[{2, 5}],
    r < 0.84, RandomChoice[{f, g, h}][sub],
    True, sub + term[d - 1]
  ]
];

tree[0] := RandomChoice[$atoms];
tree[d_Integer] := Module[{l = tree[d - 1], r = tree[d - 1]},
  Switch[RandomInteger[{0, 4}],
    0, l + r,
    1, l r,
    2, Sin[l] + Cos[r],
    3, Sqrt[l^r],
    _, (l + r)/(1 + r)
  ]
];

SeedRandom[20261126];
flatSum[size_Integer] := Plus @@ Table[term[RandomInteger[{1, 3}]], {size}];

$cases = Join[
  {{"flat-40", "plain", flatSum[40]}, {"flat-160", "plain", flatSum[160]},
   {"tree-5", "plain", tree[5]}, {"tree-7", "plain", tree[7]}},
  Table[
    (* HoldForm is the shape TeXForm sees. *)
    {"random-d" <> ToString[d], "HoldForm",
     First@RandomExpression[1, "MaxDepth" -> d, "Variables" -> {x, y, z},
       "OutputForm" -> "HoldForm", "RandomSeed" -> d]},
    {d, {6, 8, 10}}],
  (* The same trees with every head Inactive: path coverage for the
     "InactiveHead" template, not the normal shape of a corpus. *)
  Table[
    {"random-d" <> ToString[d], "Inactive",
     First@RandomExpression[1, "MaxDepth" -> d, "Variables" -> {x, y, z},
       "OutputForm" -> "Inactive", "RandomSeed" -> d]},
    {d, {6, 8, 10}}]
];

(* Warm every engine once so the first case does not pay for library loading,
   memoisation, or definition autoloading. *)
With[{$warm = First@RandomExpression[1, "MaxDepth" -> 4, "Variables" -> {x},
    "OutputForm" -> "HoldForm", "RandomSeed" -> 1]},
  MathemeToTypst[$warm]; ToString[TeXForm[$warm]]; referenceTypst[$warm]];

(* ------------------------------------------------------------------------ *)
(* Report                                                                   *)
(* ------------------------------------------------------------------------ *)

Print["\n=== TypstForm Box -> Typst Benchmark (ms per conversion) ==="];
Print[StringPadRight["Case", 14], StringPadRight["Form", 10],
  StringPadLeft["Leaf", 6], StringPadLeft["TeX", 9], StringPadLeft["Ref", 9],
  StringPadLeft["Native", 9], StringPadLeft["TeX/Nat", 9], StringPadLeft["Ref/Nat", 9],
  StringPadLeft["Same", 6]];

(* Round through an integer so no 1.3800000000000001-style noise is printed. *)
hundredths[x_] := Round[100. x]/100.;
thousandths[x_] := Round[1000. x]/1000.;

mismatches = 0;
ratios = <||>;

Scan[
  Function[spec,
    Module[{label = spec[[1]], form = spec[[2]], expr = spec[[3]],
       tex, ref, native, same},
      tex = timeMs[ToString@TeXForm@expr];
      ref = timeMs[referenceTypst[expr]];
      native = timeMs[MathemeToTypst[expr]];
      same = referenceTypst[expr] === MathemeToTypst[expr];
      If[! same, mismatches++];
      ratios[label <> "/" <> form] = tex/native;
      Print[StringPadRight[label, 14], StringPadRight[form, 10],
        StringPadLeft[ToString[LeafCount[expr]], 6],
        StringPadLeft[ToString[thousandths[tex]], 9],
        StringPadLeft[ToString[thousandths[ref]], 9],
        StringPadLeft[ToString[thousandths[native]], 9],
        StringPadLeft[ToString[hundredths[tex/native]], 9],
        StringPadLeft[ToString[hundredths[ref/native]], 9],
        StringPadLeft[If[same, "yes", "NO"], 6]];
      If[! same,
        Print["  native: ", MathemeToTypst[expr]];
        Print["  ref   : ", referenceTypst[expr]]
      ]
    ]
  ],
  $cases
];

Print["\nTeX / Native: median ", ToString[hundredths[Median[Values[ratios]]]],
  ", min ", ToString[hundredths[Min[Values[ratios]]]],
  ", max ", ToString[hundredths[Max[Values[ratios]]]], "  (>1 means native is faster)"];
Print[If[mismatches === 0,
  "all " <> ToString[Length[$cases]] <> " cases match the reference",
  "MISMATCHES: " <> ToString[mismatches] <> " of " <> ToString[Length[$cases]]]];

If[mismatches > 0, Exit[1]];
