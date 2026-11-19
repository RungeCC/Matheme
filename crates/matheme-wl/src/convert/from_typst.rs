//! Conversion of Typst evaluation [`Value`]s into Wolfram [`Expr`]s.
//!
//! Native WL primitives (`Null`, `Automatic`, `Bool`, `Integer`, `Real`,
//! `String`, `ByteArray`, `List`, `Association`, `Quantity`, `RGBColor`,
//! `DateObject`, `TimeObject`) are mapped directly. Typst-specific structured
//! values (`Type`, `Label`, `Symbol`, `Version`, `Decimal`, `Fraction`,
//! `Relative`, `Content`, `Module`, `Func`) preserve their internal structure as
//! `"Type"`-tagged associations instead of flattening to plain strings.

use typst::foundations::{
    Content, Datetime, Dict, Label, Module, Repr, Symbol as TypstSymbol, Type, Value, Version,
};
use typst::layout::{Angle, Length, Ratio, Rel};
use typst::visualize::Color;

use crate::assoc;
use crate::entry::Rule;
use crate::expr::Expr;
use crate::symbol::Symbol;

const MAX_DEPTH: usize = 64;

/// Convert a Typst [`Value`] into a structured Wolfram [`Expr`], recording
/// non-fatal notes for truncated or opaque values.
pub fn from_typst(value: &Value, warnings: &mut Vec<String>) -> Expr {
    Converter::new(warnings).value(value, 0)
}

struct Converter<'a> {
    warnings: &'a mut Vec<String>,
}

impl<'a> Converter<'a> {
    fn new(warnings: &'a mut Vec<String>) -> Self {
        Self { warnings }
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    fn value(&mut self, value: &Value, depth: usize) -> Expr {
        if depth >= MAX_DEPTH {
            self.warn("maximum Typst value nesting depth reached; truncated");
            return assoc! {
                "Type" -> value.ty().short_name(),
                "Repr" -> value.repr().as_str(),
            };
        }

        match value {
            Value::None => Expr::null(),
            Value::Auto => Expr::symbol(&Symbol::AUTOMATIC),
            Value::Bool(b) => Expr::boolean(*b),
            Value::Int(i) => Expr::from(*i),
            Value::Float(f) => Expr::from(*f),
            Value::Decimal(d) => {
                let text = d.to_string();
                let approx = text.parse::<f64>().ok().filter(|v| v.is_finite());
                assoc! {
                    "Type" -> "Decimal",
                    "String" -> text,
                    "Value" -> approx,
                }
            }
            Value::Str(s) => Expr::string(s.as_str()),
            Value::Bytes(bytes) => Expr::bytes(bytes.as_slice()),
            Value::Length(len) => length_expr(*len),
            Value::Angle(angle) => angle_expr(*angle),
            Value::Ratio(ratio) => ratio_expr(*ratio),
            Value::Relative(rel) => relative_expr(*rel),
            Value::Fraction(fr) => assoc! { "Type" -> "Fraction", "Value" -> fr.get() },
            Value::Color(color) => color_expr(color),
            Value::Datetime(dt) => datetime_expr(*dt),
            Value::Duration(dur) => quantity(dur.seconds(), "Seconds"),
            Value::Version(ver) => version_expr(ver),
            Value::Symbol(sym) => symbol_expr(sym),
            Value::Label(label) => label_expr(*label),
            Value::Type(ty) => type_expr(*ty),
            Value::Array(arr) => Expr::list(arr.iter().map(|item| self.value(item, depth + 1))),
            Value::Dict(dict) => self.dict(dict, depth + 1),
            Value::Content(content) => self.content(content, depth + 1),
            Value::Module(module) => self.module(module, depth + 1),
            Value::Func(func) => assoc! {
                "Type" -> "Function",
                "Name" -> func.name(),
                "Repr" -> func.repr().as_str(),
            },
            Value::Styles(_)
            | Value::Args(_)
            | Value::Gradient(_)
            | Value::Tiling(_)
            | Value::Dyn(_) => {
                let ty = value.ty().short_name();
                self.warn(format!(
                    "Typst {ty} value has no direct Wolfram representation; returned Repr"
                ));
                assoc! {
                    "Type" -> ty,
                    "Repr" -> value.repr().as_str(),
                }
            }
        }
    }

    fn dict(&mut self, dict: &Dict, depth: usize) -> Expr {
        Expr::association(
            dict.iter()
                .map(|(key, val)| Rule::new(key.as_str(), self.value(val, depth))),
        )
    }

    fn content(&mut self, content: &Content, depth: usize) -> Expr {
        let fields = content.fields();
        let mut rules = vec![
            Rule::new("Type", "Content"),
            Rule::new("Element", content.func().name()),
            Rule::new("Text", content.plain_text().as_str()),
            Rule::new("Fields", self.dict(&fields, depth)),
        ];
        if let Some(label) = content.label() {
            rules.push(Rule::new("Label", label_expr(label)));
        }
        Expr::association(rules)
    }

    fn module(&mut self, module: &Module, depth: usize) -> Expr {
        let bindings = Expr::association(
            module
                .scope()
                .iter()
                .map(|(name, binding)| Rule::new(name.as_str(), self.value(binding.read(), depth))),
        );
        assoc! {
            "Type" -> "Module",
            "Name" -> module.name().map(|n| n.as_str()),
            "Bindings" -> bindings,
        }
    }
}

fn type_expr(ty: Type) -> Expr {
    assoc! {
        "Type" -> "Type",
        "Name" -> ty.short_name(),
        "LongName" -> ty.long_name(),
        "Title" -> ty.title(),
    }
}

fn label_expr(label: Label) -> Expr {
    assoc! {
        "Type" -> "Label",
        "Name" -> label.resolve().as_str(),
    }
}

fn symbol_expr(sym: &TypstSymbol) -> Expr {
    let modifiers = Expr::list(sym.modifiers());
    let variants = Expr::list(sym.variants().map(|(mods, value, _)| {
        let mod_list = Expr::list(mods.into_iter().filter(|m| !m.is_empty()));
        assoc! {
            "Modifiers" -> mod_list,
            "Value" -> value,
        }
    }));
    assoc! {
        "Type" -> "Symbol",
        "Value" -> sym.get(),
        "Modifiers" -> modifiers,
        "Variants" -> variants,
    }
}

fn version_expr(ver: &Version) -> Expr {
    let components = Expr::list(ver.values().iter().copied());
    let formatted = ver
        .values()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(".");
    assoc! {
        "Type" -> "Version",
        "Components" -> components,
        "String" -> formatted,
    }
}

fn quantity(magnitude: f64, unit: &str) -> Expr {
    Expr::normal(&Symbol::QUANTITY, [Expr::from(magnitude), Expr::string(unit)])
}

fn length_expr(len: Length) -> Expr {
    let pt = len.abs.to_pt();
    let em = len.em.get();
    if em == 0.0 {
        quantity(pt, "DesktopPublishingPoints")
    } else if pt == 0.0 {
        assoc! { "Type" -> "Length", "Em" -> em }
    } else {
        assoc! {
            "Type" -> "Length",
            "Points" -> quantity(pt, "DesktopPublishingPoints"),
            "Em" -> em,
        }
    }
}

fn angle_expr(angle: Angle) -> Expr {
    quantity(angle.to_deg(), "AngularDegrees")
}

fn ratio_expr(ratio: Ratio) -> Expr {
    quantity(ratio.get() * 100.0, "Percent")
}

fn relative_expr(rel: Rel<Length>) -> Expr {
    if rel.rel.is_zero() {
        length_expr(rel.abs)
    } else if rel.abs == Length::zero() {
        ratio_expr(rel.rel)
    } else {
        assoc! {
            "Type" -> "Relative",
            "Ratio" -> ratio_expr(rel.rel),
            "Length" -> length_expr(rel.abs),
        }
    }
}

fn color_expr(color: &Color) -> Expr {
    let rgb = color.to_rgb();
    let r = f64::from(rgb.red);
    let g = f64::from(rgb.green);
    let b = f64::from(rgb.blue);
    let a = f64::from(rgb.alpha);
    if (a - 1.0).abs() <= 1e-6 {
        Expr::normal(&Symbol::RGB_COLOR, [r, g, b])
    } else {
        Expr::normal(&Symbol::RGB_COLOR, [r, g, b, a])
    }
}

fn datetime_expr(dt: Datetime) -> Expr {
    match (dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute(), dt.second()) {
        (Some(y), Some(m), Some(d), Some(h), Some(min), Some(s)) => Expr::normal(
            &Symbol::DATE_OBJECT,
            [Expr::list([
                i64::from(y),
                i64::from(m),
                i64::from(d),
                i64::from(h),
                i64::from(min),
                i64::from(s),
            ])],
        ),
        (Some(y), Some(m), Some(d), _, _, _) => Expr::normal(
            &Symbol::DATE_OBJECT,
            [Expr::list([i64::from(y), i64::from(m), i64::from(d)])],
        ),
        (_, _, _, Some(h), Some(min), Some(s)) => Expr::normal(
            &Symbol::TIME_OBJECT,
            [Expr::list([i64::from(h), i64::from(min), i64::from(s)])],
        ),
        _ => assoc! {
            "Type" -> "Datetime",
            "Year" -> dt.year().map(i64::from),
            "Month" -> dt.month().map(i64::from),
            "Day" -> dt.day().map(i64::from),
            "Hour" -> dt.hour().map(i64::from),
            "Minute" -> dt.minute().map(i64::from),
            "Second" -> dt.second().map(i64::from),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Array, Bytes, Decimal, Duration, Str};
    use typst::layout::{Abs, Em, Fr};

    fn convert(value: &Value) -> (Expr, Vec<String>) {
        let mut warnings = Vec::new();
        let expr = from_typst(value, &mut warnings);
        (expr, warnings)
    }

    fn field<'a>(entries: &'a [crate::Entry<'a>], name: &str) -> &'a Expr {
        entries
            .iter()
            .find(|e| e.key_name() == Some(name))
            .expect("missing key")
            .pair()
            .unwrap()
            .1
    }

    #[test]
    fn primitives_map_to_native_wl() {
        assert_eq!(convert(&Value::None).0, Symbol::NULL);
        assert_eq!(convert(&Value::Auto).0, Symbol::AUTOMATIC);
        assert_eq!(convert(&Value::Bool(true)).0.as_bool(), Some(true));
        assert_eq!(convert(&Value::Int(42)).0.as_i64(), Some(42));
        assert_eq!(convert(&Value::Float(2.5)).0.as_f64(), Some(2.5));
        assert_eq!(convert(&Value::Float(f64::NAN)).0, Symbol::INDETERMINATE);
        assert_eq!(
            convert(&Value::Str(Str::from("hi"))).0.as_str(),
            Some("hi")
        );
    }

    #[test]
    fn type_label_symbol_version_and_decimal_preserve_structure() {
        let (ty_expr, w) = convert(&Value::Type(Type::of::<i64>()));
        assert!(w.is_empty());
        let ty = ty_expr.as_association().expect("Type assoc");
        assert_eq!(field(&ty, "Type").as_str(), Some("Type"));
        assert_eq!(field(&ty, "Name").as_str(), Some("int"));
        assert_eq!(field(&ty, "LongName").as_str(), Some("integer"));
        assert_eq!(field(&ty, "Title").as_str(), Some("Integer"));

        let (lbl_expr, _) = convert(&Value::Label(
            Label::new(typst::utils::PicoStr::constant("sec-1")).unwrap(),
        ));
        let lbl = lbl_expr.as_association().expect("Label assoc");
        assert_eq!(field(&lbl, "Type").as_str(), Some("Label"));
        assert_eq!(field(&lbl, "Name").as_str(), Some("sec-1"));

        let sym = TypstSymbol::single("→");
        let (sym_expr, _) = convert(&Value::Symbol(sym));
        let sym_assoc = sym_expr.as_association().expect("Symbol assoc");
        assert_eq!(field(&sym_assoc, "Type").as_str(), Some("Symbol"));
        assert_eq!(field(&sym_assoc, "Value").as_str(), Some("→"));
        assert_eq!(field(&sym_assoc, "Variants").as_list().unwrap().len(), 1);

        let (ver_expr, _) = convert(&Value::Version(Version::from_iter([0, 15, 1])));
        let ver = ver_expr.as_association().expect("Version assoc");
        assert_eq!(field(&ver, "Type").as_str(), Some("Version"));
        assert_eq!(field(&ver, "String").as_str(), Some("0.15.1"));
        assert_eq!(field(&ver, "Components").as_list().unwrap().len(), 3);

        let dec: Decimal = "12.3400".parse().unwrap();
        let (dec_expr, _) = convert(&Value::Decimal(dec));
        let dec_assoc = dec_expr.as_association().expect("Decimal assoc");
        assert_eq!(field(&dec_assoc, "Type").as_str(), Some("Decimal"));
        assert_eq!(field(&dec_assoc, "String").as_str(), Some("12.3400"));
        assert_eq!(field(&dec_assoc, "Value").as_f64(), Some(12.34));
    }

    #[test]
    fn bytes_quantities_colors_and_dates_map_natively() {
        let (bytes_expr, w1) = convert(&Value::Bytes(Bytes::new(vec![1, 2, 3])));
        assert_eq!(bytes_expr.as_byte_array(), Some(&[1, 2, 3][..]));
        assert!(w1.is_empty());

        let (len, _) = convert(&Value::Length(Abs::pt(12.0).into()));
        let normal = len.as_normal().expect("Quantity");
        assert!(normal.has_head(&Symbol::QUANTITY));
        assert_eq!(normal.get(1).and_then(Expr::as_str), Some("DesktopPublishingPoints"));

        let (em_len, _) = convert(&Value::Length(Em::new(1.5).into()));
        let em_entries = em_len.as_association().expect("assoc");
        assert_eq!(field(&em_entries, "Em").as_f64(), Some(1.5));

        let (ang, _) = convert(&Value::Angle(Angle::deg(90.0)));
        assert!(ang.as_normal().unwrap().has_head(&Symbol::QUANTITY));

        let (dur, _) = convert(&Value::Duration(Duration::construct(5, 0, 0, 0, 0)));
        assert!(dur.as_normal().unwrap().has_head(&Symbol::QUANTITY));

        let (col, _) = convert(&Value::Color(Color::from_u8(255, 0, 0, 255)));
        let col_n = col.as_normal().expect("RGBColor");
        assert!(col_n.has_head(&Symbol::RGB_COLOR));
        assert_eq!(col_n.len(), 3);

        let dt = Datetime::from_ymd(2026, 11, 18).unwrap();
        let (dt_expr, _) = convert(&Value::Datetime(dt));
        assert!(dt_expr.as_normal().unwrap().has_head(&Symbol::DATE_OBJECT));
    }

    #[test]
    fn nested_array_dict_and_fraction_convert() {
        let mut dict = Dict::new();
        dict.insert(Str::from("fr"), Value::Fraction(Fr::new(2.0)));
        dict.insert(
            Str::from("items"),
            Value::Array(Array::from_iter([Value::Int(1), Value::Int(2)])),
        );
        let (expr, warnings) = convert(&Value::Dict(dict));
        assert!(warnings.is_empty());
        let entries = expr.as_association().expect("assoc");
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn non_finite_compound_values_do_not_panic() {
        let (fr_inf, _) = convert(&Value::Fraction(Fr::new(f64::INFINITY)));
        let fr_entries = fr_inf.as_association().expect("Fraction assoc");
        assert!(field(&fr_entries, "Value")
            .as_normal()
            .is_some_and(|n| n.has_head(&Symbol::DIRECTED_INFINITY)));

        let (ang_inf, _) = convert(&Value::Angle(Angle::deg(f64::INFINITY)));
        let ang_n = ang_inf.as_normal().expect("Quantity");
        assert!(ang_n.has_head(&Symbol::QUANTITY));
        assert!(ang_n
            .get(0)
            .and_then(Expr::as_normal)
            .is_some_and(|n| n.has_head(&Symbol::DIRECTED_INFINITY)));

        let (len_inf, _) = convert(&Value::Length(Abs::pt(f64::NEG_INFINITY).into()));
        let len_n = len_inf.as_normal().expect("Quantity");
        assert!(len_n.has_head(&Symbol::QUANTITY));
        assert_eq!(
            len_n
                .get(0)
                .and_then(Expr::as_normal)
                .and_then(|n| n.get(0))
                .and_then(Expr::as_i64),
            Some(-1)
        );
    }
}
