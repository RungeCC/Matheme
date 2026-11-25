//! Conversion of Wolfram [`Expr`] values into Typst `sys.inputs` [`Value`]s.
//!
//! A [`Converter`] borrows one warning sink for the whole (possibly nested)
//! input, so callers get all problems in one pass instead of failing on the
//! first unsupported element.

use typst::foundations::{Array, Bytes, Dict, Str, Value};
use wolfram_library_link::expr::ExprKind;

use crate::entry::Entry;
use crate::expr::Expr;
use crate::normal::Normal;
use crate::symbol::Symbol;

/// Convert a WL expression into a Typst value, collecting non-fatal problems.
pub fn to_typst(expr: &Expr, warnings: &mut Vec<String>) -> Value {
    Converter::new(warnings).value(expr)
}

struct Converter<'a> {
    warnings: &'a mut Vec<String>,
    path: String,
}

impl<'a> Converter<'a> {
    fn new(warnings: &'a mut Vec<String>) -> Self {
        Self {
            warnings,
            path: String::new(),
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.warnings.push(if self.path.is_empty() {
            message
        } else {
            format!("{}: {message}", self.path)
        });
    }

    fn nested(&mut self, segment: &str, expr: &Expr) -> Value {
        let length = self.path.len();
        self.path.push_str(segment);
        let value = self.value(expr);
        self.path.truncate(length);
        value
    }

    fn approximate(&mut self, number: Option<f64>, kind: &str) -> Value {
        if let Some(value) = number.filter(|n| n.is_finite()) {
            self.warn(format!("{kind} converted to a machine-precision float"));
            Value::Float(value)
        } else {
            self.warn(format!(
                "{kind} is out of range for a Typst float; mapped to none"
            ));
            Value::None
        }
    }

    fn value(&mut self, expr: &Expr) -> Value {
        if let Some(text) = expr.as_str() {
            return Value::Str(Str::from(text));
        }
        if let Some(value) = expr.as_i64() {
            return Value::Int(value);
        }
        if let Some(value) = expr.as_f64() {
            return Value::Float(value);
        }
        match expr.raw().kind() {
            ExprKind::BigInteger(_) => {
                self.warn("integer is outside Typst's signed 64-bit integer range; mapped to none");
                return Value::None;
            }
            ExprKind::BigReal(real) => {
                let text = real.as_str().replace("*^", "e");
                let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((&text, ""));
                let digits = mantissa.split('`').next().unwrap_or(mantissa);
                let number = if exponent.is_empty() {
                    digits.parse().ok()
                } else {
                    format!("{digits}e{exponent}").parse().ok()
                };
                return self.approximate(number, "arbitrary-precision real");
            }
            _ => {}
        }
        match expr.to_bytes() {
            Ok(Some(bytes)) => return Value::Bytes(Bytes::new(bytes.into_owned())),
            Err(reason) => {
                self.warn(reason);
                return Value::None;
            }
            Ok(None) => {}
        }
        if let Some(name) = expr.as_symbol() {
            return self.symbol(name);
        }
        if let Some(entries) = expr.as_association() {
            return self.association(entries);
        }
        if let Some(normal) = expr.as_normal() {
            return self.normal(normal);
        }
        self.warn(format!("unsupported input expression: {expr}"));
        Value::None
    }

    fn symbol(&mut self, name: &str) -> Value {
        if name == Symbol::TRUE.as_str() {
            return Value::Bool(true);
        }
        if name == Symbol::FALSE.as_str() {
            return Value::Bool(false);
        }
        if name == Symbol::NULL.as_str() || name == Symbol::NONE.as_str() {
            return Value::None;
        }
        self.warn(format!("unsupported symbol {name} mapped to none"));
        Value::None
    }

    fn normal(&mut self, normal: Normal<'_>) -> Value {
        if normal.has_head(&Symbol::LIST) {
            return self.array(normal.elements());
        }
        if normal.head() == Some("System`Rational")
            && let [numerator, denominator] = normal.elements()
        {
            let integer = |expr: &Expr| match expr.raw().kind() {
                ExprKind::Integer(value) => Some(*value as f64),
                ExprKind::BigInteger(value) => value.as_str().parse().ok(),
                _ => None,
            };
            let number = integer(numerator)
                .zip(integer(denominator))
                .map(|(n, d)| n / d);
            return self.approximate(number, "exact rational");
        }
        self.warn(format!(
            "unsupported expression head {} mapped to none",
            normal.head().unwrap_or("<not a symbol>")
        ));
        Value::None
    }

    fn array(&mut self, elements: &[Expr]) -> Value {
        let values: Vec<Value> = elements
            .iter()
            .enumerate()
            .map(|(index, elem)| self.nested(&format!("[{}]", index + 1), elem))
            .collect();
        Value::Array(values.into_iter().collect::<Array>())
    }

    /// Association keys must be strings: Typst dictionaries are string-keyed.
    fn association(&mut self, entries: Vec<Entry<'_>>) -> Value {
        let mut dict = Dict::new();
        for entry in entries {
            let (key, value) = match entry.pair() {
                Ok(pair) => pair,
                Err(reason) => {
                    self.warn(reason);
                    continue;
                }
            };
            match key.as_str() {
                Some(name) => {
                    let value = self.nested(&format!("[{name:?}]"), value);
                    dict.insert(Str::from(name), value);
                }
                None => self.warn(format!("association key {key} is not a String; skipped")),
            }
        }
        Value::Dict(dict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Rule;
    use wolfram_library_link::expr::{Expr as RawExpr, ExprKind, RuleEntry};

    fn convert(expr: &Expr) -> (Value, Vec<String>) {
        let mut warnings = Vec::new();
        let value = to_typst(expr, &mut warnings);
        (value, warnings)
    }

    #[test]
    fn wxf_byte_array_maps_to_typst_bytes() {
        let (value, warnings) = convert(&Expr::bytes(vec![0, 1, 2, 255]));
        match value {
            Value::Bytes(bytes) => assert_eq!(bytes.as_slice(), &[0, 1, 2, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
        assert!(warnings.is_empty());
    }

    #[test]
    fn byte_array_nested_in_inputs_association() {
        let bytes = RawExpr::new(ExprKind::ByteArray(b"png\x00data".to_vec()));
        let expr = Expr::from_raw(RawExpr::new(ExprKind::Association(vec![RuleEntry::rule(
            RawExpr::from("blob"),
            bytes,
        )])));
        let (value, warnings) = convert(&expr);
        let Value::Dict(dict) = value else {
            panic!("expected dict")
        };
        let Ok(Value::Bytes(blob)) = dict.get("blob") else {
            panic!("expected nested bytes")
        };
        assert_eq!(blob.as_slice(), b"png\x00data");
        assert!(warnings.is_empty());
    }

    #[test]
    fn wstp_byte_array_expression_maps_to_typst_bytes() {
        let expr = Expr::normal(Symbol::BYTE_ARRAY, [Expr::list([0_i64, 1, 255])]);
        let (value, warnings) = convert(&expr);
        match value {
            Value::Bytes(bytes) => assert_eq!(bytes.as_slice(), &[0, 1, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
        assert!(warnings.is_empty());
    }

    #[test]
    fn malformed_byte_array_warns_and_maps_to_none() {
        let expr = Expr::normal(Symbol::BYTE_ARRAY, [Expr::list([256_i64])]);
        let (value, warnings) = convert(&expr);
        assert!(matches!(value, Value::None));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("out of byte range"));
    }

    #[test]
    fn nested_lists_and_associations_convert() {
        let expr = Expr::association([Rule::new(
            "rows",
            Expr::list([
                Expr::list([1_i64, 2]),
                Expr::association([Rule::new("ok", true)]),
            ]),
        )]);
        let (value, warnings) = convert(&expr);
        let Value::Dict(dict) = value else {
            panic!("expected dict")
        };
        let Ok(Value::Array(rows)) = dict.get("rows") else {
            panic!("expected array")
        };
        assert_eq!(rows.len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn non_string_keys_and_unknown_symbols_warn() {
        let expr = Expr::association([
            Rule::new(1_i64, "value"),
            Rule::new("flag", Expr::symbol(Symbol::system("Indeterminate"))),
        ]);
        let (value, warnings) = convert(&expr);
        let Value::Dict(dict) = value else {
            panic!("expected dict")
        };
        assert_eq!(dict.iter().count(), 1);
        assert!(matches!(dict.get("flag"), Ok(Value::None)));
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("not a String"));
        assert!(warnings[1].contains("unsupported symbol"));
    }

    #[test]
    fn delayed_rules_are_skipped_with_an_explanation() {
        let expr = Expr::normal(
            Symbol::ASSOCIATION,
            [
                Expr::normal(Symbol::RULE, [Expr::from("good"), Expr::from(1_i64)]),
                Expr::normal(
                    Symbol::RULE_DELAYED,
                    [Expr::from("lazy"), Expr::from(2_i64)],
                ),
            ],
        );
        let (value, warnings) = convert(&expr);
        let Value::Dict(dict) = value else {
            panic!("expected dict")
        };
        assert_eq!(dict.iter().count(), 1);
        assert!(dict.get("good").is_ok());
        assert!(dict.get("lazy").is_err());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("RuleDelayed"), "{:?}", warnings);
    }
}
