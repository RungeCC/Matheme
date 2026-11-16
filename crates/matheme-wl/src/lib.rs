//! Wolfram Language expression ↔ Typst value bridge.
//!
//! This crate is deliberately independent of both the Typst *World* and the
//! WSTP transport: it only builds result expressions and maps WL values onto
//! Typst `sys.inputs` values. Both the WXF `ExprKind::ByteArray` and the WSTP
//! wire form `System\`ByteArray[{…}]` map to Typst `bytes`, so binary data can
//! be passed from a notebook without base64.

use typst::foundations::{Array, Bytes, Dict, Str, Value};
use wolfram_library_link::expr::{Expr, ExprKind, RuleEntry, Symbol};

pub fn string(s: impl Into<String>) -> Expr {
    Expr::new(ExprKind::String(s.into()))
}

pub fn symbol(name: &str) -> Expr {
    Expr::new(ExprKind::Symbol(Symbol::new(name)))
}

pub fn integer(i: i64) -> Expr {
    Expr::from(i)
}

pub fn null() -> Expr {
    symbol("System`Null")
}

pub fn rule(key: &str, value: Expr) -> RuleEntry {
    RuleEntry::rule(string(key), value)
}

pub fn assoc(entries: Vec<RuleEntry>) -> Expr {
    Expr::new(ExprKind::Association(entries))
}

pub fn list(items: Vec<Expr>) -> Expr {
    Expr::list(items)
}

pub fn as_string(expr: &Expr) -> Option<&str> {
    match expr.kind() {
        ExprKind::String(s) => Some(s.as_str()),
        _ => None,
    }
}

pub fn as_bool(expr: &Expr) -> Option<bool> {
    match expr.kind() {
        ExprKind::Symbol(s) if s.as_str() == "System`True" => Some(true),
        ExprKind::Symbol(s) if s.as_str() == "System`False" => Some(false),
        _ => None,
    }
}

/// Extract association entries. WSTP 15 sends associations as
/// `System`Association[System`Rule[k, v], ...]`, while `ExprKind::Association`
/// is produced by other transports, so support both.
pub fn as_assoc(expr: &Expr) -> Option<Vec<RuleEntry>> {
    match expr.kind() {
        ExprKind::Association(entries) => Some(entries.clone()),
        ExprKind::Normal(n) if n.has_head(&Symbol::new("System`Association")) => {
            let mut result = Vec::new();
            for rule in n.elements() {
                let ExprKind::Normal(r) = rule.kind() else {
                    return None;
                };
                if !r.has_head(&Symbol::new("System`Rule"))
                    && !r.has_head(&Symbol::new("System`RuleDelayed"))
                {
                    return None;
                }
                let parts = r.elements();
                if parts.len() != 2 {
                    return None;
                }
                result.push(RuleEntry::rule(parts[0].clone(), parts[1].clone()));
            }
            Some(result)
        }
        _ => None,
    }
}

/// Convert a Wolfram Language expression into a Typst `Value`.
///
/// Unsupported values are reported through `warnings` and mapped to `none`
/// rather than aborting the whole compilation.
pub fn to_typst_value(expr: &Expr, warnings: &mut Vec<String>) -> Value {
    if let Some(entries) = as_assoc(expr) {
        let mut dict = Dict::new();
        for entry in entries {
            match as_string(&entry.key) {
                Some(key) => {
                    dict.insert(Str::from(key), to_typst_value(&entry.value, warnings));
                }
                None => warnings.push("association key is not a String; skipped".into()),
            }
        }
        return Value::Dict(dict);
    }

    match expr.kind() {
        ExprKind::String(s) => Value::Str(Str::from(s.as_str())),
        ExprKind::Integer(i) => Value::Int(*i),
        ExprKind::Real(n) => Value::Float(n.into_inner()),
        ExprKind::Symbol(s) => match s.as_str() {
            "System`True" => Value::Bool(true),
            "System`False" => Value::Bool(false),
            "System`Null" | "System`None" => Value::None,
            other => {
                warnings.push(format!("unsupported symbol {} mapped to none", other));
                Value::None
            }
        },
        ExprKind::Normal(n) if n.has_head(&Symbol::new("System`List")) => {
            let values: Vec<Value> = n
                .elements()
                .iter()
                .map(|e| to_typst_value(e, warnings))
                .collect();
            Value::Array(values.into_iter().collect::<Array>())
        }
        ExprKind::Association(entries) => {
            let mut dict = Dict::new();
            for entry in entries {
                match as_string(&entry.key) {
                    Some(key) => {
                        dict.insert(Str::from(key), to_typst_value(&entry.value, warnings));
                    }
                    None => warnings.push("association key is not a String; skipped".into()),
                }
            }
            Value::Dict(dict)
        }
        // WSTP does not have a ByteArray wire type: the kernel sends
        // `System`ByteArray[{b1, b2, …}]` as a normal expression (also for a
        // ByteArray built from a base64 string). `ExprKind::ByteArray` is the
        // WXF-only form, so accept both. Bytes requires owned 'static data,
        // hence the copy in both arms.
        ExprKind::Normal(n) if n.has_head(&Symbol::new("System`ByteArray")) => {
            let parts = n.elements();
            let mut invalid = || {
                warnings.push("ByteArray must contain a list of byte integers".into());
                Value::None
            };
            if parts.len() != 1 {
                return invalid();
            }
            let ExprKind::Normal(list) = parts[0].kind() else {
                return invalid();
            };
            if !list.has_head(&Symbol::new("System`List")) {
                return invalid();
            }
            let mut bytes = Vec::with_capacity(list.elements().len());
            for element in list.elements() {
                let ExprKind::Integer(value) = element.kind() else {
                    return invalid();
                };
                match u8::try_from(*value) {
                    Ok(byte) => bytes.push(byte),
                    Err(_) => {
                        warnings.push(format!("ByteArray element {value} is out of byte range"));
                        return Value::None;
                    }
                }
            }
            Value::Bytes(Bytes::new(bytes))
        }
        ExprKind::ByteArray(bytes) => Value::Bytes(Bytes::new(bytes.clone())),
        _ => {
            warnings.push(format!("unsupported input expression: {}", expr));
            Value::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_array_maps_to_typst_bytes() {
        let expr = Expr::new(ExprKind::ByteArray(vec![0, 1, 2, 255]));
        let mut warnings = Vec::new();
        match to_typst_value(&expr, &mut warnings) {
            Value::Bytes(bytes) => assert_eq!(bytes.as_slice(), &[0, 1, 2, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
        assert!(warnings.is_empty(), "ByteArray must not emit a warning");
    }

    #[test]
    fn byte_array_nested_in_inputs_association() {
        let bytes = Expr::new(ExprKind::ByteArray(b"png\x00data".to_vec()));
        let expr = Expr::new(ExprKind::Association(vec![RuleEntry::rule(
            string("blob"),
            bytes,
        )]));
        let mut warnings = Vec::new();
        let Value::Dict(dict) = to_typst_value(&expr, &mut warnings) else {
            panic!("expected dict")
        };
        let Value::Bytes(blob) = dict.get("blob").expect("blob key present") else {
            panic!("expected nested bytes")
        };
        assert_eq!(blob.as_slice(), b"png\x00data");
        assert!(warnings.is_empty());
    }

    #[test]
    fn wstp_byte_array_expression_maps_to_typst_bytes() {
        // What actually arrives over a WSTP LinkObject.
        let expr = Expr::normal(
            Symbol::new("System`ByteArray"),
            vec![list(vec![integer(0), integer(1), integer(255)])],
        );
        let mut warnings = Vec::new();
        match to_typst_value(&expr, &mut warnings) {
            Value::Bytes(bytes) => assert_eq!(bytes.as_slice(), &[0, 1, 255]),
            other => panic!("expected bytes, got {other:?}"),
        }
        assert!(warnings.is_empty());
    }

    #[test]
    fn malformed_byte_array_warns_and_maps_to_none() {
        let expr = Expr::normal(
            Symbol::new("System`ByteArray"),
            vec![list(vec![integer(256)])],
        );
        let mut warnings = Vec::new();
        assert!(matches!(to_typst_value(&expr, &mut warnings), Value::None));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn byte_array_output_expression_round_trips() {
        let expr = Expr::new(ExprKind::ByteArray(vec![7, 8]));
        let ExprKind::ByteArray(bytes) = expr.kind() else {
            panic!("expected ByteArray expression")
        };
        assert_eq!(bytes, &vec![7, 8]);
    }
}
