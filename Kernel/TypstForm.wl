(* ::Package:: *)

(* TypstForm: convert Wolfram Language expressions and Box structures into
   Typst math expressions, modeled after System`Convert`TeXFormDump`. *)

Begin["`TypstForm`"];

(* ------------------------------------------------------------------------ *)
(* Character and symbol lookup tables                                       *)
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

$typstSpaces = <|
  " " -> " ",
  "\t" -> "quad ",
  "\n" -> "",
  "\:00a0" -> "",       (* NonBreakingSpace *)
  "\:2005" -> "thick ", (* FourPerEmSpace *)
  "\:2009" -> "thin ",  (* ThinSpace *)
  "\:200a" -> "thin ",  (* HairSpace *)
  "\:2028" -> "",
  "\:2029" -> "",
  "\:205f" -> "med ",   (* MediumMathSpace *)
  "\:2060" -> "",       (* WordJoiner / NoBreak *)
  "\:f360" -> "",       (* Null / AutoSpace *)
  "\:f380" -> "",
  "\:f382" -> "",       (* NegativeThinSpace *)
  "\:f383" -> "",       (* NegativeMediumSpace *)
  "\:f384" -> "",       (* NegativeThickSpace *)
  "\:f3a3" -> "",       (* InvisibleSpace *)
  "\:f3ad" -> " ",      (* MediumSpace *)
  "\:f3b3" -> "thin ",  (* VeryThinSpace *)
  "\:f3b4" -> " ",      (* ThickSpace *)
  "\:f760" -> "",       (* InvisibleComma *)
  "\:f761" -> "",       (* InvisibleApplication *)
  "\:f76d" -> "",       (* InvisibleTimes *)
  "\:f765" -> "",       (* InvisiblePlus *)
  "\:f3a0" -> "",
  "\:f3a8" -> "",
  "\:f3a9" -> "",
  "\:f3ae" -> "",
  "\:f3bc" -> "",
  "\:f3ba" -> "",
  "\:f3bb" -> ""
|>;

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
  "\:03dc" -> "F"
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

$typstDoubleStruckLetters = <|
  "\:f6e6" -> "bb(a)", "\:f6e7" -> "bb(b)", "\:f6e8" -> "bb(c)", "\:f6e9" -> "bb(d)",
  "\:f6ea" -> "bb(e)", "\:f6eb" -> "bb(f)", "\:f6ec" -> "bb(g)", "\:f6ed" -> "bb(h)",
  "\:f6ee" -> "bb(i)", "\:f6ef" -> "bb(j)", "\:f6f0" -> "bb(k)", "\:f6f1" -> "bb(l)",
  "\:f6f2" -> "bb(m)", "\:f6f3" -> "bb(n)", "\:f6f4" -> "bb(o)", "\:f6f5" -> "bb(p)",
  "\:f6f6" -> "bb(q)", "\:f6f7" -> "bb(r)", "\:f6f8" -> "bb(s)", "\:f6f9" -> "bb(t)",
  "\:f6fa" -> "bb(u)", "\:f6fb" -> "bb(v)", "\:f6fc" -> "bb(w)", "\:f6fd" -> "bb(x)",
  "\:f6fe" -> "bb(y)", "\:f6ff" -> "bb(z)",
  "\:f7a4" -> "bb(A)", "\:f7a5" -> "bb(B)", "\:f7a6" -> "bb(C)", "\:f7a7" -> "bb(D)",
  "\:f7a8" -> "bb(E)", "\:f7a9" -> "bb(F)", "\:f7aa" -> "bb(G)", "\:f7ab" -> "bb(H)",
  "\:f7ac" -> "bb(I)", "\:f7ad" -> "bb(J)", "\:f7ae" -> "bb(K)", "\:f7af" -> "bb(L)",
  "\:f7b0" -> "bb(M)", "\:f7b1" -> "bb(N)", "\:f7b2" -> "bb(O)", "\:f7b3" -> "bb(P)",
  "\:f7b4" -> "bb(Q)", "\:f7b5" -> "bb(R)", "\:f7b6" -> "bb(S)", "\:f7b7" -> "bb(T)",
  "\:f7b8" -> "bb(U)", "\:f7b9" -> "bb(V)", "\:f7ba" -> "bb(W)", "\:f7bb" -> "bb(X)",
  "\:f7bc" -> "bb(Y)", "\:f7bd" -> "bb(Z)",
  "\:f7db" -> "bb(0)", "\:f7dc" -> "bb(1)", "\:f7dd" -> "bb(2)", "\:f7de" -> "bb(3)",
  "\:f7df" -> "bb(4)", "\:f7e0" -> "bb(5)", "\:f7e1" -> "bb(6)", "\:f7e2" -> "bb(7)",
  "\:f7e3" -> "bb(8)", "\:f7e4" -> "bb(9)"
|>;

$typstCaligraphicLetters = <|
  "\:f6b2" -> "cal(a)", "\:f6b3" -> "cal(b)", "\:f6b4" -> "cal(c)", "\:f6b5" -> "cal(d)",
  "\:212f" -> "cal(e)", "\:f6b7" -> "cal(f)", "\:210a" -> "cal(g)", "\:f6b9" -> "cal(h)",
  "\:f6ba" -> "cal(i)", "\:f6bb" -> "cal(j)", "\:f6bc" -> "cal(k)", "\:2113" -> "ell ",
  "\:f6be" -> "cal(m)", "\:f6bf" -> "cal(n)", "\:2134" -> "cal(o)", "\:f6c1" -> "cal(p)",
  "\:f6c2" -> "cal(q)", "\:f6c3" -> "cal(r)", "\:f6c4" -> "cal(s)", "\:f6c5" -> "cal(t)",
  "\:f6c6" -> "cal(u)", "\:f6c7" -> "cal(v)", "\:f6c8" -> "cal(w)", "\:f6c9" -> "cal(x)",
  "\:f6ca" -> "cal(y)", "\:f6cb" -> "cal(z)",
  "\:f770" -> "cal(A)", "\:212c" -> "cal(B)", "\:f772" -> "cal(C)", "\:f773" -> "cal(D)",
  "\:2130" -> "cal(E)", "\:2131" -> "cal(F)", "\:f776" -> "cal(G)", "\:210b" -> "cal(H)",
  "\:2110" -> "cal(I)", "\:f779" -> "cal(J)", "\:f77a" -> "cal(K)", "\:2112" -> "cal(L)",
  "\:2133" -> "cal(M)", "\:f77d" -> "cal(N)", "\:f77e" -> "cal(O)", "\:f77f" -> "cal(P)",
  "\:f780" -> "cal(Q)", "\:211b" -> "cal(R)", "\:f782" -> "cal(S)", "\:f783" -> "cal(T)",
  "\:f784" -> "cal(U)", "\:f785" -> "cal(V)", "\:f786" -> "cal(W)", "\:f787" -> "cal(X)",
  "\:f788" -> "cal(Y)", "\:f789" -> "cal(Z)"
|>;

$typstGothicLetters = <|
  "\:f6cc" -> "frak(a)", "\:f6cd" -> "frak(b)", "\:f6ce" -> "frak(c)", "\:f6cf" -> "frak(d)",
  "\:f6d0" -> "frak(e)", "\:f6d1" -> "frak(f)", "\:f6d2" -> "frak(g)", "\:f6d3" -> "frak(h)",
  "\:f6d4" -> "frak(i)", "\:f6d5" -> "frak(j)", "\:f6d6" -> "frak(k)", "\:f6d7" -> "frak(l)",
  "\:f6d8" -> "frak(m)", "\:f6d9" -> "frak(n)", "\:f6da" -> "frak(o)", "\:f6db" -> "frak(p)",
  "\:f6dc" -> "frak(q)", "\:f6dd" -> "frak(r)", "\:f6de" -> "frak(s)", "\:f6df" -> "frak(t)",
  "\:f6e0" -> "frak(u)", "\:f6e1" -> "frak(v)", "\:f6e2" -> "frak(w)", "\:f6e3" -> "frak(x)",
  "\:f6e4" -> "frak(y)", "\:f6e5" -> "frak(z)",
  "\:f78a" -> "frak(A)", "\:f78b" -> "frak(B)", "\:212d" -> "frak(C)", "\:f78d" -> "frak(D)",
  "\:f78e" -> "frak(E)", "\:f78f" -> "frak(F)", "\:f790" -> "frak(G)", "\:210c" -> "frak(H)",
  "\:2111" -> "frak(I)", "\:f793" -> "frak(J)", "\:f794" -> "frak(K)", "\:f795" -> "frak(L)",
  "\:f796" -> "frak(M)", "\:f797" -> "frak(N)", "\:f798" -> "frak(O)", "\:f799" -> "frak(P)",
  "\:f79a" -> "frak(Q)", "\:211c" -> "frak(R)", "\:f79c" -> "frak(S)", "\:f79d" -> "frak(T)",
  "\:f79e" -> "frak(U)", "\:f79f" -> "frak(V)", "\:f7a0" -> "frak(W)", "\:f7a1" -> "frak(X)",
  "\:f7a2" -> "frak(Y)", "\:2128" -> "frak(Z)"
|>;

$typstOperators = <|
  "\:00d7" -> "times ",
  "\:00f7" -> "div ",
  "\:221a" -> "sqrt ",
  "\:f4a0" -> "times ",
  "\:00b1" -> "plus.minus ",
  "\:2213" -> "minus.plus ",
  "\:2211" -> "sum ",
  "\:220f" -> "product ",
  "\:2207" -> "nabla ",
  "\:f74c" -> "dif ",
  "\:2202" -> "diff ",
  "\:f74b" -> "Dif ",
  "\:222b" -> "integral ",
  "\:222e" -> "integral.cont ",
  "\:222f" -> "integral.surf ",
  "\:2230" -> "integral.vol ",
  "\:2232" -> "integral.cw ",
  "\:2233" -> "integral.ccw ",
  "\:f522" -> "-> ",
  "\:f51f" -> ":-> ",
  "\:00ac" -> "not ",
  "\:2227" -> "and ",
  "\:22bc" -> "barwedge ",
  "\:2228" -> "or ",
  "\:22bb" -> "xor ",
  "\:f523" -> "=> ",
  "\:2200" -> "forall ",
  "\:2203" -> "exists ",
  "\:2204" -> "exists.not ",
  "\:2234" -> "therefore ",
  "\:2235" -> "because ",
  "\:2970" -> "=> ",
  "\:2218" -> "compose ",
  "\:2295" -> "plus.circle ",
  "\:2296" -> "minus.circle ",
  "\:2299" -> "dot.circle ",
  "\:2297" -> "times.circle ",
  "\:22c4" -> "diamond ",
  "\:00b7" -> "dot.op ",
  "\:22c6" -> "star.op ",
  "\:2240" -> "wreath ",
  "\:2216" -> "without ",
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
  "\:f520" -> "square ",
  "\:2236" -> ":"
|>;

$typstRelations = <|
  "\:2260" -> "!= ",
  "\:f431" -> "== ",
  "\:2261" -> "equiv ",
  "\:2262" -> "equiv.not ",
  "\:2244" -> "tilde.eq.not ",
  "\:2243" -> "tilde.eq ",
  "\:2245" -> "tilde.equiv ",
  "\:2247" -> "tilde.equiv.not ",
  "\:226e" -> "lt.not ",
  "<" -> "<",
  ">" -> ">",
  "\:226f" -> "gt.not ",
  "\:2a7d" -> "lt.eq.slant ",
  "\:2a7e" -> "gt.eq.slant ",
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
  "\:227e" -> "prec.tilde ",
  "\:227f" -> "succ.tilde ",
  "\:2284" -> "subset.not ",
  "\:2282" -> "subset ",
  "\:2283" -> "supset ",
  "\:2285" -> "supset.not ",
  "\:228f" -> "subset.sq ",
  "\:2290" -> "supset.sq ",
  "\:2209" -> "in.not ",
  "\:2208" -> "in ",
  "\:220b" -> "in.rev ",
  "\:220c" -> "in.rev.not ",
  "\:2250" -> "dot.eq ",
  "\:f7d9" -> "=",
  "\:221d" -> "prop ",
  "\:2237" -> "::",
  "\:2241" -> "tilde.not ",
  "\:223c" -> "tilde.op ",
  "\:2248" -> "approx ",
  "\:2249" -> "approx.not ",
  "\:226a" -> "lt.double ",
  "\:226b" -> "gt.double ",
  "\:2aa1" -> "lt.double ",
  "\:2aa2" -> "gt.double ",
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
  "\:2223" -> "divides "
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
  "\:f7da" -> "c",
  "\:f74d" -> "e",
  "\:f74e" -> "i",
  "\:f74f" -> "j",
  "\:f749" -> "pi ",
  "\:f74a" -> "gamma ",
  "\:221e" -> "infinity ",
  "\:00b5" -> "mu ",
  "\:2127" -> "ohm.inv ",
  "\:212b" -> "angstrom ",
  "\:210f" -> "planck.reduce ",
  "\:2300" -> "emptyset ",
  "\:2135" -> "aleph ",
  "\:2136" -> "beth ",
  "\:2137" -> "gimel ",
  "\:2138" -> "daleth ",
  "\:2118" -> "wp ",
  "\:2205" -> "emptyset ",
  "\:00b0" -> "degree ",
  "\:2220" -> "angle ",
  "\:221f" -> "angle.right ",
  "\:2221" -> "angle.measured ",
  "\:2222" -> "angle.spheric ",
  "\:2032" -> "'",
  "\:2033" -> "''",
  "\:2035" -> "prime.rev ",
  "\:2036" -> "prime.rev prime.rev ",
  "\:2026" -> "dots.h ",
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
  "\:f750" -> "bullet ",
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
  "\:f3c7" -> "upright(T)",
  "\:f3c8" -> "*",
  "\:f3c9" -> "dagger ",
  "\:f757" -> ""
|>;

$typstAllCharRules = Join[
  $typstBasicEscapes,
  $typstSpaces,
  $typstGreekLetters,
  $typstDoubleStruckLetters,
  $typstCaligraphicLetters,
  $typstGothicLetters,
  $typstOperators,
  $typstRelations,
  $typstArrows,
  $typstMiscSymbols
];

(* Native Typst math operators that render upright without op(...) *)
$typstBuiltinOperators = {
  "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth",
  "csc", "csch", "deg", "det", "dim", "exp", "gcd", "hom", "inf",
  "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min",
  "Pr", "sec", "sech", "sin", "sinh", "sup", "tan", "tanh", "Re", "Im",
  "mod"
};

(* Wolfram TraditionalForm function names that need op("...") in Typst *)
$typstCustomOperators = {
  "arccot", "arcsec", "arccsc", "arcsinh", "arccosh", "arctanh",
  "arccoth", "arcsech", "arccsch", "lcm", "sgn", "Tr", "Res",
  "Si", "Ci", "Shi", "Chi", "Erf", "Erfc", "Ai", "Bi", "Li"
};

(* Delimiters *)
$typstLeftDelimiters = <|
  "(" -> "(",
  "[" -> "[",
  "\:f603" -> "[", (* LeftBracketingBar-like *)
  "\:301a" -> "bracket.l.stroked ",
  "{" -> "{",
  "\:f361" -> "{", (* LeftPiecewise *)
  "\:230a" -> "floor.l ",
  "\:2308" -> "ceil.l ",
  "\:2329" -> "chevron.l ",
  "\:f601" -> "chevron.l chevron.l ",
  "\:00ab" -> "chevron.l chevron.l ",
  "\:f605" -> "|| ",
  "|" -> "|"
|>;

$typstRightDelimiters = <|
  ")" -> ")",
  "]" -> "]",
  "\:f604" -> "]",
  "\:301b" -> "bracket.r.stroked ",
  "}" -> "}",
  "\:230b" -> "floor.r ",
  "\:2309" -> "ceil.r ",
  "\:232a" -> "chevron.r ",
  "\:f602" -> "chevron.r chevron.r ",
  "\:00bb" -> "chevron.r chevron.r ",
  "\:f606" -> "|| ",
  "|" -> "|"
|>;

$typstLeftDelimPattern = Alternatives @@ Keys[$typstLeftDelimiters];
$typstRightDelimPattern = Alternatives @@ Keys[$typstRightDelimiters];
$typstMatchingDelimiter = <|
  "(" -> ")",
  "[" -> "]",
  "\:301a" -> "\:301b",
  "{" -> "}",
  "\:230a" -> "\:230b",
  "\:2308" -> "\:2309",
  "\:2329" -> "\:232a",
  "\:f601" -> "\:f602",
  "\:00ab" -> "\:00bb",
  "\:f603" -> "\:f604",
  "\:f605" -> "\:f606",
  "|" -> "|"
|>;

(* ------------------------------------------------------------------------ *)
(* Helper predicates and utilities                                          *)
(* ------------------------------------------------------------------------ *)

delimiterBoxQ[boxes_List] := !FreeQ[boxes,
  GridBox | SuperscriptBox | SubscriptBox | SubsuperscriptBox |
  OverscriptBox | UnderscriptBox | UnderoverscriptBox |
  FractionBox | SqrtBox | RadicalBox | FrameBox | ButtonBox | PaneBox | PanelBox
];
delimiterBoxQ[boxes___] := delimiterBoxQ[{boxes}];

stripIdentityBoxes[boxes_] := boxes //. {
  RowBox[{x_}] :> x,
  InterpretationBox[TemplateBox[_, "NumericalApproximation", ___], x_, ___] :>
    Block[{BoxForm`UseApproximations = False}, MakeBoxes[x, TraditionalForm]],
  InterpretationBox[DynamicModuleBox[_, TemplateBox[args_, "IconizedObject", ___], ___], __] :>
    makeIconizedObject[args],
  (AdjustmentBox | InterpretationBox | TagBox | DynamicBox | TooltipBox)[x_, ___] :> x
};

makeIconizedObject[args_] := Replace[args, {
  {_, StyleBox[custom_, "IconizedCustomName", ___], ___} :> custom,
  {_, "ListIcon", ___} :> RowBox[{"{", "\:2026", "}"}],
  {_, "AssociationIcon", ___} :> RowBox[{"<|", "\:2026", "|>"}],
  {_, "StringIcon", ___} :> "\"\:2026\"",
  {_, "SequenceIcon", ___} :> "\:2026",
  {_, other_, ___} :> (other //. DynamicBox[FEPrivate`FrontEndResource["FEBitmaps", "IconizeEllipsis"], ___] :> "\:2026"),
  _ :> "\:2026"
}];

testNextNumber[RowBox[{}]] := False;
testNextNumber[elem_String] := StringMatchQ[elem, NumberString];
testNextNumber[(RowBox | SuperscriptBox | SubscriptBox | OverscriptBox |
  UnderscriptBox | SubsuperscriptBox | UnderoverscriptBox | StyleBox |
  AdjustmentBox | InterpretationBox | TagBox | FormBox | Cell |
  BoxData | TextData | TooltipBox | ValueBox | OptionValueBox)[elem_, ___]] :=
  testNextNumber[elem];
testNextNumber[_] := False;

(* Separate adjacent numbers like 2 3 -> 2 med 3 so Typst does not merge them *)
fixWhitespace[elem_List] := Module[{n = Length[elem]},
  Table[
    If[elem[[i]] === " " && i > 1 && i < n &&
        StringQ[elem[[i - 1]]] && StringMatchQ[elem[[i - 1]], NumberString] &&
        testNextNumber[elem[[i + 1]]],
      "\:205f",
      elem[[i]]
    ],
    {i, n}
  ]
];

(* Wrap script expressions in (...) unless they are a single alphanumeric char *)
makeScript[in_] := Module[{s = StringTrim[makeTypst[in]]},
  Which[
    s === "", "()",
    StringMatchQ[s, WordCharacter] && StringLength[s] === 1, s,
    StringMatchQ[s, RegularExpression["[A-Za-z]+\\.?[A-Za-z]*"]], s,
    StringMatchQ[s, "(" ~~ ___ ~~ ")"] && balancedParensQ[s], s,
    True, "(" <> s <> ")"
  ]
];

balancedParensQ[s_String] := Module[{chars = Characters[s], depth = 0, ok = True, i},
  For[i = 1, i <= Length[chars], i++,
    Switch[chars[[i]],
      "(", depth++,
      ")", depth--
    ];
    If[depth < 0 || (depth === 0 && i < Length[chars]),
      ok = False; Break[]
    ]
  ];
  ok && depth === 0
];

(* Escape a string for inclusion inside Typst double quotes "..." *)
escapeTypstString[s_String] :=
  StringReplace[s, {"\\" -> "\\\\", "\"" -> "\\\""}];

(* Convert a multi-character Wolfram identifier or string token *)
makeString[str_String] := Module[{mapped},
  Which[
    str === " ", " ",
    StringMatchQ[str, NumberString], str,
    StringLength[str] === 1, makeTypstChar[str],
    MemberQ[$typstBuiltinOperators, str], str <> " ",
    MemberQ[$typstCustomOperators, str], "op(\"" <> str <> "\") ",
    KeyExistsQ[$typstGreekWords, str], $typstGreekWords[str],
    StringMatchQ[str, "\"" ~~ ___ ~~ "\""],
      "\"" <> escapeTypstString[StringTake[str, {2, -2}]] <> "\"",
    True,
      (* Check if the string is purely ASCII letters/digits (e.g. a multi-letter symbol) *)
      If[StringMatchQ[str, RegularExpression["[A-Za-z][A-Za-z0-9]*"]],
        "\"" <> str <> "\"",
        mapped = StringJoin[makeTypstChar /@ Characters[str]];
        mapped
      ]
  ]
];

makeTypstChar[c_String] := Lookup[$typstAllCharRules, c, c];

joinTypstTokens[tokens_List] := Fold[
  Function[{acc, tok},
    Which[
      tok === "", acc,
      acc === "", tok,
      StringMatchQ[StringTake[acc, -1], WordCharacter] &&
        StringMatchQ[StringTake[tok, 1], LetterCharacter],
        acc <> " " <> tok,
      True,
        acc <> tok
    ]
  ],
  "",
  tokens
];

toNumberForm[str_String] :=
  makeTypst[ToBoxes[NumberForm[ToExpression[str], NumberPadding -> {"", ""}], TraditionalForm]];

(* ------------------------------------------------------------------------ *)
(* Core Box -> Typst rules                                                  *)
(* ------------------------------------------------------------------------ *)

(* Interactive / UI boxes *)
makeTypst[CheckBox[False]] := makeTypstChar["\:25a1"];
makeTypst[CheckBox[True]] := "\:2611";
makeTypst[CheckBox[Dynamic[expr_]]] := makeTypst[CheckBox[expr]];
makeTypst[OpenerBox[False]] := makeTypstChar["\:25b6"];
makeTypst[OpenerBox[True]] := makeTypstChar["\:25bc"];
makeTypst[OpenerBox[Dynamic[expr_]]] := makeTypst[OpenerBox[expr]];
makeTypst[RadioButtonBox[Dynamic[expr_], val_]] := makeTypst[RadioButtonBox[expr, val]];
makeTypst[RadioButtonBox[val_, val_]] := "dot.circle ";
makeTypst[RadioButtonBox[_, _]] := "circle.stroked ";
makeTypst[TooltipBox[boxes_, ___]] := makeTypst[boxes];
makeTypst[ErrorBox[boxes_, ___]] := "underline(" <> makeTypst[boxes] <> ")";
makeTypst[AdjustmentBox[boxes_, ___]] := makeTypst[boxes];
makeTypst[InterpretationBox[boxes_, ___]] := makeTypst[boxes];
makeTypst[TagBox[boxes_, ___]] := makeTypst[boxes];
makeTypst[DynamicBox[boxes_, ___]] := makeTypst[boxes];
makeTypst[(AnimatorBox | SliderBox)[___]] := (Message[TypstForm::unspt, "Animator/Slider"]; "");
makeTypst[InputFieldBox[boxes_, opts___]] := makeTypst[boxes];
makeTypst[PopupMenuBox[val_, {labels__?OptionQ}, ___]] := makeTypst[val /. {labels}];
makeTypst[PopupMenuBox[val_, ___]] := makeTypst[ToBoxes[val]];
makeTypst[PaneSelectorBox[rules_, Dynamic[expr_], opts___]] :=
  makeTypst[PaneSelectorBox[rules, expr, opts]];
makeTypst[PaneSelectorBox[{rules__?OptionQ}, val_, ___]] := makeTypst[val /. {rules}];
makeTypst[(PaneBox | PanelBox | FrameBox)[boxes_, ___]] := makeTypst[boxes];
makeTypst[ButtonBox[boxes_, ___]] := makeTypst[boxes];

(* Document / Container boxes *)
makeTypst[Notebook[{cell_}, ___]] := makeTypst[cell];
makeTypst[Notebook[cells_List, ___]] := StringRiffle[makeTypst /@ cells, " \\\n\\\n"];
makeTypst[BoxData[boxes_List]] := StringRiffle[makeTypst /@ boxes, " \\\n"];
makeTypst[BoxData[boxes_]] := makeTypst[boxes];
makeTypst[TextData[text_]] := makeTypst[RowBox[Flatten[{text}]]];
makeTypst[FormBox[boxes_, form_, ___]] := makeTypst[boxes];
makeTypst[TemplateArgBox[lst_List]] := makeTypst[RowBox[lst]];
makeTypst[TemplateArgBox[expr_]] := makeTypst[expr];

(* StyleBox and Cell *)
makeTypst[(StyleBox | Cell)[boxes_, "Input" | "InputOnly" | "InlineInput", ___]] :=
  "bold(" <> makeTypst[boxes] <> ")";
makeTypst[(StyleBox | Cell)[boxes_, "IT" | "TI" | "MO" | "SB", ___]] :=
  "italic(" <> makeTypst[boxes] <> ")";
makeTypst[(StyleBox | Cell)[boxes_, "BF" | "TB" | "MB", ___]] :=
  "bold(" <> makeTypst[boxes] <> ")";
makeTypst[(StyleBox | Cell)[boxes_, "TBI" | "MBO" | "SBO", ___]] :=
  "bold(italic(" <> makeTypst[boxes] <> "))";
makeTypst[(StyleBox | Cell)[str_, sty_String : "", opts__?OptionQ]] := Module[
  {fv, und, fw, fs, inner},
  fv = FontVariations /. {opts} /. FontVariations -> {};
  und = TrueQ["Underline" /. fv /. "Underline" -> False];
  {fw, fs} = {FontWeight, FontSlant} /. {opts} /. {FontWeight | FontSlant -> "Plain"};
  inner = makeTypst[StyleBox[str, sty]];
  If[fs === "Italic", inner = "italic(" <> inner <> ")"];
  If[und, inner = "underline(" <> inner <> ")"];
  If[fw === "Bold", inner = "bold(" <> inner <> ")"];
  inner
];
makeTypst[(StyleBox | Cell)[boxes_, ___]] := makeTypst[boxes];

(* TemplateBox *)
makeTypst[TemplateBox[{n_, k_}, "Binomial", ___]] :=
  "binom(" <> makeTypst[n] <> ", " <> makeTypst[k] <> ")";
makeTypst[TemplateBox[{n_, m_, q_}, "QBinomial", ___]] :=
  "binom(" <> makeTypst[n] <> ", " <> makeTypst[m] <> ")_" <> makeScript[q];
makeTypst[TemplateBox[{expr_, var_, val_}, "Limit2Arg", ___]] :=
  "lim_(" <> makeTypst[var] <> " -> " <> makeTypst[val] <> ") " <> makeTypst[expr];
makeTypst[TemplateBox[{base_, super_}, "Superscript", ___]] :=
  makeTypst[SuperscriptBox[base, super]];
makeTypst[tbox_TemplateBox, opts___] :=
  makeTypst[stripIdentityBoxes[BoxForm`TemplateBoxToDisplayBoxes[tbox]]];

(* Strings *)
makeTypst[str_String /; StringMatchQ[str, "\"" ~~ LetterCharacter ~~ "\""]] :=
  "\"" <> StringTake[str, {2, 2}] <> "\"";
makeTypst[str_String /; StringMatchQ[str, NumberString ~~ "`*^" ~~ DigitCharacter..]] :=
  toNumberForm[str];
makeTypst[str_String /; StringMatchQ[str, NumberString ~~ "`"]] :=
  toNumberForm[str];
makeTypst[str_String /; StringMatchQ[str, NumberString ~~ "`" ~~ NumberString]] :=
  toNumberForm[str];
makeTypst[str_String /; StringLength[str] === 1] := makeTypstChar[str];
makeTypst[str_String] :=
  makeString[StringReplace[str, {"\\\"" -> "\"", "\"" -> ""}]];

(* Special RowBox patterns *)
makeTypst[RowBox[{"T", AdjustmentBox["E", ___], "X"}]] := "\"TeX\"";
makeTypst[RowBox[{"L", StyleBox[AdjustmentBox["A", ___], ___], "T", AdjustmentBox["E", ___], "X"}]] := "\"LaTeX\"";

(* Piecewise: GridBox[{{"\[Piecewise]", GridBox[...]}}, ...] or RowBox[{"\[Piecewise]", GridBox[...]}] *)
formatPiecewise[grid_List] := Module[{rows},
  rows = Map[StringRiffle[makeTypst /@ #, " & "] &, grid];
  "cases(" <> StringRiffle[rows, ", "] <> ")"
];
makeTypst[GridBox[{{"\:f361", GridBox[grid_List, ___]}}, ___?OptionQ]] :=
  formatPiecewise[grid];
makeTypst[RowBox[{"\:f361", GridBox[grid_List, ___]}]] :=
  formatPiecewise[grid];

(* Integrals *)
makeTypst[RowBox[{"\:222b", RowBox[{integrand_, RowBox[{"\:f74c", var_}]}]}]] :=
  "integral " <> makeTypst[integrand] <> " thin dif " <> makeTypst[var];
makeTypst[RowBox[{SubsuperscriptBox["\:222b", sub_, super_, ___], RowBox[{integrand_, RowBox[{"\:f74c", var_}]}]}]] :=
  "integral_" <> makeScript[sub] <> "^" <> makeScript[super] <> " " <>
  makeTypst[integrand] <> " thin dif " <> makeTypst[var];

(* Special named functions *)
makeTypst[RowBox[{"Im", "(", rest___, ")"}]] := "Im" <> makeTypst[RowBox[{"(", rest, ")"}]];
makeTypst[RowBox[{"Re", "(", rest___, ")"}]] := "Re" <> makeTypst[RowBox[{"(", rest, ")"}]];
makeTypst[RowBox[{"Arg", "(", rest___, ")"}]] := "arg" <> makeTypst[RowBox[{"(", rest, ")"}]];
makeTypst[RowBox[{"Max", "(", rest___, ")"}]] := "max" <> makeTypst[RowBox[{"(", rest, ")"}]];
makeTypst[RowBox[{"Min", "(", rest___, ")"}]] := "min" <> makeTypst[RowBox[{"(", rest, ")"}]];

(* mod *)
makeTypst[RowBox[{l__, " ", "mod", " ", r__}]] :=
  "(" <> makeTypst[RowBox[{l}]] <> " mod " <> makeTypst[RowBox[{r}]] <> ")";

(* Comments *)
makeTypst[RowBox[{"(*", c1___String, RowBox[{c2___String}], c3___String, "*)"}]] :=
  "\"(*" <> escapeTypstString[StringJoin[c1, c2, c3]] <> "*)\"";

(* Linear stack scan for the first properly matched nearest delimiter pair
   that requires lr(...) or structured grouping, avoiding greedy cross-pair
   matches such as (a) + (b) and exponential pattern backtracking. *)
findMatchingDelimiterSpan[elem_List] := Module[{stack = {}, i, tok, openIdx, openTok, mid},
  For[i = 1, i <= Length[elem], i++,
    tok = elem[[i]];
    If[StringQ[tok],
      If[Length[stack] > 0 && Lookup[$typstMatchingDelimiter, stack[[-1, 2]], None] === tok,
        {openIdx, openTok} = stack[[-1]];
        stack = Most[stack];
        mid = Take[elem, {openIdx + 1, i - 1}];
        If[openTok === "{" || !MemberQ[{"(", "["}, openTok] || delimiterBoxQ[mid],
          Return[{openIdx, i}, Module]
        ],
        If[KeyExistsQ[$typstMatchingDelimiter, tok],
          AppendTo[stack, {i, tok}]
        ]
      ]
    ]
  ];
  None
];

(* General RowBox with balanced delimiter pairing *)
makeTypst[RowBox[elem_List]] := Module[{span, openIdx, closeIdx, lb, rb, l, mid, r, ld, rd, inner, wrapped},
  span = findMatchingDelimiterSpan[elem];
  If[span === None,
    joinTypstTokens[makeTypst /@ fixWhitespace[elem]],
    {openIdx, closeIdx} = span;
    lb = elem[[openIdx]];
    rb = elem[[closeIdx]];
    l = Take[elem, openIdx - 1];
    mid = Take[elem, {openIdx + 1, closeIdx - 1}];
    r = Drop[elem, closeIdx];
    ld = $typstLeftDelimiters[lb];
    rd = $typstRightDelimiters[rb];
    inner = makeTypst[RowBox[mid]];
    wrapped = If[MemberQ[{"(", "["}, lb] && MemberQ[{")", "]"}, rb],
      ld <> inner <> rd,
      "lr(" <> ld <> " " <> inner <> " " <> rd <> ")"
    ];
    joinTypstTokens[{
      If[l === {}, "", makeTypst[RowBox[l]]],
      wrapped,
      If[r === {}, "", makeTypst[RowBox[r]]]
    }]
  ]
];

(* Superscripts and Subscripts *)
makeTypst[SuperscriptBox[base_, super_String, ___]] /;
  Complement[Union[Characters[super]], {"'", "\:2032"}] === {} :=
  makeTypst[base] <> StringRepeat["'", StringLength[super]];

makeTypst[SuperscriptBox[base_, super_, ___]] :=
  makeTypst[base] <> "^" <> makeScript[super];

makeTypst[SuperscriptBox[base_, ___?OptionQ]] := makeTypst[base];

makeTypst[SubscriptBox[base_, sub_, ___]] := Module[{b = makeTypst[base]},
  If[!StringFreeQ[b, "^"],
    "attach(" <> b <> ", b: " <> makeTypst[sub] <> ")",
    b <> "_" <> makeScript[sub]
  ]
];

makeTypst[SubsuperscriptBox[base_, sub_, super_, ___]] :=
  makeTypst[base] <> "_" <> makeScript[sub] <> "^" <> makeScript[super];

makeTypst[SubsuperscriptBox[base_, ___?OptionQ]] := makeTypst[base];

(* OverscriptBox *)
makeTypst[OverscriptBox[base_String /; StringLength[base] == 1, "_", ___]] :=
  "bar(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "_" | "\:2500", ___]] :=
  "overline(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "\:2190" | "\:27f5" | "\:f526", ___]] :=
  "arrow.l(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "\:2192" | "\:27f6" | "\:f525" | "\:21c0", ___]] :=
  "arrow(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "\:2194" | "\:27f7", ___]] :=
  "arrow.l.r(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "^", ___]] :=
  "hat(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "\:02c7", ___]] :=
  "caron(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "~" | "\:223c", ___]] :=
  "tilde(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "'" | "\:2032", ___]] :=
  "acute(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "`" | "\:2035", ___]] :=
  "grave(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "\:02d8", ___]] :=
  "breve(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "." | "\:2022" | "\:00b7", ___]] :=
  "dot(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, ".." | "\:2022\:2022" | "\:00b7\:00b7" | "\:00a8", ___]] :=
  "dot.double(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "..." | "\:2022\:2022\:2022" | "\:00b7\:00b7\:00b7" | "\:f758" | "\:2026" | "\:22ef", ___]] :=
  "dot.triple(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[base_, "...." | "\:2022\:2022\:2022\:2022" | "\:00b7\:00b7\:00b7\:00b7" | "\:00a8\:00a8", ___]] :=
  "dot.quad(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[OverscriptBox[base_, "\:fe37", ___], script_, ___]] :=
  "overbrace(" <> makeTypst[base] <> ", " <> makeTypst[script] <> ")";
makeTypst[OverscriptBox[base_, OverscriptBox["\:fe37", script_, ___], ___]] :=
  "overbrace(" <> makeTypst[base] <> ", " <> makeTypst[script] <> ")";
makeTypst[OverscriptBox[base_, "\:fe37", ___]] :=
  "overbrace(" <> makeTypst[base] <> ")";
makeTypst[OverscriptBox[sp : "\:2211" | "\:220f", over_, ___]] :=
  makeTypst[sp] <> "^" <> makeScript[over] <> " ";
makeTypst[OverscriptBox[base_, over_, ___]] :=
  "attach(" <> makeTypst[base] <> ", t: " <> makeTypst[over] <> ")";

(* UnderscriptBox *)
makeTypst[UnderscriptBox[base_, "_" | "\:2500", ___]] :=
  "underline(" <> makeTypst[base] <> ")";
makeTypst[UnderscriptBox[base_, "\:2194" | "\:27f7", ___]] :=
  "attach(" <> makeTypst[base] <> ", b: arrow.l.r)";
makeTypst[UnderscriptBox[base_, "\:2192" | "\:27f6" | "\:f525", ___]] :=
  "attach(" <> makeTypst[base] <> ", b: arrow.r)";
makeTypst[UnderscriptBox[UnderscriptBox[base_, "\:fe38", ___], script_, ___]] :=
  "underbrace(" <> makeTypst[base] <> ", " <> makeTypst[script] <> ")";
makeTypst[UnderscriptBox[base_, UnderscriptBox["\:fe38", script_, ___], ___]] :=
  "underbrace(" <> makeTypst[base] <> ", " <> makeTypst[script] <> ")";
makeTypst[UnderscriptBox[base_, "\:fe38", ___]] :=
  "underbrace(" <> makeTypst[base] <> ")";
makeTypst[UnderscriptBox[sp : "\:2211" | "\:220f", under_, ___]] :=
  makeTypst[sp] <> "_" <> makeScript[under] <> " ";
makeTypst[UnderscriptBox["lim", under_, ___]] :=
  "lim_" <> makeScript[under] <> " ";
makeTypst[UnderscriptBox[base_, under_, ___]] :=
  "attach(" <> makeTypst[base] <> ", b: " <> makeTypst[under] <> ")";

(* UnderoverscriptBox *)
makeTypst[UnderoverscriptBox[sp : "\:2211" | "\:220f", under_, over_, ___]] :=
  makeTypst[sp] <> "_" <> makeScript[under] <> "^" <> makeScript[over] <> " ";
makeTypst[UnderoverscriptBox[base_, under_, over_, ___]] :=
  "attach(" <> makeTypst[base] <> ", b: " <> makeTypst[under] <> ", t: " <> makeTypst[over] <> ")";

(* Sqrt, Radical, Fraction *)
makeTypst[SqrtBox[x_, ___]] := "sqrt(" <> makeTypst[x] <> ")";
makeTypst[RadicalBox[x_, y_, ___]] := "root(" <> makeTypst[y] <> ", " <> makeTypst[x] <> ")";
makeTypst[FractionBox[num_, den_, ___]] := "frac(" <> makeTypst[num] <> ", " <> makeTypst[den] <> ")";

(* GridBox -> Typst mat(delim: #none, ...) *)
makeTypst[GridBox[grid_List, opts___?OptionQ]] := Module[{rows},
  rows = Map[StringRiffle[makeTypst /@ #, ", "] &, grid];
  "mat(delim: #none, " <> StringRiffle[rows, "; "] <> ")"
];

(* List fallback *)
makeTypst[lst_List] := makeTypst[RowBox[lst]];

(* Unsupported fallback *)
makeTypst[expr_] := (Message[TypstForm::unspt, expr]; "");

(* ------------------------------------------------------------------------ *)
(* Public entry points                                                      *)
(* ------------------------------------------------------------------------ *)

Options[TypstForm] = {"Form" -> TraditionalForm};
Options[MathemeToTypst] = Options[TypstForm];
Options[BoxesToTypst] = {};

BoxesToTypst[boxes_, OptionsPattern[]] :=
  StringTrim[makeTypst[stripIdentityBoxes[boxes]]];

MathemeToTypst[expr_, OptionsPattern[]] := With[{form = OptionValue["Form"]},
  BoxesToTypst[MakeBoxes[expr, form]]
];

Format[TypstForm[expr_, opts : OptionsPattern[]], OutputForm] :=
  MathemeToTypst[expr, opts];

TypstForm /: MakeBoxes[t : TypstForm[expr_, opts : OptionsPattern[]], StandardForm | TraditionalForm] :=
  With[{str = MathemeToTypst[expr, opts]},
    InterpretationBox[StyleBox[str, "Output", ShowStringCharacters -> False], t]
  ];

End[];
