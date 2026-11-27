//! Owned representation of Wolfram's Box DSL.
//!
//! `wolfram_expr::Expr` is convenient for transport but is not thread-safe and
//! carries fully-qualified contexts. Converting it once into this small owned
//! tree lets the renderer dispatch on [`Head`] and convert independent
//! subtrees in parallel.
//!
//! The WL entry points run `stripIdentityBoxes` before sending a tree, so in
//! production [`from_wolfram`] mostly sees already-normalized input; the
//! identity rewrites below exist for direct callers of this crate (tests,
//! benches) and deliberately mirror `stripIdentityBoxes` so the two passes
//! agree instead of each assuming the other runs.

use matheme_wl::Expr;

use crate::head::Head;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Str(String),
    Sym(String),
    Num(String),
    Box { head: Head, args: Vec<Node> },
}

impl Node {
    pub fn head(&self) -> Option<&Head> {
        match self {
            Self::Box { head, .. } => Some(head),
            _ => None,
        }
    }

    pub fn args(&self) -> &[Node] {
        match self {
            Self::Box { args, .. } => args,
            _ => &[],
        }
    }

    /// Argument `index`, or `None` when this node has fewer arguments.
    pub fn arg(&self, index: usize) -> Option<&Node> {
        self.args().get(index)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn is(&self, head: &Head) -> bool {
        self.head() == Some(head)
    }

    pub fn with_args(head: Head, args: Vec<Node>) -> Self {
        Self::Box { head, args }
    }
}

/// Box heads and the symbols box options carry are `` System` `` names;
/// `MakeBoxes` never emits them from another context. A qualified name from
/// any other context keeps its full spelling, so it misses every `Head` and
/// style-symbol match and lands in the same unsupported fallback the WL
/// reference takes for a foreign symbol — instead of being silently promoted
/// to the `System` box it merely shares a short name with.
const SYSTEM_CONTEXT: &str = "System`";

fn system_name(full: &str) -> &str {
    full.strip_prefix(SYSTEM_CONTEXT).unwrap_or(full)
}

/// Build the owned tree, applying the identity-box rewrites that
/// `stripIdentityBoxes` performs in the WL implementation.
pub fn from_wolfram(expr: &Expr) -> Result<Node, String> {
    let node = raw_from_wolfram(expr)?;
    strip_identity(node)
}

fn raw_from_wolfram(expr: &Expr) -> Result<Node, String> {
    if let Some(value) = expr.as_str() {
        return Ok(Node::Str(value.to_owned()));
    }
    if let Some(value) = expr.as_symbol() {
        return Ok(Node::Sym(system_name(value).to_owned()));
    }
    if let Some(value) = expr.as_i64() {
        return Ok(Node::Num(value.to_string()));
    }
    if let Some(value) = expr.as_f64() {
        if !value.is_finite() {
            return Err(format!("non-finite real in box tree: {value}"));
        }
        return Ok(Node::Num(format!("{}", value)));
    }
    if let Some(normal) = expr.as_normal() {
        let Some(head) = normal.head() else {
            return Err("compound expression with a non-symbol head".into());
        };
        let args = normal
            .elements()
            .iter()
            .map(raw_from_wolfram)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Node::Box {
            head: Head::from_name(system_name(head)),
            args,
        });
    }
    Err(format!("unsupported Wolfram atom: {expr}"))
}

/// Collapse the boxes that only carry presentation or hold one child, mirroring
/// `stripIdentityBoxes`. Applied bottom-up, so a rewrite can expose another.
fn strip_identity(node: Node) -> Result<Node, String> {
    let Node::Box { head, args } = node else {
        return Ok(node);
    };
    let args = args
        .into_iter()
        .map(strip_identity)
        .collect::<Result<Vec<_>, _>>()?;

    match head {
        Head::RowBox if args.len() == 1 => match args.into_iter().next().unwrap() {
            Node::Box {
                head: Head::List,
                args,
            } if args.len() == 1 => strip_identity(args.into_iter().next().unwrap()),
            Node::Box {
                head: Head::List,
                args,
            } => Ok(Node::Box {
                head: Head::List,
                args,
            }),
            other => strip_identity(other),
        },
        Head::InterpretationBox => {
            // The kernel expands these two shapes with state the native path
            // does not have, so leave them to the WL converter.
            if matches!(
                args.first().and_then(Node::head),
                Some(Head::TemplateBox | Head::DynamicModuleBox)
            ) {
                return Err("InterpretationBox with a template needs the WL fallback".into());
            }
            // Otherwise `stripIdentityBoxes` keeps only the display box.
            Ok(args.into_iter().next().unwrap_or(Node::Str(String::new())))
        }
        Head::TagBox => {
            // Preserve `InactiveToken` tags: infix operators and powers read
            // them back to recover the operator.
            let inactive = matches!(args.get(1), Some(Node::Str(tag)) if tag == "InactiveToken");
            if inactive {
                Ok(Node::Box {
                    head: Head::TagBox,
                    args,
                })
            } else {
                Ok(args.into_iter().next().unwrap_or_else(|| Node::Box {
                    head: Head::TagBox,
                    args: Vec::new(),
                }))
            }
        }
        head if head.is_transparent() => {
            Ok(args.into_iter().next().unwrap_or(Node::Str(String::new())))
        }
        head => Ok(Node::Box { head, args }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matheme_wl::Symbol;

    #[test]
    fn row_box_singletons_are_unwrapped() {
        let expr = Expr::normal(
            Symbol::new("System`RowBox"),
            [Expr::normal(Symbol::LIST, [Expr::string("x")])],
        );
        assert_eq!(from_wolfram(&expr).unwrap(), Node::Str("x".into()));
    }

    #[test]
    fn inactive_tags_survive_stripping() {
        let expr = Expr::normal(
            Symbol::new("System`TagBox"),
            [
                Expr::string("+"),
                Expr::string("InactiveToken"),
                Expr::string("SyntaxForm"),
            ],
        );
        let node = from_wolfram(&expr).unwrap();
        assert!(node.is(&Head::TagBox));
    }

    /// A foreign-context name must not be promoted to the `` System` `` box it
    /// merely shares a short name with; the WL reference matches the real
    /// symbol and would take the same unsupported fallback.
    #[test]
    fn foreign_context_names_keep_their_context() {
        let expr = Expr::normal(
            Symbol::new("Global`RowBox"),
            [Expr::normal(Symbol::LIST, [Expr::string("x")])],
        );
        let node = from_wolfram(&expr).unwrap();
        assert_eq!(
            node,
            Node::Box {
                head: Head::Other("Global`RowBox".into()),
                args: vec![Node::Box {
                    head: Head::List,
                    args: vec![Node::Str("x".into())],
                }],
            }
        );
        assert_eq!(
            from_wolfram(&Expr::symbol(Symbol::new("Global`Bold"))).unwrap(),
            Node::Sym("Global`Bold".into())
        );
        assert_eq!(
            from_wolfram(&Expr::symbol(Symbol::new("System`Bold"))).unwrap(),
            Node::Sym("Bold".into())
        );
    }
}
