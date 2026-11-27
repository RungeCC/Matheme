(* ::Package:: *)

(* Native vs WL parity for every claimed TypstForm configuration field.
   Unlike TypstFormTests.wl this suite requires the compiled matheme LibraryLink
   library, because it compares the Rust renderer with the WL reference on the
   same box tree. Run with:

     wolframscript -code 'Get["tests/wolfram/TypstFormNativeParityTests.wl"];
       Print[Runge`Matheme`TypstFormNativeParityTests`RunTypstFormNativeParityTests[]]' *)

Get[FileNameJoin[{DirectoryName[DirectoryName[DirectoryName[$InputFileName]]], "Kernel", "Matheme.wl"}]];

BeginPackage["Runge`Matheme`TypstFormNativeParityTests`", {"Runge`Matheme`"}];

RunTypstFormNativeParityTests::usage =
  "RunTypstFormNativeParityTests[] compares the native renderer with the WL reference for all claimed options.";

Begin["`Private`"];

tf = "Runge`Matheme`Private`TypstForm`";
patchFn = Symbol[tf <> "nativeConfigPatch"];
defaults = Symbol[tf <> "$typstBoxOptionDefaults"];
withCfg = Symbol[tf <> "withTypstConfig"];
convertFn = Symbol[tf <> "convertBox"];
stripFn = Symbol[tf <> "stripIdentityBoxes"];
callFunction = Runge`Matheme`Private`callFunction;

wlResult[boxes_, opts_List] := withCfg[
  defaults, opts,
  Function[cfg, StringTrim[convertFn[stripFn[boxes]]]]
];

(* Mirrors production: `convertBoxNative` receives already-stripped boxes, so
   the native side is compared on the same input the entry points send it. *)
nativeResult[boxes_, opts_List] := withCfg[
  defaults, opts,
  Function[cfg, Module[{patch = patchFn[cfg], res},
    If[Head[patch] === Failure, Return[Failure["Config", patch[[2]]]]];
    res = callFunction["boxesToTypst", <|"Boxes" -> stripFn[boxes], "Config" -> patch|>];
    If[TrueQ[Lookup[res, "Supported", False]], Lookup[res, "Result"],
      Failure[Lookup[res, "Stage", "?"], <|
        "Field" -> Lookup[res, "Field", "?"],
        "Reason" -> Lookup[res, "Reason", "?"]
      |>]]
  ]]
];

(* The expected strings are the same goldens pinned by
   crates/matheme-box/tests/parity_goldens.rs; update both in the same commit.
   Self-consistency alone cannot catch the two engines drifting together. *)
parityCases = {
  {"FractionStyle-Slash", FractionBox["1", "2"], {"FractionStyle" -> "Slash"},
    "(1)/(2)"},
  {"ConjugateStyle-Overline", TemplateBox[{"x"}, "Conjugate"], {"ConjugateStyle" -> "Overline"},
    "overline(x)"},
  {"TransposeSymbol-template", TemplateBox[{"x"}, "Transpose"], {"TransposeSymbol" -> "t"},
    "x^t"},
  {"TransposeSymbol-character", RowBox[{FromCharacterCode[16^^f3c7]}], {"TransposeSymbol" -> "t"},
    "t"},
  {"MultiLetterStyle-Italic", RowBox[{"abc"}], {"MultiLetterStyle" -> "Italic"},
    "italic(\"abc\")"},
  {"MultiLetterStyle-Upright", RowBox[{"abc"}], {"MultiLetterStyle" -> "Upright"},
    "upright(\"abc\")"},
  {"StripContext-False", RowBox[{"MyPkg`x"}], {"StripContext" -> False},
    "\"MyPkg`x\""},
  {"PreserveStyle-False", StyleBox["x", FontWeight -> Bold], {"PreserveStyle" -> False},
    "x"},
  {"DelimiterStyle-Explicit-row",
    RowBox[{FromCharacterCode[16^^230a], "x", FromCharacterCode[16^^230b]}],
    {"DelimiterStyle" -> "Explicit"},
    "lr(floor.l x floor.r)"},
  {"DelimiterStyle-Explicit-template", TemplateBox[{"x"}, "Abs"], {"DelimiterStyle" -> "Explicit"},
    "lr(|x|)"},
  {"MatrixDelimiter-explicit",
    RowBox[{"(", GridBox[{{"a", "b"}, {"c", "d"}}], ")"}],
    {"MatrixDelimiter" -> "\"[\""},
    "(mat(delim: #none, a, b; c, d))"},
  {"ConstantStyle-Upright", RowBox[{FromCharacterCode[16^^f74d]}], {"ConstantStyle" -> "Upright"},
    "upright(e)"},
  {"DifferentialStyle-Italic", RowBox[{FromCharacterCode[16^^f74c]}], {"DifferentialStyle" -> "Italic"},
    "d"},
  {"CharacterRules", RowBox[{"\[Pi]"}], {"CharacterRules" -> {"\[Pi]" -> "pi.alt "}},
    "pi.alt"},
  {"OperatorNames", RowBox[{"myop"}], {"OperatorNames" -> {"myop"}},
    "op(\"myop\")"},
  {"TemplateRules-arity-key", TemplateBox[{"x"}, "Abs"],
    {"TemplateRules" -> {"Abs" -> "generic", "Abs/1" -> "exact"}},
    "exact"},
  {"TypstTemplate-fixed", TemplateBox[{"x", "y"}, "MyTag"],
    {"TemplateRules" -> {"MyTag" -> TypstTemplate["f({1}, {2})"]}},
    "f(x, y)"},
  {"TypstTemplate-rest", TemplateBox[{"x", "y", "z"}, "MyTag"],
    {"TemplateRules" -> {"MyTag" -> TypstTemplate["row({1}; {rest})", "; "]}},
    "row(x; y; z)"}
};

RunTypstFormNativeParityTests[] := Module[{tests, report},
  tests = Join[
    VerificationTest[
      nativeResult[#[[2]], #[[3]]] === wlResult[#[[2]], #[[3]]] === #[[4]],
      True,
      TestID -> "Parity-" <> #[[1]]
    ] & /@ parityCases,
    {
      VerificationTest[
        nativeResult[FractionBox["1", "2"], {"BoxRules" -> {FractionBox[n_, d_] :> "myfrac"}}],
        Failure["Config", <|"Field" -> "BoxRules", "Reason" -> "BoxRules needs WL evaluation"|>],
        TestID -> "Parity-BoxRules-FailsClosed"
      ],
      VerificationTest[
        nativeResult[TemplateBox[{"x"}, "MyTag"],
          {"TemplateRules" -> {"MyTag" -> StringTemplate["{{1}}"]}}],
        Failure["Config", <|
          "Field" -> "TemplateRules", "Reason" -> "template handler needs WL evaluation"
        |>],
        TestID -> "Parity-WolframStringTemplate-FailsClosed"
      ]
    }
  ];
  report = TestReport[tests];
  <|
    "AllPassed" -> TrueQ[
      report["TestsFailedCount"] == 0 && report["TestsSucceededCount"] == Length[tests]
    ],
    "Succeeded" -> report["TestsSucceededCount"],
    "Failed" -> report["TestsFailedCount"],
    "Failures" -> Join[
      report["TestsFailedWrongResultsKeys"],
      report["TestsFailedWithMessagesKeys"],
      report["TestsNotEvaluatedKeys"]
    ]
  |>
];

End[];
EndPackage[];
