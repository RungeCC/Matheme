//! Conversion of WL values into Typst `sys.inputs` values.
//!
//! A [`Converter`] borrows one warning sink for the whole (possibly nested)
//! input, so callers get all problems in one pass instead of one error. Normal
//! expressions have their own module because they carry the container shapes
//! (list, association, byte array).

use typst::foundations::{Array, Bytes, Dict, Str, Value};
use wolfram_library_link::expr::{Expr as RawExpr, ExprKind};

use crate::expr::{Expr, Rule};
use crate::normal;

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
        self.value_raw(expr.raw())
    }

    /// Convert a borrowed raw expression. Used for the elements of WL lists,
    /// which would otherwise have to be cloned into wrappers first.
    pub(crate) fn value_raw(&mut self, raw: &RawExpr) -> Value {
        match raw.kind() {
            ExprKind::String(value) => Value::Str(Str::from(value.as_str())),
            ExprKind::Integer(value) => Value::Int(*value),
            ExprKind::Real(value) => Value::Float(value.into_inner()),
            ExprKind::Symbol(symbol) => match symbol.as_str() {
                "System`True" => Value::Bool(true),
                "System`False" => Value::Bool(false),
                "System`Null" | "System`None" => Value::None,
                other => {
                    self.warn(format!("unsupported symbol {other} mapped to none"));
                    Value::None
                }
            },
            // WXF carries bytes natively; `normal` handles the WSTP form
            // `System`ByteArray[{…}]`. `Bytes` needs owned data.
            ExprKind::ByteArray(bytes) => Value::Bytes(Bytes::new(bytes.clone())),
            ExprKind::Normal(normal) => normal::convert(self, normal).unwrap_or_else(|| {
                let head = match normal.head().kind() {
                    ExprKind::Symbol(symbol) => symbol.as_str().to_owned(),
                    other => other.to_string(),
                };
                self.warn(format!("unsupported expression head {head} mapped to none"));
                Value::None
            }),
            ExprKind::Association(_) => self.association(
                Expr::from_raw(raw.clone())
                    .as_association()
                    .expect("ExprKind::Association"),
            ),
            _ => {
                self.warn(format!("unsupported input expression: {raw}"));
                Value::None
            }
        }
    }

    /// Association keys must be strings: Typst dictionaries are string-keyed.
    pub(crate) fn association(&mut self, entries: Vec<Rule>) -> Value {
        let mut dict = Dict::new();
        for rule in entries {
            match rule.key().as_str() {
                Some(key) => {
                    let value = self.value(rule.value());
                    dict.insert(Str::from(key), value);
                }
                None => self.warn(format!(
                    "association key {} is not a String; skipped",
                    rule.key().raw()
                )),
            }
        }
        Value::Dict(dict)
    }

    /// Typst arrays are built from already-converted element values.
    pub(crate) fn array(&mut self, elements: &[RawExpr]) -> Value {
        let values: Array = elements
            .iter()
            .map(|element| self.value_raw(element))
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        Value::Array(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wolfram_library_link::expr::RuleEntry;

    fn convert(expr: &Expr) -> (Value, Vec<String>) {
        let mut warnings = Vec::new();
        let value = to_typst_value(expr, &mut warnings);
        (value, warnings)
    }

    #[test]
    fn wxf_byte_array_maps_to_typst_bytes() {
        let (value, warnings) = convert(&Expr::from_raw(RawExpr::new(ExprKind::ByteArray(
            vec![0, 1, 2, 255],
        ))));
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
        let Value::Dict(dict) = value else { panic!("expected dict") };
        let Value::Bytes(blob) = dict.get("blob").expect("blob key present") else {
            panic!("expected nested bytes")
        };
        assert_eq!(blob.as_slice(), b"png\x00data");
        assert!(warnings.is_empty());
    }

    #[test]
    fn wstp_byte_array_expression_maps_to_typst_bytes() {
        // What actually arrives over a WSTP LinkObject.
        let expr = Expr::normal("System`ByteArray", [Expr::list([0_i64, 1, 255])]);
        let (value, warnings) = convert(&expr);
        match value {
            Value::Bytes(bytes) => assert_eq!(bytes.as_slice(), &[0, 1, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
        assert!(warnings.is_empty());
    }

    #[test]
    fn malformed_byte_array_warns_and_maps_to_none() {
        let expr = Expr::normal("System`ByteArray", [Expr::list([256_i64])]);
        let (value, warnings) = convert(&expr);
        assert!(matches!(value, Value::None));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("out of byte range"));
    }

    #[test]
    fn nested_lists_and_associations_convert() {
        let expr = Expr::association([Rule::new(
            "rows",
            Expr::list([Expr::list([1_i64, 2]), Expr::association([Rule::new("ok", true)])]),
        )]);
        let (value, warnings) = convert(&expr);
        let Value::Dict(dict) = value else { panic!("expected dict") };
        let Value::Array(rows) = dict.get("rows").expect("rows key present") else {
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
        let Value::Dict(dict) = value else { panic!("expected dict") };
        // Non-string keys are dropped; unsupported values are kept as `none`.
        assert_eq!(dict.iter().count(), 1);
        assert!(matches!(dict.get("flag"), Ok(Value::None)));
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("not a String"));
        assert!(warnings[1].contains("unsupported symbol"));
    }
}
