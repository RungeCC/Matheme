//! Golden outputs for the WL parity corpus.
//!
//! `tests/wolfram/TypstFormNativeParityTests.wl` compares the native renderer
//! with the WL reference on the same box trees; self-consistency alone cannot
//! catch the two engines drifting together. The cases here pin every parity
//! case to an explicit expected string, and the WL suite carries the same
//! expectations. When a golden changes deliberately, update both files in the
//! same commit.

use matheme_box::{Config, boxes_to_typst_with_config};
use matheme_wl::{Expr, Rule, Symbol, TypstTemplate};

fn system(head: &str) -> Symbol {
    Symbol::new(format!("System`{head}"))
}

fn normal(head: &str, args: Vec<Expr>) -> Expr {
    Expr::normal(system(head), args)
}

fn list(args: Vec<Expr>) -> Expr {
    normal("List", args)
}

fn row(items: Vec<Expr>) -> Expr {
    normal("RowBox", vec![list(items)])
}

fn string(value: &str) -> Expr {
    Expr::string(value)
}

fn template(args: &[&str], tag: &str) -> Expr {
    normal(
        "TemplateBox",
        vec![
            list(args.iter().map(|value| string(value)).collect()),
            string(tag),
        ],
    )
}

fn switches(entries: impl IntoIterator<Item = Rule>) -> Expr {
    Expr::association([Rule::new("Switches", Expr::association(entries))])
}

fn chars(entries: impl IntoIterator<Item = Rule>) -> Expr {
    Expr::association([Rule::new("CharOverrides", Expr::association(entries))])
}

fn operators(entries: impl IntoIterator<Item = Rule>) -> Expr {
    Expr::association([Rule::new("OperatorOverrides", Expr::association(entries))])
}

fn templates(entries: impl IntoIterator<Item = Rule>) -> Expr {
    Expr::association([Rule::new("TemplateOverrides", Expr::association(entries))])
}

fn typst_template(body: &str) -> Expr {
    Expr::normal(Symbol::new(TypstTemplate::WIRE_HEAD), [string(body)])
}

fn render(boxes: Expr, patch: Expr) -> String {
    let config = Config::from_expr(&patch).unwrap();
    boxes_to_typst_with_config(&boxes, &config).unwrap()
}

#[test]
fn fraction_style_slash() {
    let boxes = normal("FractionBox", vec![string("1"), string("2")]);
    let patch = switches([Rule::new("FractionStyle", "Slash")]);
    assert_eq!(render(boxes, patch), "(1)/(2)");
}

#[test]
fn conjugate_style_overline() {
    let boxes = template(&["x"], "Conjugate");
    let patch = switches([Rule::new("ConjugateStyle", "Overline")]);
    assert_eq!(render(boxes, patch), "overline(x)");
}

#[test]
fn transpose_symbol_template() {
    let boxes = template(&["x"], "Transpose");
    let patch = switches([Rule::new("TransposeSymbol", "t")]);
    assert_eq!(render(boxes, patch), "x^t");
}

#[test]
fn transpose_symbol_character() {
    // Production sends this as a `CharOverrides` entry: the WL side folds
    // `TransposeSymbol` into the effective char map before building the patch.
    let boxes = row(vec![string("\u{f3c7}")]);
    let patch = chars([Rule::new(string("\u{f3c7}"), string("t"))]);
    assert_eq!(render(boxes, patch), "t");
}

#[test]
fn multi_letter_style_italic() {
    let boxes = row(vec![string("abc")]);
    let patch = switches([Rule::new("MultiLetterStyle", "Italic")]);
    assert_eq!(render(boxes, patch), "italic(\"abc\")");
}

#[test]
fn multi_letter_style_upright() {
    let boxes = row(vec![string("abc")]);
    let patch = switches([Rule::new("MultiLetterStyle", "Upright")]);
    assert_eq!(render(boxes, patch), "upright(\"abc\")");
}

#[test]
fn strip_context_false() {
    let boxes = row(vec![string("MyPkg`x")]);
    let patch = switches([Rule::new("StripContext", "False")]);
    assert_eq!(render(boxes, patch), "\"MyPkg`x\"");
}

#[test]
fn preserve_style_false() {
    let boxes = normal(
        "StyleBox",
        vec![
            string("x"),
            Expr::normal(
                system("Rule"),
                [
                    Expr::symbol(system("FontWeight")),
                    Expr::symbol(system("Bold")),
                ],
            ),
        ],
    );
    let patch = switches([Rule::new("PreserveStyle", "False")]);
    assert_eq!(render(boxes, patch), "x");
}

#[test]
fn delimiter_style_explicit_row() {
    let boxes = row(vec![string("\u{230a}"), string("x"), string("\u{230b}")]);
    let patch = switches([Rule::new("DelimiterStyle", "Explicit")]);
    assert_eq!(render(boxes, patch), "lr(floor.l x floor.r)");
}

#[test]
fn delimiter_style_explicit_template() {
    let boxes = template(&["x"], "Abs");
    let patch = switches([Rule::new("DelimiterStyle", "Explicit")]);
    assert_eq!(render(boxes, patch), "lr(|x|)");
}

#[test]
fn matrix_delimiter_explicit() {
    let grid = normal(
        "GridBox",
        vec![list(vec![
            list(vec![string("a"), string("b")]),
            list(vec![string("c"), string("d")]),
        ])],
    );
    let boxes = row(vec![string("("), grid, string(")")]);
    let patch = switches([Rule::new("MatrixDelimiter", "\"[\"")]);
    assert_eq!(render(boxes, patch), "(mat(delim: #none, a, b; c, d))");
}

#[test]
fn constant_style_upright() {
    let boxes = row(vec![string("\u{f74d}")]);
    let patch = chars([Rule::new(string("\u{f74d}"), string("upright(e)"))]);
    assert_eq!(render(boxes, patch), "upright(e)");
}

#[test]
fn differential_style_italic() {
    let boxes = row(vec![string("\u{f74c}")]);
    let patch = chars([Rule::new(string("\u{f74c}"), string("d "))]);
    assert_eq!(render(boxes, patch), "d");
}

#[test]
fn character_rules() {
    let boxes = row(vec![string("\u{3c0}")]);
    let patch = chars([Rule::new(string("\u{3c0}"), string("pi.alt "))]);
    assert_eq!(render(boxes, patch), "pi.alt");
}

#[test]
fn operator_names() {
    let boxes = row(vec![string("myop")]);
    let patch = operators([Rule::new(string("myop"), string("op(\"myop\")"))]);
    assert_eq!(render(boxes, patch), "op(\"myop\")");
}

#[test]
fn template_rules_arity_key() {
    let boxes = template(&["x"], "Abs");
    let patch = templates([
        Rule::new(string("Abs"), string("generic")),
        Rule::new(string("Abs/1"), string("exact")),
    ]);
    assert_eq!(render(boxes, patch), "exact");
}

#[test]
fn typst_template_fixed_slots() {
    let boxes = template(&["x", "y"], "MyTag");
    let patch = templates([Rule::new(string("MyTag"), typst_template("f({1}, {2})"))]);
    assert_eq!(render(boxes, patch), "f(x, y)");
}

#[test]
fn typst_template_rest_slot() {
    let boxes = template(&["x", "y", "z"], "MyTag");
    let patch = templates([Rule::new(
        string("MyTag"),
        Expr::normal(
            Symbol::new(TypstTemplate::WIRE_HEAD),
            [string("row({1}; {rest})"), string("; ")],
        ),
    )]);
    assert_eq!(render(boxes, patch), "row(x; y; z)");
}
