use typst::foundations::{Array, Dict, Str, Value};
use wolfram_library_link::expr::{Expr, ExprKind, RuleEntry, Symbol};
use wolfram_library_link::wstp::Link;

/// Read the WSTP argument list, resolving Wolfram 15's context-free symbol
/// names (e.g. `List`, `True`) into their `System\`` forms.
pub fn read_args(link: &mut Link) -> Vec<Expr> {
    let expr = link
        .get_expr_with_resolver(&mut |name: &str| {
            let full = format!("System`{}", name);
            Some(Symbol::new(&full))
        })
        .expect("failed to read WSTP arguments");

    match expr.kind() {
        ExprKind::Normal(n) if n.has_head(&Symbol::new("System`List")) => n.elements().to_vec(),
        _ => panic!("expected an argument list"),
    }
}

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
        ExprKind::ByteArray(_) => {
            warnings.push("ByteArray inputs are not supported; mapped to none".into());
            Value::None
        }
        _ => {
            warnings.push(format!("unsupported input expression: {}", expr));
            Value::None
        }
    }
}
