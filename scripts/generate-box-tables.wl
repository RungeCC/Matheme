(* ::Package:: *)

(* Generate static lookup tables for the Rust Box -> Typst converter from the
   Wolfram Language source of truth in Kernel/TypstForm/. Run with:
   wolframscript -file scripts/generate-box-tables.wl

   Values that are WL symbols (e.g. Automatic, Left, Right) are exported as
   their symbol names so Rust can dispatch on them as strings. *)

root = FileNameJoin[{DirectoryName[$InputFileName], ".."}];
Get[FileNameJoin[{root, "Kernel", "Matheme.wl"}]];
ctx = "Runge`Matheme`Private`TypstForm`";

toStr[x_Symbol] := SymbolName[x];
toStr[x_] := x;

table[name_String] := Module[{value},
  value = Symbol[ctx <> name];
  Which[
    AssociationQ[value], AssociationThread[Keys[value], toStr /@ Values[value]],
    ListQ[value], toStr /@ value,
    True, Throw[CreateFailure["TableNotFound", <|"Name" -> name|>]]
  ]
];

names = {
  "$typstDefaultCharMap", "$typstDefaultOperatorMap", "$typstGreekWords",
  "$typstMatchingDelimiter", "$typstSemanticDelimiterFunctions",
  "$typstMatrixDelimiterSpec", "$typstLeftDelimiters", "$typstRightDelimiters",
  "$typstInfixSpacedOps", "$typstNoLeadingSpaceBefore", "$typstNoTrailingSpaceAfter"
};

tables = AssociationThread[names, table /@ names];

(* Staleness guard: tables.json is a snapshot of the WL sources above, and a
   stale snapshot makes the two engines diverge silently. The MD5 of the
   source files (in this exact order, ":"-separated, lowercase 32-digit hex)
   is written as "$typstTableSourceHash"; scripts/build.nu compares it against
   the current files and refuses to build a stale snapshot. *)
sourceFiles = {
  FileNameJoin[{root, "Kernel", "TypstForm", "Characters.wl"}],
  FileNameJoin[{root, "Kernel", "TypstForm", "Boxes.wl"}]
};
sourceHash = StringRiffle[
  IntegerString[FileHash[#, "MD5"], 16, 32] & /@ sourceFiles, ":"];

(* ------------------------------------------------------------------------ *)
(* JSON writer                                                              *)
(* ------------------------------------------------------------------------ *)
(* Export[..., "JSON"] silently drops private-use and format characters, which
   is exactly what box trees are full of (\[InvisibleApplication], markers, ...).
   Everything is therefore escaped by hand and the file is pure ASCII. *)

jsonChar[c_String] := Module[{code = First[ToCharacterCode[c]]},
  Which[
    c === "\"" || c === "\\", "\\" <> c,
    c === "\n", "\\n", c === "\r", "\\r", c === "\t", "\\t",
    code < 32 || code > 126,
      If[code <= 65535,
        StringJoin["\\u", IntegerString[code, 16, 4]],
        Module[{v = code - 65536},
          StringJoin["\\u", IntegerString[55296 + Quotient[v, 1024], 16, 4],
            "\\u", IntegerString[56320 + Mod[v, 1024], 16, 4]]]
      ],
    True, c
  ]
];

jsonString[s_String] := "\"" <> StringJoin[jsonChar /@ Characters[s]] <> "\"";

jsonText[s_String] := jsonString[s];
jsonText[n_Integer | n_Real] := ToString[n];
jsonText[a_Association] :=
  "{" <> StringRiffle[(jsonString[#] <> ": " <> jsonText[a[#]]) & /@ Keys[a], ", "] <> "}";
jsonText[l_List] := "[" <> StringRiffle[jsonText /@ l, ", "] <> "]";

(* Box trees keep the symbol/string distinction and use `[head, args...]`. *)
boxJson[s_String] := jsonString[s];
boxJson[n_Integer | n_Real] := ToString[n];
boxJson[s_Symbol] := "[" <> jsonString["$sym"] <> ", " <> jsonString[SymbolName[s]] <> "]";
boxJson[e_] :=
  "[" <> StringRiffle[boxJson /@ Prepend[List @@ e, SymbolName[Head[e]]], ", "] <> "]";

(* ------------------------------------------------------------------------ *)
(* Semantic TemplateBox expansions                                          *)
(* ------------------------------------------------------------------------ *)
(* A TemplateBox tag is a pointer to a front-end template, not something the
   Rust renderer can interpret. The reference converter resolves it with
   BoxForm`TemplateBoxToDisplayBoxes, so exporting exactly those expansions as
   data keeps both engines in agreement without a hand-maintained tag list. *)

stripBoxes = Symbol[ctx <> "stripIdentityBoxes"];

(* Stand-ins for the template arguments. They survive the expansion verbatim
   and are substituted back by the Rust renderer. *)
marker[i_Integer] := "\:f8f0" <> ToString[i] <> "\:f8f1";

encodeBox = boxJson;

boxCall[h_Symbol, n_Integer] := Quiet@Check[
  With[{v = h @@ Table[Unique["q"], {n}]}, MakeBoxes[v, TraditionalForm]],
  $Failed];

(* Structural tags (Inactive wrappers, quantity, piecewise, ...) never come from
   a NumericFunction head, so a few representative expressions are boxed too. *)
$templateProbeExpressions = {
  Inactive[Sin][x], Inactive[f][x], Inactive[f][x, y], Inactive[ArcSin][x],
  Quantity[1, "Meters"], Quantity[1, "Meters"]/Quantity[2, "Seconds"],
  Piecewise[{{x, x > 0}}, 0], Interval[{0, 1}], Root[#^3 - # &, 1],
  SeriesData[x, 0, {1, 2}, 0, 2, 1], MatrixForm[{{1, 2}, {3, 4}}],
  Defer[Sin[x]], Around[1, 0.1],
  (* Held so the templates survive evaluation of matrix operations. *)
  HoldForm[Inverse[{{a, b}, {c, d}}]], HoldForm[Conjugate[x]],
  HoldForm[Transpose[{{a, b}}]], HoldForm[Det[{{a, b}, {c, d}}]],
  HoldForm[Norm[{a, b}]]
};

tagPairs[] := Module[{numeric, boxed, fromHeads, fromProbes},
  numeric = Select[Names["System`*"],
    MemberQ[Attributes[Evaluate[#]], NumericFunction] &];
  fromHeads = Reap[
      Do[
        Do[
          Module[{b = boxCall[Evaluate[Symbol[h]], n]},
            Cases[b, TemplateBox[a_List, t_String, ___] :> Sow[t -> Length[a]],
              {0, Infinity}]],
          {n, {1, 2, 3, 4}}],
        {h, numeric}]][[2]];
  fromProbes = Reap[
      Do[
        boxed = Quiet@Check[With[{v = e}, MakeBoxes[v, TraditionalForm]], $Failed];
        Cases[boxed, TemplateBox[a_List, t_String, ___] :> Sow[t -> Length[a]],
          {0, Infinity}],
        {e, $templateProbeExpressions}]][[2]];
  DeleteDuplicates[Flatten[Join[fromHeads, fromProbes], 1]]
];

makeExpansion[tag_String, n_Integer] := Module[{boxes},
  boxes = Quiet@Check[
    BoxForm`TemplateBoxToDisplayBoxes[TemplateBox[Table[marker[i], {i, n}], tag]],
    $Failed];
  If[boxes === $Failed || MatchQ[boxes, _TemplateBox] ||
      Head[boxes] === BoxForm`TemplateBoxToDisplayBoxes,
    $Failed,
    encodeBox[stripBoxes[boxes]]
  ]
];

pairs = tagPairs[];
encoded = Map[Function[p, {p[[1]] <> "/" <> ToString[p[[2]]], makeExpansion[p[[1]], p[[2]]]}], pairs];
encoded = Select[encoded, StringQ[#[[2]]] &];

(* ------------------------------------------------------------------------ *)
(* Cross-check against the front end's own resources                        *)
(* ------------------------------------------------------------------------ *)
(* The expansions above come from probing the kernel, which says nothing about
   whether a tag still has a front-end definition or whether a character still
   has a name. Core.nb holds the template definitions as StyleData cells and
   UnicodeCharacters.tr holds the character names; both are read (never
   written) so a divergence is reported here instead of surfacing as a silently
   unsupported template deep inside the converter. *)

(* The front end's own resources live next to the kernel inside the
   installation tree, so they are addressed relative to
   $InstallationDirectory -- never by a version- or platform-specific path. *)
frontEndResource[parts__String] :=
  FileNameJoin[{$InstallationDirectory, "SystemFiles", "FrontEnd", parts}];

coreNB = frontEndResource["StyleSheets", "Core.nb"];
unicodeTR = frontEndResource["TextResources", "UnicodeCharacters.tr"];
probedTags = DeleteDuplicates[pairs[[All, 1]]];

If[FileExistsQ[coreNB],
  Module[{nb, coreTags, missing},
    nb = Get[coreNB];
    coreTags = DeleteDuplicates@Cases[nb,
       Cell[StyleData[tag_String, ___], opts___] /;
          ! FreeQ[HoldComplete[opts], TemplateBoxOptions] :> tag,
       {0, Infinity}];
    missing = Complement[probedTags, coreTags];
    Print["Core.nb: ", Length[coreTags], " tags defined, ", Length[probedTags],
      " probed, ", Length[missing], " without a definition",
      If[missing === {}, "", ": " <> ToString[missing]]]],
  Print["Core.nb: not found at ", coreNB, ", tag cross-check skipped"]
];

(* One line looks like: 0x03B1 <tab> \\[Alpha] <tab> ($a$   $alpha$) <tab> Letter *)
parseTR[line_String] := Module[{code, name},
  code = First[StringCases[line, "0x" ~~ h : Repeated[HexadecimalCharacter, {4, 6}] :> h], ""];
  name = First[StringCases[line, "\\[" ~~ n : Except["]" | "\\"] .. ~~ "]" :> n], ""];
  If[code === "" || name === "", Nothing, FromDigits[code, 16] -> name]
];

If[FileExistsQ[unicodeTR],
  Module[{byCode, keys, unnamed, private, standard},
    byCode = Association[parseTR /@ Import[unicodeTR, {"Text", "Lines"}]];
    keys = Select[Keys[tables["$typstDefaultCharMap"]], StringLength[#] === 1 &];
    unnamed = Select[keys, ! KeyExistsQ[byCode, First[ToCharacterCode[#]]] &];
    (* Wolfram's own private-use block is deliberately absent from the table,
       so only standard code points are worth reporting. *)
    private = Select[unnamed, 16^^F000 <= First[ToCharacterCode[#]] <= 16^^F8FF &];
    standard = Complement[unnamed, private];
    Print["UnicodeCharacters.tr: ", Length[byCode], " named characters, ",
      Length[keys], " single-character keys, ", Length[unnamed], " unnamed (",
      Length[private], " private-use, ", Length[standard], " standard",
      If[standard === {}, "",
        ": " <> StringRiffle[
          ("U+" <> IntegerString[First[ToCharacterCode[#]], 16, 4]) & /@ standard, ", "]],
      ")"]],
  Print["UnicodeCharacters.tr: not found at ", unicodeTR, ", name cross-check skipped"]
];

templateJson = "{" <> StringRiffle[
    (jsonString[#[[1]]] <> ": " <> #[[2]]) & /@ encoded, ", "] <> "}";

baseJson = jsonText[tables];
out = FileNameJoin[{root, "crates", "matheme-box", "src", "tables.json"}];
stream = OpenWrite[out];
WriteString[stream, StringDrop[baseJson, -1] <>
  ",\n\"$typstTableSourceHash\": " <> jsonString[sourceHash] <>
  ",\n\"$typstTemplateExpansions\": " <> templateJson <> "}\n"];
Close[stream];

Print["wrote ", out, " with ", Length[tables], " tables, ",
  Length[encoded], " of ", Length[pairs], " template tags expanded"];
Print["source hash: ", sourceHash];
