//! Conversion of Wolfram "normal" expressions (`head[args…]`).
//!
//! Three heads are meaningful for `sys.inputs`: `List`, `Association` and
//! `ByteArray`. `System`ByteArray[{…}]` is the WSTP wire form of a WL
//! `ByteArray`; the WXF-only `ExprKind::ByteArray` is handled by the caller.

use typst::foundations::{Bytes, Value};
use wolfram_library_link::expr::{Expr as RawExpr, ExprKind, Normal, Symbol};

use crate::expr::rules_from_normal;
use crate::typst_value::Converter;

/// Convert a normal expression, or return `None` if its head is unsupported.
/// Malformed containers return `Some(Value::None)` after emitting a warning.
pub(crate) fn convert(converter: &mut Converter, normal: &Normal) -> Option<Value> {
    if normal.has_head(&Symbol::new("System`List")) {
        return Some(converter.array(normal.elements()));
    }

    if normal.has_head(&Symbol::new("System`Association")) {
        return rules_from_normal(normal).map(|rules| converter.association(rules));
    }

    if normal.has_head(&Symbol::new("System`ByteArray")) {
        return Some(byte_array(converter, normal.elements()));
    }

    None
}

fn byte_array(converter: &mut Converter, elements: &[RawExpr]) -> Value {
    let [list] = elements else {
        converter.warn("ByteArray expects a single List of byte integers");
        return Value::None;
    };
    let ExprKind::Normal(list) = list.kind() else {
        converter.warn("ByteArray expects a List of byte integers");
        return Value::None;
    };
    if !list.has_head(&Symbol::new("System`List")) {
        converter.warn("ByteArray expects a List of byte integers");
        return Value::None;
    }

    let mut bytes = Vec::with_capacity(list.elements().len());
    for (index, element) in list.elements().iter().enumerate() {
        let ExprKind::Integer(value) = element.kind() else {
            converter.warn(format!("ByteArray element {index} is not an Integer"));
            return Value::None;
        };
        match u8::try_from(*value) {
            Ok(byte) => bytes.push(byte),
            Err(_) => {
                converter.warn(format!(
                    "ByteArray element {index} ({value}) is out of byte range"
                ));
                return Value::None;
            }
        }
    }
    Value::Bytes(Bytes::new(bytes))
}
