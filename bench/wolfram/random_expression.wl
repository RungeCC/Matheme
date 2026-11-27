(* ::Package:: *)

(* Random symbolic-expression corpus for the TypstForm benchmarks.

   Generates expressions out of a library of special functions (Bessel,
   hypergeometric, orthogonal polynomials, elliptic, zeta, ...) so that the
   converter is exercised on realistic TemplateBox-heavy input rather than on
   arithmetic alone. Loaded by typstform_bench.wl.

   The tree is built with `Hold[h][args]` as the application head: that is
   inert, so building never evaluates anything, yet nothing in the corpus is
   Inactive -- the corpus is made of ordinary expressions, exactly the shape a
   user hands to TeXForm / MathemeToTypst. Heads become real symbols only at the
   very end, inside a held context (iHeld). "OutputForm" -> "Inactive" is
   opt-in and exists solely for the cases that deliberately cover the
   TemplateBox "InactiveHead" path.

   Representative use:
     e = First@RandomExpression[1, "MaxDepth" -> 10, "Variables" -> {x, y, z},
           "RandomSeed" -> 1];

   "OutputForm" values:
     "HoldForm"  (default) HoldForm[expr] with real heads -- the TeXForm shape
     "Inactive"            every head wrapped in Inactive -- path coverage only
     "Inert"               the raw build tree, heads being Hold[h] -- for tools
     "Evaluated"           what the tree evaluates to
     "String"              the held expression as an InputForm string *)

ClearAll[RandomExpression, RandomExpressionInfo, $RandomExpressionLibrary,
  $RandomExpressionDefaultWeights, iConfig, iGen, iNode, iLeaf, iArg,
  iConstant, iNonZeroInt, iValidQ, iGenerateValid, iHold, iPress, iHeld,
  iRelease, iInactive, iInputString, iInert, iFormat];

(* ================= 函数库 ================= *)
$RandomExpressionLibrary = <|
  "Arithmetic" -> {
    {Plus, {"Expr", "Expr"}}, {Plus, {"Expr", "Expr", "Expr"}},
    {Times, {"Expr", "Expr"}}, {Subtract, {"Expr", "Expr"}},
    {Divide, {"Expr", "Expr"}}, {Power, {"Expr", "Exponent"}},
    {Power, {"Expr", "Expr"}}},
  "Elementary" -> {
    {Sin, {"Expr"}}, {Cos, {"Expr"}}, {Tan, {"Expr"}}, {Exp, {"Expr"}},
    {Log, {"Expr"}}, {Sqrt, {"Expr"}}, {Sinh, {"Expr"}}, {Cosh, {"Expr"}},
    {Tanh, {"Expr"}}, {ArcSin, {"Expr"}}, {ArcTan, {"Expr"}},
    {ArcSinh, {"Expr"}}, {Abs, {"Expr"}}},
  "Gamma" -> {
    {Gamma, {"Expr"}}, {LogGamma, {"Expr"}}, {Gamma, {"Param", "Expr"}},
    {PolyGamma, {"Expr"}}, {PolyGamma, {"Order", "Expr"}},
    {Beta, {"Expr", "Param"}}, {Pochhammer, {"Expr", "Order"}},
    {Binomial, {"Expr", "Order"}}, {BarnesG, {"Expr"}}},
  "ErrorIntegral" -> {
    {Erf, {"Expr"}}, {Erfc, {"Expr"}}, {Erfi, {"Expr"}},
    {FresnelS, {"Expr"}}, {FresnelC, {"Expr"}}, {ExpIntegralEi, {"Expr"}},
    {ExpIntegralE, {"Order", "Expr"}}, {SinIntegral, {"Expr"}},
    {CosIntegral, {"Expr"}}, {LogIntegral, {"Expr"}}},
  "Zeta" -> {
    {Zeta, {"Expr"}}, {Zeta, {"Param", "Expr"}}, {PolyLog, {"Order", "Expr"}},
    {PolyLog, {"Param", "Expr"}}, {LerchPhi, {"Expr", "Param", "Param"}}},
  "Bessel" -> {
    {BesselJ, {"Order", "Expr"}}, {BesselY, {"Order", "Expr"}},
    {BesselI, {"Param", "Expr"}}, {BesselK, {"Param", "Expr"}},
    {HankelH1, {"Order", "Expr"}}, {SphericalBesselJ, {"Order", "Expr"}},
    {StruveH, {"Param", "Expr"}}, {AiryAi, {"Expr"}}, {AiryBi, {"Expr"}},
    {AiryAiPrime, {"Expr"}}, {KelvinBer, {"Order", "Expr"}}},
  "Orthogonal" -> {
    {LegendreP, {"Order", "Expr"}}, {LegendreQ, {"Order", "Expr"}},
    {HermiteH, {"Order", "Expr"}}, {ChebyshevT, {"Order", "Expr"}},
    {ChebyshevU, {"Order", "Expr"}}, {LaguerreL, {"Order", "Expr"}},
    {LaguerreL, {"Order", "Param", "Expr"}},
    {GegenbauerC, {"Order", "Param", "Expr"}},
    {JacobiP, {"Order", "Param", "Param", "Expr"}}},
  "Hypergeometric" -> {
    {Hypergeometric0F1, {"Param", "Expr"}},
    {Hypergeometric1F1, {"Param", "Param", "Expr"}},
    {Hypergeometric2F1, {"Param", "Param", "Param", "Expr"}},
    {HypergeometricU, {"Param", "Param", "Expr"}},
    {ParabolicCylinderD, {"Param", "Expr"}}},
  "Elliptic" -> {
    {EllipticK, {"Expr"}}, {EllipticE, {"Expr"}},
    {EllipticF, {"Expr", "Param"}}, {EllipticPi, {"Param", "Expr"}},
    {JacobiSN, {"Expr", "Param"}}, {JacobiCN, {"Expr", "Param"}},
    {JacobiDN, {"Expr", "Param"}}},
  "Misc" -> {
    {ProductLog, {"Expr"}}, {ProductLog, {"Int", "Expr"}},
    {Gudermannian, {"Expr"}},
    {MathieuC, {"Param", "Param", "Expr"}}}
|>;

$RandomExpressionDefaultWeights = <|
  "Arithmetic" -> 5, "Elementary" -> 3, "Gamma" -> 1, "ErrorIntegral" -> 1,
  "Zeta" -> 1, "Bessel" -> 1, "Orthogonal" -> 1, "Hypergeometric" -> 1,
  "Elliptic" -> 1, "Misc" -> 1, "Custom" -> 2|>;

RandomExpression::fail = "Could not generate a valid expression in `1` attempts.";
RandomExpression::nocat = "No usable categories.";
RandomExpression::badform = "Unknown \"OutputForm\" `1`; using \"HoldForm\".";

Options[RandomExpression] = {
  "Variables" -> {x}, "MaxDepth" -> 4, "MinDepth" -> 2,
  "Categories" -> All, "CategoryWeights" -> <||>, "ExtraFunctions" -> {},
  "LeafProbability" -> 0.25, "ConstantProbability" -> 0.3,
  "ParameterExpressionProbability" -> 0.2, "ConstantType" -> "Mixed",
  "IntegerRange" -> {-5, 5}, "MaxOrder" -> 4,
  "SymbolicConstants" -> {Pi, E, EulerGamma, I},
  "RequireAllVariables" -> False, "RejectDegenerate" -> True,
  "EvaluationTimeLimit" -> 2, "OutputForm" -> "HoldForm",
  "RandomSeed" -> Automatic, "MaxAttempts" -> 200
};

iConfig[opts_List] := Module[{ov, lib, cats, extra, w, maxD},
  ov = OptionValue[RandomExpression, opts, #] &;
  lib = $RandomExpressionLibrary;
  extra = ov["ExtraFunctions"];
  If[extra =!= {}, lib = Append[lib, "Custom" -> extra]];
  cats = ov["Categories"];
  cats = If[cats === All, Keys[lib], Select[Flatten[{cats}], KeyExistsQ[lib, #] &]];
  If[extra =!= {} && ! MemberQ[cats, "Custom"], AppendTo[cats, "Custom"]];
  If[cats === {}, Message[RandomExpression::nocat]; Return[$Failed]];
  w = Lookup[Join[$RandomExpressionDefaultWeights, ov["CategoryWeights"]], cats, 1];
  maxD = Max[0, ov["MaxDepth"]];
  <|
    "Variables" -> Flatten[{ov["Variables"]}],
    "MaxDepth" -> maxD, "MinDepth" -> Min[ov["MinDepth"], maxD],
    "Library" -> lib, "Categories" -> cats, "Weights" -> N[w],
    "LeafProbability" -> ov["LeafProbability"],
    "ConstantProbability" -> ov["ConstantProbability"],
    "ParamExprProb" -> ov["ParameterExpressionProbability"],
    "ConstantType" -> ov["ConstantType"], "IntegerRange" -> ov["IntegerRange"],
    "MaxOrder" -> ov["MaxOrder"], "SymbolicConstants" -> ov["SymbolicConstants"],
    "RequireAllVariables" -> TrueQ[ov["RequireAllVariables"]],
    "RejectDegenerate" -> TrueQ[ov["RejectDegenerate"]],
    "EvaluationTimeLimit" -> ov["EvaluationTimeLimit"],
    "OutputForm" -> ov["OutputForm"], "MaxAttempts" -> ov["MaxAttempts"]
  |>
];

(* ================= 惰性构造 ================= *)
(* `Hold[h][args]` is an application whose head is the *expression* Hold[h]: it
   never evaluates, so the tree can be built freely, and no part of it is
   Inactive. Turning those heads into real symbols (iHeld) or into Inactive
   (iInactive) is a separate, final step. *)

SetAttributes[iHold, HoldAllComplete];

(* Wrap without evaluating: iHold[e] -> Hold[e], holding the very same tree. *)
iPress[e_] := iHold[e] /. iHold[x_] :> Hold[x];

(* Real heads inside HoldForm -- the shape TeXForm / MathemeToTypst actually see. *)
iHeld[e_] := HoldForm @@ (iPress[e] /. Hold[h_Symbol] :> h);

(* Real heads, released: the only helper that evaluates. *)
iRelease[e_] := ReleaseHold[iPress[e] /. Hold[h_Symbol] :> h];

(* Opt-in: every head wrapped in Inactive, i.e. the debugging shape that boxes
   to TemplateBox[..., "InactiveHead"]. Inert by construction, so the final
   ReleaseHold only drops the wrapper iPress added. *)
iInactive[e_] := ReleaseHold[iPress[e] /. Hold[h_Symbol] :> Inactive[h]];

iInputString[e_] := StringReplace[ToString[iHeld[e], InputForm],
  StartOfString ~~ "HoldForm[" ~~ body__ ~~ "]" ~~ EndOfString :> body];

(* Back to the build tree from any output form, without evaluating anything:
   every rule fires inside the Hold that wraps the whole expression. *)
iInert[e_] := Hold[e] /. HoldForm[x_] :> x /. Hold[h_Symbol] :> h /. Inactive[h_Symbol] :> h;

(* ================= 生成 ================= *)

iNonZeroInt[{a_, b_}] := RandomChoice[DeleteCases[Range[a, b], 0] /. {} -> {1}];

iConstant[cfg_, type_: Automatic] := Module[{t = If[type === Automatic, cfg["ConstantType"], type]},
  If[t === "Mixed",
    t = RandomChoice[{4, 2, 1, If[cfg["SymbolicConstants"] === {}, 0, 1]} ->
        {"Integer", "Rational", "Real", "Symbolic"}]];
  Switch[t,
    "Integer", iNonZeroInt[cfg["IntegerRange"]],
    "Rational", iNonZeroInt[cfg["IntegerRange"]]/RandomInteger[{2, 5}],
    "Real", Round[RandomReal[cfg["IntegerRange"]], 0.01] /. 0. -> 0.5,
    "Symbolic", RandomChoice[cfg["SymbolicConstants"]],
    _, iNonZeroInt[cfg["IntegerRange"]]
  ]
];

iLeaf[cfg_] := If[RandomReal[] < cfg["ConstantProbability"],
  iConstant[cfg], RandomChoice[cfg["Variables"]]];

iGen[cfg_, depth_, level_] := Which[
  depth <= 0, iLeaf[cfg],
  level >= cfg["MinDepth"] && RandomReal[] < cfg["LeafProbability"], iLeaf[cfg],
  True, iNode[cfg, depth, level]
];

(* With[] is needed: `Hold[head]` on its own would keep the *variable* head
   rather than its value. The body then evaluates to Hold[f][args], inert. *)
iNode[cfg_, depth_, level_] := Module[{cat, head, argTypes, args},
  cat = RandomChoice[cfg["Weights"] -> cfg["Categories"]];
  {head, argTypes} = RandomChoice[cfg["Library"][cat]];
  args = iArg[cfg, #, depth - 1, level + 1] & /@ argTypes;
  With[{h = head}, Hold[h] @@ args]
];

iArg[cfg_, "Expr", d_, l_] := iGen[cfg, d, l];
iArg[cfg_, "Order", _, _] := RandomInteger[{0, cfg["MaxOrder"]}];
iArg[cfg_, "Int", _, _] := iNonZeroInt[cfg["IntegerRange"]];
iArg[cfg_, "Exponent", _, _] := RandomChoice[{2, 2, 3, -1, -2, 1/2, 1/3, -1/2}];
iArg[cfg_, "Param", d_, l_] :=
  If[d > 0 && RandomReal[] < cfg["ParamExprProb"],
    iGen[cfg, d, l],
    RandomChoice[{iConstant[cfg, "Rational"], iConstant[cfg, "Integer"]}]];
iArg[cfg_, _, d_, l_] := iGen[cfg, d, l];

(* Runs on the inert tree: FreeQ inspects it directly, and the degeneracy probe
   is the only place that evaluates -- time-limited and Quiet. *)
iValidQ[cfg_, e_] := Module[{vars = cfg["Variables"], a},
  If[cfg["RequireAllVariables"],
    If[! AllTrue[vars, ! FreeQ[e, #] &], Return[False]],
    If[AllTrue[vars, FreeQ[e, #] &], Return[False]]];
  If[cfg["RejectDegenerate"],
    a = Quiet@TimeConstrained[iRelease[e], cfg["EvaluationTimeLimit"], $Aborted];
    If[a === $Aborted || ! FreeQ[a, Indeterminate | _DirectedInfinity] ||
       AllTrue[vars, FreeQ[a, #] &], Return[False]]];
  True
];

iGenerateValid[cfg_] := Module[{e = $Failed, k = 0, ok = False},
  While[! ok && k < cfg["MaxAttempts"],
    k++; e = iGen[cfg, cfg["MaxDepth"], 0]; ok = iValidQ[cfg, e]];
  If[ok, e, Message[RandomExpression::fail, cfg["MaxAttempts"]]; $Failed]
];

iFormat[$Failed, _] := $Failed;
iFormat[e_, "Inert"] := e;
iFormat[e_, "HoldForm"] := iHeld[e];
iFormat[e_, "Inactive"] := iInactive[e];
iFormat[e_, "Evaluated"] := Quiet[iRelease[e]];
iFormat[e_, "String"] := iInputString[e];
iFormat[e_, other_] := (Message[RandomExpression::badform, other]; iHeld[e]);

RandomExpression[opts : OptionsPattern[]] := First@RandomExpression[1, opts];

RandomExpression[n_Integer?Positive, opts : OptionsPattern[]] := Module[{cfg, seed, run},
  cfg = iConfig[{opts}];
  If[cfg === $Failed, Return[$Failed]];
  run = Function[Table[iFormat[iGenerateValid[cfg], cfg["OutputForm"]], n]];
  seed = OptionValue["RandomSeed"];
  If[seed === Automatic, run[], BlockRandom[run[], RandomSeeding -> seed]]
];

(* Accepts any output form; the tree is recovered without evaluating it. *)
RandomExpressionInfo[e_] := Module[{t = iInert[e]},
  <|
    "Heads" -> Counts@Cases[t, h_Symbol[___] :> h, {1, Infinity}, Heads -> True],
    "Depth" -> Depth[t] - 1,
    "LeafCount" -> LeafCount[t],
    "Variables" -> Union@Cases[t, _Symbol?(Context[#] =!= "System`" &), Infinity]
  |>
];
