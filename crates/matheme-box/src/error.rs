//! Structured reasons for handing a box tree back to the WL reference renderer.
//!
//! A fallback must say *why* it happened: the old `Unsupported(String)` only
//! carried prose, so the frontend could neither classify nor diagnose a silent
//! loss of the native fast path. `stage` says which plane rejected the tree,
//! `field` names the config key or box head responsible, and `reason` is the
//! human-readable detail.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// A configuration field that the native renderer does not claim.
    Config,
    /// Box-tree shape that violates the documented box grammar (wrong arity,
    /// payload of the wrong kind). This is mis-shaped input, not a rendering
    /// gap: the tree is still handed to the WL fallback, but diagnostics must
    /// not claim the head is "unsupported".
    Malformed,
    /// Box-tree shape or rendering semantics that cannot be reproduced.
    Render,
    /// The incoming Wolfram expression could not be read as a box tree.
    Ast,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "Config",
            Self::Malformed => "Malformed",
            Self::Render => "Render",
            Self::Ast => "Ast",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub stage: Stage,
    pub field: String,
    pub reason: String,
}

impl Unsupported {
    pub fn config(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            stage: Stage::Config,
            field: field.into(),
            reason: reason.into(),
        }
    }

    /// Generic render-stage fallback. Callers that know the offending head
    /// should prefer [`Unsupported::box_head`] so `Field` stays actionable.
    pub fn render(reason: impl Into<String>) -> Self {
        Self {
            stage: Stage::Render,
            field: "Render".into(),
            reason: reason.into(),
        }
    }

    pub fn box_head(head: &str, reason: impl Into<String>) -> Self {
        Self {
            stage: Stage::Render,
            field: head.into(),
            reason: reason.into(),
        }
    }

    /// A box whose arguments violate the documented grammar (missing or extra
    /// arguments, payload of the wrong kind). Distinct from
    /// [`Unsupported::box_head`] so diagnostics say "malformed" instead of
    /// treating a mis-shaped tree as a missing rendering rule.
    pub fn malformed(head: &str, reason: impl Into<String>) -> Self {
        Self {
            stage: Stage::Malformed,
            field: head.into(),
            reason: reason.into(),
        }
    }

    pub fn ast(reason: impl Into<String>) -> Self {
        Self {
            stage: Stage::Ast,
            field: "Ast".into(),
            reason: reason.into(),
        }
    }
}

impl From<String> for Unsupported {
    fn from(reason: String) -> Self {
        Self::ast(reason)
    }
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} / {}: {}", self.stage, self.field, self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_have_stable_wire_names() {
        assert_eq!(Stage::Config.as_str(), "Config");
        assert_eq!(Stage::Malformed.as_str(), "Malformed");
        assert_eq!(Stage::Render.as_str(), "Render");
        assert_eq!(Stage::Ast.as_str(), "Ast");
    }

    /// A shape violation is malformed input, not a missing rendering rule.
    #[test]
    fn wrong_arity_reports_malformed() {
        let err = Unsupported::malformed("FractionBox", "expected at least 2 argument(s), got 1");
        assert_eq!(err.stage, Stage::Malformed);
        assert_eq!(err.field, "FractionBox");
    }

    #[test]
    fn errors_keep_field_and_reason() {
        let err = Unsupported::config("BoxRules", "rule needs WL evaluation");
        assert_eq!(err.stage, Stage::Config);
        assert_eq!(err.field, "BoxRules");
        assert_eq!(err.reason, "rule needs WL evaluation");
    }
}
