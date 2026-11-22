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
  "UnsupportedBoxHandler" -> Automatic
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
        $typstInQuantityUnit = If[isNested, $typstInQuantityUnit, False]
      },
      body[cfg]
    ]
  ]
];

SetAttributes[withTypstConfig, HoldRest];

End[];
