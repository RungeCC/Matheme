(* ::Package:: *)

(* TypstForm/Characters.wl: Typst 0.15.1-verified character, operator, and delimiter tables. *)

Begin["`TypstForm`"];

(* ------------------------------------------------------------------------ *)
(* Basic Typst math escapes                                                 *)
(* ------------------------------------------------------------------------ *)

$typstBasicEscapes = <|
  "#" -> "\\#",
  "$" -> "\\$",
  "&" -> "\\&",
  "_" -> "\\_",
  "^" -> "\\^",
  "\\" -> "backslash ",
  "\"" -> "\\\"",
  "~" -> "tilde.op ",
  "`" -> "grave(\"\") "
|>;

(* ------------------------------------------------------------------------ *)
(* Spaces and invisible formatting characters                               *)
(* ------------------------------------------------------------------------ *)

$typstSpaces = <|
  " " -> " ",
  "\t" -> "quad ",
  "\n" -> "",
  "\:00a0" -> "",       (* NonBreakingSpace *)
  "\:2005" -> "thick ", (* FourPerEmSpace *)
  "\:2009" -> "thin ",  (* ThinSpace *)
  "\:200a" -> "thin ",  (* HairSpace *)
  "\:2028" -> "",       (* LineSeparator *)
  "\:2029" -> "",       (* ParagraphSeparator *)
  "\:205f" -> "med ",   (* MediumMathSpace *)
  "\:2060" -> "",       (* WordJoiner / NoBreak *)
  "\:2061" -> "",       (* FunctionApplication *)
  "\:2062" -> "",       (* InvisibleTimes *)
  "\:2063" -> "",       (* InvisibleSeparator *)
  "\:2064" -> "",       (* InvisiblePlus *)
  "\:f35f" -> "",
  "\:f360" -> "",       (* InvisibleSpace *)
  "\:f380" -> "",       (* NegativeVeryThinSpace *)
  "\:f382" -> "",       (* NegativeThinSpace *)
  "\:f383" -> "",       (* NegativeMediumSpace *)
  "\:f384" -> "",       (* NegativeThickSpace *)
  "\:f39f" -> "",
  "\:f3a0" -> "",       (* Null *)
  "\:f3a2" -> "",       (* COMPATIBILITYNoBreak *)
  "\:f3a3" -> "",       (* IndentingNewLine *)
  "\:f3a8" -> "",       (* AutoLeftMatch *)
  "\:f3a9" -> "",       (* AutoRightMatch *)
  "\:f3ad" -> " ",      (* AutoSpace *)
  "\:f3ae" -> "",       (* AutoOperand *)
  "\:f3b0" -> "",
  "\:f3b3" -> "thin ",  (* VeryThinSpace *)
  "\:f3b4" -> " ",      (* ThickSpace *)
  "\:f3ba" -> "",       (* SpanFromLeft *)
  "\:f3bb" -> "",       (* SpanFromAbove *)
  "\:f3bc" -> "",       (* SpanFromBoth *)
  "\:f760" -> "",       (* AlignmentMarker *)
  "\:f765" -> "",       (* InvisibleComma *)
  "\:f76d" -> "",       (* InvisibleApplication *)
  "\:f76e" -> "",       (* DiscretionaryLineSeparator *)
  "\:f76f" -> ""        (* DiscretionaryParagraphSeparator *)
|>;

(* ------------------------------------------------------------------------ *)
(* Greek letters                                                            *)
(* ------------------------------------------------------------------------ *)

$typstGreekLetters = <|
  "\:03b1" -> "alpha ",
  "\:03b2" -> "beta ",
  "\:03b3" -> "gamma ",
  "\:03b4" -> "delta ",
  "\:03f5" -> "epsilon ",
  "\:03b5" -> "epsilon.alt ",
  "\:03b6" -> "zeta ",
  "\:03b7" -> "eta ",
  "\:03b8" -> "theta ",
  "\:03d1" -> "theta.alt ",
  "\:03b9" -> "iota ",
  "\:03ba" -> "kappa ",
  "\:03f0" -> "kappa.alt ",
  "\:03bb" -> "lambda ",
  "\:03bc" -> "mu ",
  "\:03bd" -> "nu ",
  "\:03be" -> "xi ",
  "\:03bf" -> "o",
  "\:03c0" -> "pi ",
  "\:03d6" -> "pi.alt ",
  "\:03c1" -> "rho ",
  "\:03f1" -> "rho.alt ",
  "\:03c3" -> "sigma ",
  "\:03c2" -> "sigma.alt ",
  "\:03c4" -> "tau ",
  "\:03c5" -> "upsilon ",
  "\:03d5" -> "phi ",
  "\:03c6" -> "phi.alt ",
  "\:03c7" -> "chi ",
  "\:03c8" -> "psi ",
  "\:03c9" -> "omega ",
  "\:03dd" -> "digamma ",
  "\:0391" -> "A",
  "\:0392" -> "B",
  "\:0393" -> "Gamma ",
  "\:0394" -> "Delta ",
  "\:0395" -> "E",
  "\:0396" -> "Z",
  "\:0397" -> "H",
  "\:0398" -> "Theta ",
  "\:0399" -> "I",
  "\:039a" -> "K",
  "\:039b" -> "Lambda ",
  "\:039c" -> "M",
  "\:039d" -> "N",
  "\:039e" -> "Xi ",
  "\:039f" -> "O",
  "\:03a0" -> "Pi ",
  "\:03a1" -> "P",
  "\:03a3" -> "Sigma ",
  "\:03a4" -> "T",
  "\:03a5" -> "Upsilon ",
  "\:03d2" -> "Upsilon ",
  "\:03a6" -> "Phi ",
  "\:03a7" -> "X",
  "\:03a8" -> "Psi ",
  "\:03a9" -> "Omega ",
  "\:03dc" -> "Digamma "
|>;

$typstGreekWords = <|
  "alpha" -> "alpha ", "beta" -> "beta ", "gamma" -> "gamma ", "delta" -> "delta ",
  "epsilon" -> "epsilon ", "zeta" -> "zeta ", "eta" -> "eta ", "theta" -> "theta ",
  "iota" -> "iota ", "kappa" -> "kappa ", "lambda" -> "lambda ", "mu" -> "mu ",
  "nu" -> "nu ", "xi" -> "xi ", "omicron" -> "o", "pi" -> "pi ",
  "rho" -> "rho ", "sigma" -> "sigma ", "tau" -> "tau ", "upsilon" -> "upsilon ",
  "phi" -> "phi ", "chi" -> "chi ", "psi" -> "psi ", "omega" -> "omega ",
  "ALPHA" -> "A", "BETA" -> "B", "GAMMA" -> "Gamma ", "DELTA" -> "Delta ",
  "EPSILON" -> "E", "ZETA" -> "Z", "ETA" -> "H", "THETA" -> "Theta ",
  "IOTA" -> "I", "KAPPA" -> "K", "LAMBDA" -> "Lambda ", "MU" -> "M",
  "NU" -> "N", "XI" -> "Xi ", "OMICRON" -> "O", "PI" -> "Pi ",
  "RHO" -> "P", "SIGMA" -> "Sigma ", "TAU" -> "T", "UPSILON" -> "Upsilon ",
  "PHI" -> "Phi ", "CHI" -> "X", "PSI" -> "Psi ", "OMEGA" -> "Omega "
|>;

(* ------------------------------------------------------------------------ *)
(* Mathematical alphabets (DoubleStruck, Script/Cal, Gothic/Frak, Formal)   *)
(* ------------------------------------------------------------------------ *)

$typstDoubleStruckLetters = Join[
  AssociationThread[
    FromCharacterCode /@ Range[16^^f6e6, 16^^f6ff],
    ("bb(" <> # <> ")") & /@ CharacterRange["a", "z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^f7a4, 16^^f7bd],
    ("bb(" <> # <> ")") & /@ CharacterRange["A", "Z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^f7db, 16^^f7e4],
    ("bb(" <> # <> ")") & /@ CharacterRange["0", "9"]
  ]
];

$typstDoubleStruckShorthands = AssociationThread[
  FromCharacterCode /@ Range[16^^f7a4, 16^^f7bd],
  (# <> # <> " ") & /@ CharacterRange["A", "Z"]
];

makeScriptAlphabet[fn_String] := Join[
  AssociationThread[
    FromCharacterCode /@ Range[16^^f6b2, 16^^f6cb],
    (fn <> "(" <> # <> ")") & /@ CharacterRange["a", "z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^f770, 16^^f789],
    (fn <> "(" <> # <> ")") & /@ CharacterRange["A", "Z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^f7f0, 16^^f7f9],
    (fn <> "(" <> # <> ")") & /@ CharacterRange["0", "9"]
  ],
  <|
    "\:212f" -> fn <> "(e)",
    "\:210a" -> fn <> "(g)",
    "\:2113" -> "ell ",
    "\:2134" -> fn <> "(o)",
    "\:212c" -> fn <> "(B)",
    "\:2130" -> fn <> "(E)",
    "\:2131" -> fn <> "(F)",
    "\:210b" -> fn <> "(H)",
    "\:2110" -> fn <> "(I)",
    "\:2112" -> fn <> "(L)",
    "\:2133" -> fn <> "(M)",
    "\:211b" -> fn <> "(R)"
  |>
];

$typstCaligraphicLetters = makeScriptAlphabet["cal"];
$typstScriptStyleLetters = makeScriptAlphabet["scr"];

$typstGothicLetters = Join[
  AssociationThread[
    FromCharacterCode /@ Range[16^^f6cc, 16^^f6e5],
    ("frak(" <> # <> ")") & /@ CharacterRange["a", "z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^f78a, 16^^f7a3],
    ("frak(" <> # <> ")") & /@ CharacterRange["A", "Z"]
  ],
  <|
    "\:212d" -> "frak(C)",
    "\:210c" -> "frak(H)",
    "\:2111" -> "frak(I)",
    "\:211c" -> "frak(R)",
    "\:2128" -> "frak(Z)"
  |>
];

(* Formal letters (\[FormalA]..\[FormalZ], Formal Greek, FormalScript) *)
$typstFormalLetters = Join[
  AssociationThread[
    FromCharacterCode /@ Range[16^^f800, 16^^f819],
    CharacterRange["a", "z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^f81a, 16^^f833],
    CharacterRange["A", "Z"]
  ],
  <|
    "\:f834" -> "A", "\:f835" -> "B", "\:f836" -> "Gamma ", "\:f837" -> "Delta ",
    "\:f838" -> "E", "\:f839" -> "Z", "\:f83a" -> "H", "\:f83b" -> "Theta ",
    "\:f83c" -> "I", "\:f83d" -> "K", "\:f83e" -> "Lambda ", "\:f83f" -> "M",
    "\:f840" -> "N", "\:f841" -> "Xi ", "\:f842" -> "O", "\:f843" -> "Pi ",
    "\:f844" -> "P", "\:f846" -> "Sigma ", "\:f847" -> "T", "\:f848" -> "Upsilon ",
    "\:f849" -> "Phi ", "\:f84a" -> "X", "\:f84b" -> "Psi ", "\:f84c" -> "Omega ",
    "\:f854" -> "alpha ", "\:f855" -> "beta ", "\:f856" -> "gamma ", "\:f857" -> "delta ",
    "\:f858" -> "epsilon.alt ", "\:f859" -> "zeta ", "\:f85a" -> "eta ", "\:f85b" -> "theta ",
    "\:f85c" -> "iota ", "\:f85d" -> "kappa ", "\:f85e" -> "lambda ", "\:f85f" -> "mu ",
    "\:f860" -> "nu ", "\:f861" -> "xi ", "\:f862" -> "o", "\:f863" -> "pi ",
    "\:f864" -> "rho ", "\:f865" -> "sigma.alt ", "\:f866" -> "sigma ", "\:f867" -> "tau ",
    "\:f868" -> "upsilon ", "\:f869" -> "phi.alt ", "\:f86a" -> "chi ", "\:f86b" -> "psi ",
    "\:f86c" -> "omega ", "\:f874" -> "theta.alt ", "\:f875" -> "Upsilon ",
    "\:f878" -> "phi ", "\:f879" -> "pi.alt ", "\:f87d" -> "Sigma ", "\:f87e" -> "sigma.alt ",
    "\:f87f" -> "Digamma ", "\:f880" -> "digamma ", "\:f881" -> "K", "\:f882" -> "kappa ",
    "\:f883" -> "S", "\:f884" -> "sigma ", "\:f885" -> "kappa.alt ", "\:f886" -> "rho.alt ",
    "\:f88a" -> "epsilon "
  |>,
  AssociationThread[
    FromCharacterCode /@ Range[16^^fa000, 16^^fa019],
    ("cal(" <> # <> ")") & /@ CharacterRange["a", "z"]
  ],
  AssociationThread[
    FromCharacterCode /@ Range[16^^fa01a, 16^^fa033],
    ("cal(" <> # <> ")") & /@ CharacterRange["A", "Z"]
  ]
];

(* ------------------------------------------------------------------------ *)
(* Operators, Relations, Arrows, Misc Symbols (all Typst 0.15.1 verified)   *)
(* ------------------------------------------------------------------------ *)

$typstOperators = <|
  "\:00d7" -> "times ",
  "\:00f7" -> "div ",
  "\:221a" -> "sqrt ",
  "\:f4a0" -> "times ",       (* Cross *)
  "\:00b1" -> "plus.minus ",
  "\:2213" -> "minus.plus ",
  "\:2211" -> "sum ",
  "\:220f" -> "product ",
  "\:2207" -> "nabla ",
  "\:f74c" -> "dif ",         (* DifferentialD *)
  "\:2202" -> "partial ",     (* PartialD *)
  "\:f74b" -> "Dif ",         (* CapitalDifferentialD *)
  "\:222b" -> "integral ",
  "\:222c" -> "integral.double ",
  "\:222d" -> "integral.triple ",
  "\:222e" -> "integral.cont ",
  "\:222f" -> "integral.surf ",
  "\:2230" -> "integral.vol ",
  "\:2232" -> "integral.cw ",
  "\:2233" -> "integral.ccw ",
  "\:f522" -> "-> ",          (* Rule *)
  "\:f51f" -> ":-> ",         (* RuleDelayed *)
  "\:f4a1" -> "|-> ",         (* Function *)
  "\:f120" -> "<-> ",         (* TwoWayRule *)
  "\:f3d4" -> "<-> ",         (* UndirectedEdge *)
  "\:f3d5" -> "-> ",          (* DirectedEdge *)
  "\:00ac" -> "not ",
  "\:2227" -> "and ",
  "\:22bc" -> "\:22bc ",      (* Nand / barwedge *)
  "\:22bd" -> "\:22bd ",      (* Nor *)
  "\:f4a2" -> "\:22bd ",      (* Xnor *)
  "\:2228" -> "or ",
  "\:22bb" -> "xor ",
  "\:f523" -> "=> ",          (* Implies *)
  "\:2200" -> "forall ",
  "\:2203" -> "exists ",
  "\:2204" -> "exists.not ",
  "\:2234" -> "therefore ",
  "\:2235" -> "because ",
  "\:2970" -> "=> ",
  "\:2218" -> "compose ",
  "\:f3de" -> "compose ",     (* PermutationProduct *)
  "\:2295" -> "plus.o ",      (* CirclePlus *)
  "\:2296" -> "minus.o ",     (* CircleMinus *)
  "\:2299" -> "dot.o ",       (* CircleDot *)
  "\:2297" -> "times.o ",     (* CircleTimes *)
  "\:f3da" -> "times.o ",     (* TensorProduct *)
  "\:f3db" -> "and ",         (* TensorWedge *)
  "\:22c4" -> "diamond ",
  "\:00b7" -> "dot.op ",
  "\:22c5" -> "dot.op ",
  "\:2217" -> "ast ",
  "\:22c6" -> "star.op ",
  "\:2240" -> "wreath ",
  "\:2216" -> "without ",
  "\:222a" -> "union ",
  "\:2229" -> "inter ",
  "\:22c0" -> "and.big ",
  "\:22c1" -> "or.big ",
  "\:2322" -> "frown ",
  "\:2323" -> "smile ",
  "\:22c3" -> "union.big ",
  "\:228e" -> "union.plus ",
  "\:22c2" -> "inter.big ",
  "\:2293" -> "inter.sq ",
  "\:2294" -> "union.sq ",
  "\:2210" -> "product.co ",
  "\:f520" -> "square ",      (* Square *)
  "\:f39e" -> "+",            (* ImplicitPlus *)
  "\:f3dc" -> "Pr ",          (* ProbabilityPr *)
  "\:f3dd" -> "bb(E) ",       (* ExpectationE *)
  "\:f3d9" -> "limits(op(\"K\"))", (* ContinuedFractionK *)
  "\:f438" -> "lim ",         (* Limit *)
  "\:f439" -> "limsup ",      (* MaxLimit *)
  "\:f43a" -> "liminf ",      (* MinLimit *)
  "\:f4a3" -> "cal(E) ",      (* DiscreteShift *)
  "\:f4a4" -> "Delta ",       (* DifferenceDelta *)
  "\:f4a5" -> "cal(Q) ",      (* DiscreteRatio *)
  "\:f530" -> " ",            (* Application *)
  "\:2236" -> ":"
|>;

$typstRelations = <|
  "\:2260" -> "!= ",
  "\:f431" -> "== ",
  "\:2261" -> "equiv ",
  "\:2262" -> "equiv.not ",
  "\:2244" -> "tilde.eq.not ",
  "\:2243" -> "tilde.eq ",
  "\:2242" -> "\:2242 ",
  "\:2245" -> "tilde.equiv ",
  "\:2247" -> "tilde.equiv.not ",
  "\:224d" -> "asymp ",
  "\:226d" -> "asymp.not ",
  "\:f400" -> "tilde.eq.not ",
  "\:f401" -> "cancel(approx) ",
  "\:f402" -> "cancel(approx) ",
  "\:226e" -> "lt.not ",
  "<" -> "<",
  ">" -> ">",
  "\:226f" -> "gt.not ",
  "\:2a7d" -> "lt.eq.slant ",
  "\:2a7e" -> "gt.eq.slant ",
  "\:f424" -> "cancel(lt.eq.slant) ",
  "\:f429" -> "cancel(gt.eq.slant) ",
  "\:2270" -> "lt.eq.not ",
  "\:2264" -> "<= ",
  "\:2265" -> ">= ",
  "\:2271" -> "gt.eq.not ",
  "\:2266" -> "lt.equiv ",
  "\:2267" -> "gt.equiv ",
  "\:2280" -> "prec.not ",
  "\:227a" -> "prec ",
  "\:227b" -> "succ ",
  "\:2281" -> "succ.not ",
  "\:227c" -> "prec.eq ",
  "\:227d" -> "succ.eq ",
  "\:2aaf" -> "prec.eq ",
  "\:2ab0" -> "succ.eq ",
  "\:f42b" -> "prec.eq.not ",
  "\:f42d" -> "succ.eq.not ",
  "\:227e" -> "prec.tilde ",
  "\:227f" -> "succ.tilde ",
  "\:f434" -> "succ ",
  "\:f435" -> "succ.eq ",
  "\:f436" -> "prec ",
  "\:f437" -> "prec.eq ",
  "\:2284" -> "subset.not ",
  "\:2282" -> "subset ",
  "\:2283" -> "supset ",
  "\:2285" -> "supset.not ",
  "\:228f" -> "subset.sq ",
  "\:2290" -> "supset.sq ",
  "\:f42e" -> "subset.sq.not ",
  "\:f42f" -> "supset.sq.not ",
  "\:2209" -> "in.not ",
  "\:2208" -> "in ",
  "\:220b" -> "in.rev ",
  "\:220c" -> "in.rev.not ",
  "\:2250" -> "eq.dot ",
  "\:f7d9" -> "=",
  "\:221d" -> "prop ",
  "\:2237" -> "::",
  "\:2241" -> "tilde.not ",
  "\:223c" -> "tilde.op ",
  "\:f3d2" -> "tilde.op ",    (* Distributed *)
  "\:2248" -> "approx ",
  "\:2249" -> "approx.not ",
  "\:226a" -> "lt.double ",
  "\:226b" -> "gt.double ",
  "\:2aa1" -> "lt.double ",
  "\:2aa2" -> "gt.double ",
  "\:f422" -> "cancel(lt.double) ",
  "\:f423" -> "cancel(lt.double) ",
  "\:f427" -> "cancel(gt.double) ",
  "\:f428" -> "cancel(gt.double) ",
  "\:2276" -> "lt.gt ",
  "\:2277" -> "gt.lt ",
  "\:2272" -> "lt.tilde ",
  "\:2273" -> "gt.tilde ",
  "\:22ea" -> "lt.tri.not ",
  "\:22b2" -> "lt.tri ",
  "\:22b3" -> "gt.tri ",
  "\:22eb" -> "gt.tri.not ",
  "\:22ec" -> "lt.tri.eq.not ",
  "\:22b4" -> "lt.tri.eq ",
  "\:22b5" -> "gt.tri.eq ",
  "\:22ed" -> "gt.tri.eq.not ",
  "\:f412" -> "lt.tri.eq.not ",
  "\:f413" -> "gt.tri.eq.not ",
  "\:2288" -> "subset.eq.not ",
  "\:2286" -> "subset.eq ",
  "\:2287" -> "supset.eq ",
  "\:2289" -> "supset.eq.not ",
  "\:22e2" -> "subset.sq.eq.not ",
  "\:2291" -> "subset.sq.eq ",
  "\:2292" -> "supset.sq.eq ",
  "\:22e3" -> "supset.sq.eq.not ",
  "\:f3d1" -> "divides.not ",
  "\:f3d0" -> "divides ",
  "\:2225" -> "parallel ",
  "\:2226" -> "parallel.not ",
  "\:2223" -> "divides ",
  "\:22a7" -> "models ",
  "\:22a9" -> "forces ",
  "\:f432" -> "mid(|)",       (* VerticalSeparator *)
  "\:f3d3" -> "mid(|)"        (* Conditioned *)
|>;

$typstArrows = <|
  "\:2190" -> "<- ",
  "\:2192" -> "-> ",
  "\:2191" -> "arrow.t ",
  "\:2193" -> "arrow.b ",
  "\:2199" -> "arrow.bl ",
  "\:2198" -> "arrow.br ",
  "\:2196" -> "arrow.tl ",
  "\:2197" -> "arrow.tr ",
  "\:f526" -> "<- ",
  "\:f525" -> "-> ",
  "\:f52a" -> "arrow.t ",
  "\:f52b" -> "arrow.b ",
  "\:21d0" -> "arrow.l.double ",
  "\:21d2" -> "=> ",
  "\:21d1" -> "arrow.t.double ",
  "\:21d3" -> "arrow.b.double ",
  "\:21c6" -> "arrows.lr ",
  "\:21c4" -> "arrows.rl ",
  "\:2194" -> "<-> ",
  "\:21d4" -> "<=> ",
  "\:2195" -> "arrow.t.b ",
  "\:21d5" -> "arrow.t.b.double ",
  "\:27f7" -> "<--> ",
  "\:27fa" -> "<==> ",
  "\:27f5" -> "<-- ",
  "\:27f8" -> "<== ",
  "\:27f6" -> "--> ",
  "\:27f9" -> "==> ",
  "\:21bc" -> "harpoon.lt ",
  "\:21c0" -> "harpoon.rt ",
  "\:21bf" -> "harpoon.tl ",
  "\:21c3" -> "harpoon.bl ",
  "\:21bd" -> "harpoon.lb ",
  "\:21c1" -> "harpoon.rb ",
  "\:21c2" -> "harpoon.br ",
  "\:21be" -> "harpoon.tr ",
  "\:21cb" -> "harpoons.ltrb ",
  "\:21cc" -> "harpoons.rtlb ",
  "\:21a6" -> "|-> ",
  "\:21a4" -> "<-| ",
  "\:22a8" -> "tack.r.double ",
  "\:22a3" -> "tack.l ",
  "\:22a2" -> "tack.r ",
  "\:22a5" -> "bot ",
  "\:22a4" -> "top "
|>;

$typstMiscSymbols = <|
  "\:f7da" -> "c",            (* ConstantC *)
  "\:f74d" -> "e",            (* ExponentialE *)
  "\:f74e" -> "i",            (* ImaginaryI *)
  "\:f74f" -> "j",            (* ImaginaryJ *)
  "\:f749" -> "pi ",          (* DoubledPi *)
  "\:f74a" -> "gamma ",       (* DoubledGamma *)
  "\:221e" -> "infinity ",
  "\:00b5" -> "mu ",
  "\:2127" -> "Omega.inv ",
  "\:212b" -> "angstrom ",
  "\:210f" -> "planck ",
  "\:2300" -> "emptyset ",
  "\:2135" -> "aleph ",
  "\:2136" -> "beth ",
  "\:2137" -> "gimel ",
  "\:2138" -> "daleth ",
  "\:2118" -> "pee ",         (* Weierstrass p *)
  "\:f11d" -> "sha ",         (* Shah *)
  "\:2205" -> "emptyset ",
  "\:00b0" -> "degree ",
  "\:2220" -> "angle ",
  "\:221f" -> "angle.right ",
  "\:2221" -> "angle.arc ",
  "\:2222" -> "angle.spheric ",
  "\:2032" -> "'",
  "\:2033" -> "''",
  "\:2035" -> "prime.rev ",
  "\:2036" -> "prime.rev prime.rev ",
  "\:2026" -> "dots.h ",
  "\:f758" -> "dots.h ",      (* TripleDot *)
  "\:22ef" -> "dots.h.c ",
  "\:22ee" -> "dots.v ",
  "\:22f0" -> "dots.up ",
  "\:22f1" -> "dots.down ",
  "\:2022" -> "bullet ",
  "\:2020" -> "dagger ",
  "\:2021" -> "dagger.double ",
  "\:2212" -> "-",
  "\:2013" -> "--",
  "\:2014" -> "---",
  "\:2010" -> "-",
  "\:266d" -> "flat ",
  "\:266e" -> "natural ",
  "\:266f" -> "sharp ",
  "\:25a0" -> "square.filled ",
  "\:f750" -> "bullet ",      (* FilledSmallCircle *)
  "\:f751" -> "square.stroked ", (* DottedSquare *)
  "\:f752" -> "square.filled ",  (* GraySquare *)
  "\:f753" -> "circle.filled ",  (* GrayCircle *)
  "\:f527" -> "square.filled ",  (* SelectionPlaceholder *)
  "\:f528" -> "square.stroked ", (* Placeholder *)
  "\:f3a4" -> "square.stroked ", (* AutoPlaceholder *)
  "\:25cf" -> "circle.filled ",
  "\:25b2" -> "triangle.filled.t ",
  "\:25bc" -> "triangle.filled.b ",
  "\:25c0" -> "triangle.filled.l ",
  "\:25b6" -> "triangle.filled.r ",
  "\:25a1" -> "square.stroked ",
  "\:25e6" -> "circle.small ",
  "\:25cb" -> "circle.stroked ",
  "\:25b3" -> "triangle.stroked.t ",
  "\:25bd" -> "triangle.stroked.b ",
  "\:25c7" -> "diamond.stroked ",
  "\:25c6" -> "diamond.filled ",
  "\:2605" -> "star.filled ",
  "\:2662" -> "suit.diamond.stroked ",
  "\:2663" -> "suit.club.filled ",
  "\:2661" -> "suit.heart.stroked ",
  "\:2660" -> "suit.spade.filled ",
  "\:2713" -> "checkmark ",
  "\:f3c7" -> "upright(T)",   (* Transpose *)
  "\:f3c8" -> "*",            (* Conjugate *)
  "\:f3c9" -> "dagger ",      (* ConjugateTranspose *)
  "\:f3ce" -> "dagger ",      (* HermitianConjugate *)
  "\:f700" -> "dotless.j ",   (* DotlessJ *)
  "\:f730" -> "dotless.i ",   (* ScriptDotlessI *)
  "\:f731" -> "dotless.j ",   (* ScriptDotlessJ *)
  "\:f724" -> "\\#",          (* NumberSign *)
  "\:f7fc" -> ",",            (* NumberComma *)
  "\:f754" -> "\\_",          (* LetterSpace *)
  "\:f755" -> "frown ",       (* DownBreve *)
  "\:f757" -> "",             (* MathematicaIcon *)
  "\:f603" -> "|",            (* LeftBracketingBar (unpaired fallback) *)
  "\:f604" -> "|",            (* RightBracketingBar (unpaired fallback) *)
  "\:f605" -> "bar.v.double ", (* LeftDoubleBracketingBar (unpaired fallback) *)
  "\:f606" -> "bar.v.double ", (* RightDoubleBracketingBar (unpaired fallback) *)
  "\:f76b" -> "[",            (* LeftModified *)
  "\:f76c" -> "]",            (* RightModified *)
  "\:f761" -> "chevron.l.double ", (* LeftSkeleton *)
  "\:f762" -> "chevron.r.double ", (* RightSkeleton *)
  "\:f113" -> "chevron.l bar.v ",  (* LeftAssociation *)
  "\:f114" -> "bar.v chevron.r ",  (* RightAssociation *)
  "\:f361" -> "{",            (* Piecewise (fallback) *)
  "\:fe35" -> "overparen",
  "\:fe36" -> "underparen",
  "\:fe37" -> "overbrace",
  "\:fe38" -> "underbrace"
|>;

$typstDefaultCharMap = Join[
  $typstBasicEscapes,
  $typstSpaces,
  $typstGreekLetters,
  $typstDoubleStruckLetters,
  $typstCaligraphicLetters,
  $typstGothicLetters,
  $typstFormalLetters,
  $typstOperators,
  $typstRelations,
  $typstArrows,
  $typstMiscSymbols
];

(* ------------------------------------------------------------------------ *)
(* Built-in & Custom Typst math operators (aligned with Typst 0.15.1 math)  *)
(* ------------------------------------------------------------------------ *)

$typstBuiltinOperators = {
  "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth",
  "csc", "csch", "ctg", "deg", "det", "dim", "exp", "gcd", "hom", "id",
  "im", "inf", "ker", "lcm", "lg", "lim", "liminf", "limsup", "ln", "log",
  "max", "min", "mod", "Pr", "sec", "sech", "sin", "sinc", "sinh", "sup",
  "tan", "tanh", "tg", "tr", "Re", "Im"
};

$typstCustomOperators = {
  "arccot", "arcsec", "arccsc", "arcsinh", "arccosh", "arctanh",
  "arccoth", "arcsech", "arccsch", "sgn", "Tr", "Res",
  "Si", "Ci", "Shi", "Chi", "Ei", "li", "Li",
  "erf", "erfc", "erfi", "Erf", "Erfc", "Erfi",
  "Ai", "Bi", "gd", "sn", "cn", "dn", "am",
  "cs", "ds", "ns", "sc", "dc", "nc", "sd", "cd", "nd",
  "sech", "csch", "Arg", "Max", "Min"
};

$typstDefaultOperatorMap = Join[
  AssociationThread[$typstBuiltinOperators, $typstBuiltinOperators],
  AssociationMap["op(\"" <> # <> "\")" &, $typstCustomOperators],
  <|
    "Arg" -> "arg",
    "Max" -> "max",
    "Min" -> "min",
    "Tr" -> "tr"
  |>
];

(* ------------------------------------------------------------------------ *)
(* Delimiters                                                               *)
(* ------------------------------------------------------------------------ *)

$typstLeftDelimiters = <|
  "(" -> "(",
  "[" -> "[",
  "\:f76b" -> "[",
  "\:301a" -> "bracket.l.stroked",
  "{" -> "{",
  "\:f361" -> "{",
  "\:230a" -> "floor.l",
  "\:2308" -> "ceil.l",
  "\:2329" -> "chevron.l",
  "\:27e8" -> "chevron.l",
  "\:f761" -> "chevron.l.double",
  "\:00ab" -> "chevron.l.double",
  "\:f603" -> "|",
  "\:f605" -> "bar.v.double",
  "\:f113" -> "chevron.l bar.v",
  "|" -> "|"
|>;

$typstRightDelimiters = <|
  ")" -> ")",
  "]" -> "]",
  "\:f76c" -> "]",
  "\:301b" -> "bracket.r.stroked",
  "}" -> "}",
  "\:230b" -> "floor.r",
  "\:2309" -> "ceil.r",
  "\:232a" -> "chevron.r",
  "\:27e9" -> "chevron.r",
  "\:f762" -> "chevron.r.double",
  "\:00bb" -> "chevron.r.double",
  "\:f604" -> "|",
  "\:f606" -> "bar.v.double",
  "\:f114" -> "bar.v chevron.r",
  "|" -> "|"
|>;

$typstMatchingDelimiter = <|
  "(" -> ")",
  "[" -> "]",
  "\:f76b" -> "\:f76c",
  "\:301a" -> "\:301b",
  "{" -> "}",
  "\:230a" -> "\:230b",
  "\:2308" -> "\:2309",
  "\:2329" -> "\:232a",
  "\:27e8" -> "\:27e9",
  "\:f761" -> "\:f762",
  "\:00ab" -> "\:00bb",
  "\:f603" -> "\:f604",
  "\:f605" -> "\:f606",
  "\:f113" -> "\:f114",
  "|" -> "|"
|>;

(* Delimiter pairs that map to dedicated Typst math functions in "Semantic" mode *)
$typstSemanticDelimiterFunctions = <|
  "\:f603" -> "abs",
  "\:f605" -> "norm",
  "\:230a" -> "floor",
  "\:2308" -> "ceil"
|>;

(* Delimiter pairs that fold cleanly into Typst mat(delim: ..., ...) *)
$typstMatrixDelimiterSpec = <|
  "(" -> Automatic,       (* Typst mat default is "(" *)
  "[" -> "\"[\"",
  "\:f76b" -> "\"[\"",
  "{" -> "\"{\"",
  "|" -> "\"|\"",
  "\:f603" -> "\"|\"",
  "\:f605" -> "bar.v.double"
|>;

(* ------------------------------------------------------------------------ *)
(* Dynamic map builders & character lookup                                  *)
(* ------------------------------------------------------------------------ *)

buildTypstCharMap[cfg_Association] := Module[
  {overrides = <||>, constStyle, diffStyle, transpSym, bbStyle, scriptStyle, userRules},
  constStyle = Lookup[cfg, "ConstantStyle", "Italic"];
  diffStyle = Lookup[cfg, "DifferentialStyle", "Upright"];
  transpSym = Lookup[cfg, "TransposeSymbol", "upright(T)"];
  bbStyle = Lookup[cfg, "BlackboardStyle", "Function"];
  scriptStyle = Lookup[cfg, "ScriptStyle", "Calligraphic"];
  userRules = normalizeRuleAssoc[Lookup[cfg, "CharacterRules", <||>]];

  If[constStyle === "Upright",
    overrides["\:f74d"] = "upright(e)";
    overrides["\:f74e"] = "upright(i)";
    overrides["\:f74f"] = "upright(j)";
  ];
  If[diffStyle === "Italic",
    overrides["\:f74c"] = "d ";
    overrides["\:f74b"] = "D ";
  ];
  If[StringQ[transpSym] && transpSym =!= "upright(T)",
    overrides["\:f3c7"] = transpSym;
  ];
  If[bbStyle === "Shorthand",
    overrides = Join[overrides, $typstDoubleStruckShorthands];
  ];
  If[scriptStyle === "Script",
    overrides = Join[overrides, $typstScriptStyleLetters];
  ];

  If[Length[overrides] === 0 && Length[userRules] === 0,
    $typstDefaultCharMap,
    Join[$typstDefaultCharMap, overrides, userRules]
  ]
];

buildTypstOperatorMap[cfg_Association] := Module[{userOps, extra = <||>},
  userOps = Lookup[cfg, "OperatorNames", {}];
  If[AssociationQ[userOps],
    extra = userOps,
    Scan[
      Function[item,
        Which[
          StringQ[item], extra[item] = "op(\"" <> escapeTypstString[item] <> "\")",
          MatchQ[item, (_Rule | _RuleDelayed)] && StringQ[First[item]] && StringQ[Last[item]],
            extra[First[item]] = Last[item]
        ]
      ],
      Flatten[{userOps}]
    ]
  ];
  If[Length[extra] === 0,
    $typstDefaultOperatorMap,
    Join[$typstDefaultOperatorMap, extra]
  ]
];

escapeTypstString[s_String] :=
  StringReplace[s, {"\\" -> "\\\\", "\"" -> "\\\""}];

puaCodeQ[code_Integer] :=
  (16^^e000 <= code <= 16^^f8ff) || (16^^f0000 <= code <= 16^^10ffff);

handleUnknownPuaChar[c_String] := Module[{code, hex, handler, fallback},
  code = First[ToCharacterCode[c]];
  hex = ToUpperCase[IntegerString[code, 16, If[code > 16^^ffff, 6, 4]]];
  handler = Lookup[$typstActiveConfig, "UnknownCharacterHandler", Automatic];
  Which[
    handler === "Ignore", "",
    handler === "Unicode", "#\"\\u{" <> hex <> "}\"",
    handler === Automatic,
      Message[TypstForm::unkchar, ToString[c, CharacterEncoding -> "PrintableASCII"], "U+" <> hex];
      fallback = ToString[c, CharacterEncoding -> "PrintableASCII"];
      If[StringMatchQ[fallback, "\\[" ~~ ___ ~~ "]"],
        "\"" <> escapeTypstString[StringTake[fallback, {3, -2}]] <> "\"",
        "\"" <> escapeTypstString[fallback] <> "\""
      ],
    True,
      With[{res = handler[c]},
        If[StringQ[res], res, ToString[res]]
      ]
  ]
];

mapTypstChar[c_String] := Module[{mapped, code},
  (* Context-sensitive punctuation inside mat(...) / cases(...) / Quantity *)
  If[c === "," && (TrueQ[$typstInMatrixCell] || TrueQ[$typstInCasesCell]),
    Return["\\, ", Module]
  ];
  If[c === ";" && TrueQ[$typstInMatrixCell],
    Return["\\; ", Module]
  ];
  If[c === "/" && TrueQ[$typstInQuantityUnit],
    Return["\\/", Module]
  ];

  mapped = Lookup[$typstCharMap, c, Missing["NotFound"]];
  If[!MissingQ[mapped],
    Return[mapped, Module]
  ];

  (* Guard against unmapped Private Use Area characters leaking to Typst *)
  code = First[ToCharacterCode[c]];
  If[puaCodeQ[code],
    Return[handleUnknownPuaChar[c], Module]
  ];

  c
];

End[];
