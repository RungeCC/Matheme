//! Conversion of WL values into Typst `sys.inputs` values.
//!
//! A [`Converter`] borrows one warning sink for the whole (possibly nested)
//! input, so callers get all problems in one pass instead of one error. The
//! container shapes (list, association, byte array) are dispatched through
//! [`Normal`]; no `ExprKind` matching happens here.

use typst::foundations::{Array, Bytes, Dict, Str, Value};
use wolfram_library_link::expr::Expr as RawExpr;

use crate::entry::{self, Entry};
use crate::expr::Expr;
use crate::normal::Normal;
use crate::symbol::Symbol;

/// Convert a WL expression into a Typst value, collecting non-fatal problems.
pub fn to_typst_value(expr: &Expr, warnings: &mut Vec<String>) -> Value {
    Converter::new(warnings).value(expr)
}

pub(crate) struct Converter<'a> {
    warnings: &'a mut Vec<String>,
}

impl<'a> Converter<'a> {
    pub(crate) fn new(warnings: &'a mut Vec<String>) -> Self {
        Self { warnings }
    }

    pub(crate) fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    pub(crate) fn value(&mut self, expr: &Expr) -> Value {
        if let Some(text) = expr.as_str() {
            return Value::Str(Str::from(text));
        }
        if let Some(value) = expr.as_i64() {
            return Value::Int(value);
        }
        if let Some(value) = expr.as_f64() {
            return Value::Float(value);
        }
        if let Some(bytes) = expr.as_byte_array() {
            return Value::Bytes(Bytes::new(bytes.to_vec()));
        }
        if let Some(name) = expr.as_symbol() {
            return self.symbol(name);
        }
        if let Some(normal) = expr.as_normal() {
            return self.normal(normal);
        }
        if let Some(entries) = expr.as_association() {
            return self.association(entries);
        }
        self.warn(format!("unsupported input expression: {}", expr.raw()));
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

    fn normal(&mut self, normal: Normal) -> Value {
        if normal.has_head(&Symbol::LIST) {
            return self.array(normal.elements());
        }
        if normal.has_head(&Symbol::ASSOCIATION) {
            return self.association(entry::parse(normal.elements()));
        }
        if normal.has_head(&Symbol::BYTE_ARRAY) {
            return self.byte_array(normal.elements());
        }
        self.warn(format!(
            "unsupported expression head {} mapped to none",
            normal.head().unwrap_or("<not a symbol>")
        ));
        Value::None
    }

    /// Typst arrays are built element by element; list elements are cloned into
    /// owned wrappers, which keeps the converter free of raw expression types.
    fn array(&mut self, elements: &[RawExpr]) -> Value {
        let values: Vec<Value> = elements
            .iter()
            .map(|element| self.value(&Expr::from_raw(element.clone())))
            .collect();
        Value::Array(values.into_iter().collect::<Array>())
    }

    /// Association keys must be strings: Typst dictionaries are string-keyed.
    fn association(&mut self, entries: Vec<Entry>) -> Value {
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
                    let value = self.value(value);
                    dict.insert(Str::from(name), value);
                }
                None => self.warn(format!(
                    "association key {} is not a String; skipped",
                    key.raw()
                )),
            }
        }
        Value::Dict(dict)
    }

    /// `ByteArray[{…}]` is the WSTP wire form; the WXF form is read directly by
    /// [`Converter::value`] before the normal dispatcher is reached.
    fn byte_array(&mut self, elements: &[RawExpr]) -> Value {
        let [list] = elements else {
            self.warn("ByteArray expects a single List of byte integers");
            return Value::None;
        };
        let list = Expr::from_raw(list.clone());
        let Some(elements) = list.as_normal().filter(|n| n.has_head(&Symbol::LIST)) else {
            self.warn("ByteArray expects a List of byte integers");
            return Value::None;
        };

        let mut bytes = Vec::with_capacity(elements.len());
        for (index, element) in elements.elements().iter().enumerate() {
            let element = Expr::from_raw(element.clone());
            let Some(value) = element.as_i64() else {
                self.warn(format!("ByteArray element {index} is not an Integer"));
                return Value::None;
            };
            match u8::try_from(value) {
                Ok(byte) => bytes.push(byte),
                Err(_) => {
                    self.warn(format!(
                        "ByteArray element {index} ({value}) is out of byte range"
                    ));
                    return Value::None;
                }
            }
        }
        Value::Bytes(Bytes::new(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Rule;
    use wolfram_library_link::expr::{Expr as RawExpr, ExprKind, RuleEntry};

    fn convert(expr: &Expr) -> (Value, Vec<String>) {
        let mut warnings = Vec::new();
        let value = to_typst_value(expr, &mut warnings);
        (value, warnings)
    }

    #[test]
    fn wxf_byte_array_maps_to_typst_bytes() {
        let (value, warnings) = convert(&Expr::from_raw(RawExpr::new(ExprKind::ByteArray(vec![
            0, 1, 2, 255,
        ]))));
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
        // What actually arrives over a WSTP LinkObject.
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
            Rule::new("flag", Expr::symbol("System`Indeterminate")),
        ]);
        let (value, warnings) = convert(&expr);
        let Value::Dict(dict) = value else {
            panic!("expected dict")
        };
        // Non-string keys are dropped; unsupported values are kept as `none`.
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
