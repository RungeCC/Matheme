(* ::Package:: *)

(* TypstForm/Config.wl: option definitions, validation, and dynamic conversion state. *)

Begin["`TypstForm`"];

$typstBoxOptionDefaults = {
  "BoxRules" -> {},
  "TemplateRules" -> <||>,
  "CharacterRules" -> <||>,
  "OperatorNames" -> {},
  "DelimiterStyle" -> "Semantic",
  "FractionStyle" -> "Fraction",
  "ConstantStyle" -> "Italic",
  "DifferentialStyle" -> "Upright",
  "TransposeSymbol" -> "upright(T)",
  "ConjugateStyle" -> "Superscript",
  "BlackboardStyle" -> "Function",
  "ScriptStyle" -> "Calligraphic",
  "MultiLetterStyle" -> "Quoted",
  "MatrixDelimiter" -> Automatic,
  "PreserveStyle" -> True,
  "StripContext" -> True,
  "UnknownCharacterHandler" -> Automatic,
  "UnsupportedBoxHandler" -> Automatic,
  "NativeDiagnostics" -> False
};

$typstExprOptionDefaults = Join[
  {
    "Form" -> TraditionalForm,
    "ExpressionRules" -> {}
  },
  $typstBoxOptionDefaults
];

(* Dynamic conversion state scoped per top-level call via Block. *)
$typstActiveConfig = None;
$typstCharMap = None;
$typstOperatorMap = None;
$typstTemplateMap = None;
$typstInMatrixCell = False;
$typstInCasesCell = False;
$typstInDelimiterDepth = 0;
$typstInQuantityUnit = False;
$typstEvalDepth = 0;
$typstMaxEvalDepth = 64;

(* Internal switch: when True, a native fallback emits TypstForm::nativefallback
   with the rejected stage/field/reason. Default output stays silent. *)
$typstNativeDiagnostics = False;

(* Latched when the native link itself fails (library missing, transport
   error): later conversions skip the attempt instead of retrying
   LibraryFunctionLoad on every call. Cleared by MathemeClose[] and whenever
   the library loads successfully. *)
$typstNativeLinkFailed = False;

(* Single-entry memo mapping a configuration association to its native patch.
   Identical options rebuild an identical patch on every call, and the diff
   against the ~800-entry default character map alone costs about a millisecond
   -- a pure per-call tax on top of every conversion. Association keys compare
   by value, so the memo hits across calls that pass the same options. *)
$typstNativePatchCache = <||>;

normalizeRuleList[rules_List] := Flatten[rules];
normalizeRuleList[rule : (_Rule | _RuleDelayed)] := {rule};
normalizeRuleList[assoc_Association] := Normal[assoc];
normalizeRuleList[None | Null] := {};
normalizeRuleList[_] := {};

normalizeRuleAssoc[assoc_Association] := assoc;
normalizeRuleAssoc[rules : {(_Rule | _RuleDelayed) ...}] := Association[rules];
normalizeRuleAssoc[rule : (_Rule | _RuleDelayed)] := Association[rule];
normalizeRuleAssoc[None | Null | {}] := <||>;
normalizeRuleAssoc[_] := <||>;

resolveTypstConfig[baseDefaults_List, rawOpts_List] := Module[{base, flatOpts, merged},
  base = If[AssociationQ[$typstActiveConfig],
    $typstActiveConfig,
    Association[baseDefaults]
  ];
  flatOpts = Flatten[rawOpts];
  merged = If[flatOpts === {},
    base,
    Join[base, Association[flatOpts]]
  ];
  merged
];

withTypstConfig[baseDefaults_List, rawOpts_List, body_] := Module[
  {cfg, charMap, opMap, tplMap, isNested},
  isNested = AssociationQ[$typstActiveConfig];
  cfg = resolveTypstConfig[baseDefaults, rawOpts];
  (* Reuse prebuilt lookup maps on nested recursive calls if no overrides were passed. *)
  If[isNested && Flatten[rawOpts] === {},
    body[cfg],
    charMap = buildTypstCharMap[cfg];
    opMap = buildTypstOperatorMap[cfg];
    tplMap = normalizeRuleAssoc[Lookup[cfg, "TemplateRules", <||>]];
    Block[
      {
        $typstActiveConfig = cfg,
        $typstCharMap = charMap,
        $typstOperatorMap = opMap,
        $typstTemplateMap = tplMap,
        $typstInMatrixCell = If[isNested, $typstInMatrixCell, False],
        $typstInCasesCell = If[isNested, $typstInCasesCell, False],
        $typstInDelimiterDepth = If[isNested, $typstInDelimiterDepth, 0],
        $typstInQuantityUnit = If[isNested, $typstInQuantityUnit, False],
        $typstNativeDiagnostics = TrueQ[Lookup[cfg, "NativeDiagnostics", False]]
      },
      body[cfg]
    ]
  ]
];

SetAttributes[withTypstConfig, HoldRest];

End[];
