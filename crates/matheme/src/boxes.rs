//! Native Box DSL → Typst conversion endpoint.
//!
//! The Wolfram layer sends an optional native config patch. Anything the
//! renderer cannot claim is reported as `"Supported" -> False` with the
//! offending `Stage`/`Field`, so the caller can fall back to the WL reference
//! converter and, when diagnostics are enabled, explain why.
//!
//! An unsupported tree deliberately rides the *success* channel instead of the
//! error pipeline: "this renderer cannot draw this tree" is a domain outcome,
//! not a bridge failure. Raising it would surface as a generic `Failure` in
//! `callFunction` and lose the structured `Stage`/`Field`/`Reason` trio the
//! fallback decision and its diagnostics message are built from.

use matheme_box::{Config, Unsupported};
use matheme_wl::{Expr, Rule, decode};

use crate::call::{Answer, Outcome};
use crate::diagnostics::DiagnosticSource;
use crate::wl_args;

wl_args! {
    #[action = "boxesToTypst"]
    #[derive(Debug)]
    pub struct BoxesArgs {
        "Boxes" => pub boxes: Expr [decode::any()],
        "Config" => pub config: Expr [decode::association()]
            = Expr::association(std::iter::empty::<Rule>()),
    }
}

fn result_entries(result: Result<String, Unsupported>) -> Vec<Rule> {
    match result {
        Ok(result) => vec![Rule::new("Supported", true), Rule::new("Result", result)],
        Err(error) => vec![
            Rule::new("Supported", false),
            Rule::new("Stage", error.stage.as_str()),
            Rule::new("Field", error.field),
            Rule::new("Reason", error.reason),
        ],
    }
}

pub fn run(args: BoxesArgs) -> Answer {
    let outcome = match Config::from_expr(&args.config).and_then(|config| {
        matheme_box::boxes_to_typst_with_config_and_warnings(&args.boxes, &config)
    }) {
        Ok((typst, warnings)) => {
            let mut outcome = Outcome::new(result_entries(Ok(typst)));
            // Recognized-but-dropped box features ride the diagnostics
            // channel; `callUnchecked` issues them as warnings on the WL side.
            for warning in warnings {
                outcome.warn(
                    DiagnosticSource::Conversion,
                    format!("{}: {}", warning.field, warning.message),
                );
            }
            outcome
        }
        Err(error) => Outcome::new(result_entries(Err(error))),
    };
    Answer::Success(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_declares_box_and_optional_config() {
        use crate::args::FromArgs;
        let args = BoxesArgs::wl_arguments();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].0, "Boxes");
        assert_eq!(args[0].1, "_");
        assert_eq!(args[1].0, "Config");
        assert_eq!(args[1].1, "_Association");
    }

    #[test]
    fn failure_response_exposes_stage_field_and_reason() {
        let entries = result_entries(Err(Unsupported::config("BoxRules", "needs WL")));
        let fields: Vec<(&str, &str)> = entries
            .iter()
            .filter_map(|rule| {
                let key = rule.key().as_str()?;
                let value = rule.value().as_str()?;
                Some((key, value))
            })
            .collect();
        assert_eq!(
            fields,
            vec![
                ("Stage", "Config"),
                ("Field", "BoxRules"),
                ("Reason", "needs WL"),
            ]
        );
    }
}
