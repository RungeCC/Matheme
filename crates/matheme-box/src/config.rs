//! Native conversion configuration (R1 data plane).
//!
//! The WL side computes a *delta* against the effective configuration and
//! sends it as the `"Config"` association. This module decodes that delta and
//! exposes it to the renderer. Anything that cannot be decoded as plain data
//! is rejected with `Stage::Config` / `Field = <key>` so the caller falls back
//! to the WL reference converter instead of silently rendering the default.

use std::collections::HashMap;

use matheme_wl::{Expr, TypstTemplate, short_name};

use crate::error::Unsupported;
use crate::schema;

/// Wire version of the `"Config"` association. Bumped when the set of wire
/// value types changes (v2 added `TypstTemplate` values).
pub const NATIVE_CONFIG_VERSION: i64 = 2;

/// A user `TemplateRules` entry that the native renderer can execute.
///
/// A `String` is returned verbatim, matching the WL string handler. A
/// `TypstTemplate` substitutes rendered string arguments into its own slots;
/// it is a distinct public value, never Wolfram's `StringTemplate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTemplate {
    Text(String),
    Typst(TypstTemplate),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    char_overrides: HashMap<String, String>,
    operator_overrides: HashMap<String, String>,
    templates: HashMap<String, RuntimeTemplate>,
    switches: HashMap<String, String>,
}

impl Config {
    /// Decode a native config patch. Missing `"Version"` is accepted as a
    /// default-config request; an explicit version must match this build.
    pub fn from_expr(expr: &Expr) -> Result<Self, Unsupported> {
        let entries = expr
            .as_association()
            .ok_or_else(|| Unsupported::config("Config", "native config must be an Association"))?;

        let mut config = Self::default();
        for entry in entries {
            let (key_expr, value) = entry
                .pair()
                .map_err(|reason| Unsupported::config("Config", reason))?;
            let key = key_expr.as_str().ok_or_else(|| {
                Unsupported::config("Config", "native config keys must be Strings")
            })?;

            match key {
                "Version" => {
                    let version = value.as_i64().ok_or_else(|| {
                        Unsupported::config("Version", "version must be an integer")
                    })?;
                    if version != NATIVE_CONFIG_VERSION {
                        return Err(Unsupported::config(
                            "Version",
                            format!(
                                "native config version {version} is not supported (expected {NATIVE_CONFIG_VERSION})"
                            ),
                        ));
                    }
                }
                "CharOverrides" => {
                    config.char_overrides = decode_string_map("CharOverrides", value)?;
                }
                "OperatorOverrides" => {
                    config.operator_overrides = decode_string_map("OperatorOverrides", value)?;
                }
                "TemplateOverrides" => {
                    config.templates = decode_templates(value)?;
                }
                "Switches" => {
                    config.switches = decode_switches(value)?;
                }
                other => {
                    return Err(Unsupported::config(other, "unclaimed native config field"));
                }
            }
        }

        Ok(config)
    }

    pub fn char(&self, text: &str) -> Option<&str> {
        self.char_overrides.get(text).map(String::as_str)
    }

    pub fn operator(&self, name: &str) -> Option<&str> {
        self.operator_overrides.get(name).map(String::as_str)
    }

    /// User template lookup: exact `tag/arity` first, then the tag-wide key.
    pub fn template(&self, tag: &str, arity: usize) -> Option<&RuntimeTemplate> {
        self.templates
            .get(&format!("{tag}/{arity}"))
            .or_else(|| self.templates.get(tag))
    }

    /// A switch value, as the WL config spells it (e.g. `"Overline"`).
    pub fn switch(&self, key: &str) -> Option<&str> {
        self.switches.get(key).map(String::as_str)
    }

    /// True when a switch is set to a non-default value.
    pub fn switch_is(&self, key: &str, value: &str) -> bool {
        self.switch(key) == Some(value)
    }
}

fn decode_string_map(field: &str, expr: &Expr) -> Result<HashMap<String, String>, Unsupported> {
    let entries = expr
        .as_association()
        .ok_or_else(|| Unsupported::config(field, "expected an Association of Strings"))?;
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        let (key, value) = entry
            .pair()
            .map_err(|reason| Unsupported::config(field, reason))?;
        let key = key
            .as_str()
            .ok_or_else(|| Unsupported::config(field, "override keys must be Strings"))?;
        let value = value
            .as_str()
            .ok_or_else(|| Unsupported::config(field, "override values must be Strings"))?;
        out.insert(key.to_owned(), value.to_owned());
    }
    Ok(out)
}

fn decode_templates(expr: &Expr) -> Result<HashMap<String, RuntimeTemplate>, Unsupported> {
    // Errors name the public option ("TemplateRules"), matching what the WL
    // side's `nativeConfigPatch` reports for the same field — never the wire
    // field name, which is not a user-visible concept.
    let entries = expr
        .as_association()
        .ok_or_else(|| Unsupported::config("TemplateRules", "expected an Association"))?;
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        let (key, value) = entry
            .pair()
            .map_err(|reason| Unsupported::config("TemplateRules", reason))?;
        let key = key
            .as_str()
            .ok_or_else(|| Unsupported::config("TemplateRules", "template keys must be Strings"))?;
        let template = if let Some(text) = value.as_str() {
            RuntimeTemplate::Text(text.to_owned())
        } else {
            TypstTemplate::try_from(value)
                .map(RuntimeTemplate::Typst)
                .map_err(|err| Unsupported::config("TemplateRules", err.to_string()))?
        };
        out.insert(key.to_owned(), template);
    }
    Ok(out)
}

fn decode_switches(expr: &Expr) -> Result<HashMap<String, String>, Unsupported> {
    let entries = expr
        .as_association()
        .ok_or_else(|| Unsupported::config("Switches", "expected an Association"))?;
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        let (key, value) = entry
            .pair()
            .map_err(|reason| Unsupported::config("Switches", reason))?;
        let key = key
            .as_str()
            .ok_or_else(|| Unsupported::config("Switches", "switch keys must be Strings"))?;
        if !schema::claimed_switches().contains(&key) {
            return Err(Unsupported::config(
                key,
                "switch is not claimed by the native renderer",
            ));
        }
        let text = match value.as_str() {
            Some(text) => text.to_owned(),
            None if value.as_symbol().is_some() => {
                // `Automatic` and `True`/`False` arrive as symbols; keep the
                // unqualified name so callers can compare it literally.
                short_name(value.as_symbol().unwrap()).to_owned()
            }
            None => {
                return Err(Unsupported::config(
                    key,
                    "switch values must be Strings or symbolic constants",
                ));
            }
        };
        out.insert(key.to_owned(), text);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use matheme_wl::{Rule, Symbol};

    fn config_expr(entries: impl IntoIterator<Item = Rule>) -> Expr {
        Expr::association(entries)
    }

    fn string(value: &str) -> Expr {
        Expr::string(value)
    }

    #[test]
    fn empty_patch_is_the_default_configuration() {
        assert_eq!(
            Config::from_expr(&config_expr([])).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn current_version_is_accepted() {
        let expr = config_expr([Rule::new("Version", NATIVE_CONFIG_VERSION)]);
        assert!(Config::from_expr(&expr).is_ok());
    }

    #[test]
    fn wrong_version_reports_its_field() {
        let expr = config_expr([Rule::new("Version", NATIVE_CONFIG_VERSION + 1)]);
        let err = Config::from_expr(&expr).unwrap_err();
        assert_eq!(err.field, "Version");
        assert_eq!(err.stage, crate::error::Stage::Config);
    }

    #[test]
    fn unclaimed_field_is_rejected() {
        let expr = config_expr([Rule::new("BoxRules", Expr::list(Vec::<Expr>::new()))]);
        let err = Config::from_expr(&expr).unwrap_err();
        assert_eq!(err.field, "BoxRules");
    }

    #[test]
    fn char_overrides_are_decoded() {
        let expr = config_expr([Rule::new(
            "CharOverrides",
            config_expr([Rule::new(string("\u{f74c}"), string("d alt"))]),
        )]);
        let config = Config::from_expr(&expr).unwrap();
        assert_eq!(config.char("\u{f74c}"), Some("d alt"));
        assert_eq!(config.char("\u{3b1}"), None);
    }

    #[test]
    fn operator_overrides_are_decoded() {
        let expr = config_expr([Rule::new(
            "OperatorOverrides",
            config_expr([Rule::new(string("myop"), string("op(\"custom\")"))]),
        )]);
        let config = Config::from_expr(&expr).unwrap();
        assert_eq!(config.operator("myop"), Some("op(\"custom\")"));
    }

    #[test]
    fn a_template_key_is_exact_before_it_is_generic() {
        let expr = config_expr([Rule::new(
            "TemplateOverrides",
            config_expr([
                Rule::new(string("Abs"), string("generic")),
                Rule::new(string("Abs/1"), string("exact")),
            ]),
        )]);
        let config = Config::from_expr(&expr).unwrap();
        assert_eq!(
            config.template("Abs", 1),
            Some(&RuntimeTemplate::Text("exact".into()))
        );
        assert_eq!(
            config.template("Abs", 2),
            Some(&RuntimeTemplate::Text("generic".into()))
        );
    }

    #[test]
    fn function_templates_fail_closed() {
        let expr = config_expr([Rule::new(
            "TemplateOverrides",
            config_expr([Rule::new(
                string("Abs"),
                Expr::normal(Symbol::new("System`Function"), Vec::<Expr>::new()),
            )]),
        )]);
        let err = Config::from_expr(&expr).unwrap_err();
        assert_eq!(err.field, "TemplateRules");
    }

    #[test]
    fn switches_are_decoded_with_symbolic_values() {
        let expr = config_expr([Rule::new(
            "Switches",
            config_expr([
                Rule::new(string("ConjugateStyle"), string("Overline")),
                Rule::new(string("PreserveStyle"), Expr::symbol(Symbol::FALSE)),
            ]),
        )]);
        let config = Config::from_expr(&expr).unwrap();
        assert!(config.switch_is("ConjugateStyle", "Overline"));
        assert_eq!(config.switch("PreserveStyle"), Some("False"));
    }

    #[test]
    fn unclaimed_switch_fails_closed() {
        let expr = config_expr([Rule::new(
            "Switches",
            config_expr([Rule::new(string("Mystery"), string("x"))]),
        )]);
        let err = Config::from_expr(&expr).unwrap_err();
        assert_eq!(err.field, "Mystery");
    }
}
