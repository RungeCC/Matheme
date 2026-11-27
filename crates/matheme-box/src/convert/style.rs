//! StyleBox / Cell styles: bold, italic, underline — and the warnings for
//! recognized-but-dropped font options.

use super::consts::{
    BOLD_ITALIC_STYLE_NAMES, BOLD_STYLE_NAMES, DROPPED_FONT_OPTIONS, ITALIC_STYLE_NAMES,
};
use super::text::text_of;
use super::{Converter, Ctx, arg, node_list};
use crate::ast::Node;
use crate::error::Unsupported;
use crate::head::Head;

impl Converter<'_> {
    pub(super) fn style(&self, args: &[Node], ctx: Ctx<'_>) -> Result<String, Unsupported> {
        let head = Head::StyleBox;
        let inner = arg(&head, args, 0)?;
        if ctx.config.switch_is("PreserveStyle", "False") {
            return self.convert(inner, ctx);
        }
        let rest = args.get(1..).unwrap_or_default();
        let mut bold = false;
        let mut italic = false;
        let mut underline = false;
        for item in rest {
            match item {
                Node::Str(name) => match name.as_str() {
                    n if BOLD_STYLE_NAMES.contains(&n) => bold = true,
                    n if ITALIC_STYLE_NAMES.contains(&n) => italic = true,
                    n if BOLD_ITALIC_STYLE_NAMES.contains(&n) => {
                        bold = true;
                        italic = true;
                    }
                    // Remaining style names carry the cell's semantic context
                    // ("Text", "Section", ...), not typography.
                    _ => {}
                },
                Node::Sym(name) => match name.as_str() {
                    "Bold" => bold = true,
                    "Italic" => italic = true,
                    "Underlined" => underline = true,
                    "RGBColor" | "GrayLevel" | "Hue" | "CMYKColor" => {
                        return Err(Unsupported::render(
                            "style colors need the WL fallback".to_owned(),
                        ));
                    }
                    _ => {}
                },
                Node::Box { head: color, .. } if color.is_color() => {
                    return Err(Unsupported::render(
                        "style colors need the WL fallback".to_owned(),
                    ));
                }
                Node::Box { head: rule, args } if rule.is_rule() => {
                    let key = args.first().and_then(text_of).unwrap_or_default();
                    let value = args.get(1);
                    match key {
                        "FontWeight" => match value.and_then(text_of) {
                            Some("Bold") => bold = true,
                            other => self.warn(
                                format!("{}/FontWeight", head.name()),
                                format!(
                                    "value {:?} dropped: only Bold changes the layout",
                                    other.unwrap_or("<compound value>")
                                ),
                            ),
                        },
                        "FontSlant" => match value.and_then(text_of) {
                            Some("Italic") => italic = true,
                            other => self.warn(
                                format!("{}/FontSlant", head.name()),
                                format!(
                                    "value {:?} dropped: only Italic changes the layout",
                                    other.unwrap_or("<compound value>")
                                ),
                            ),
                        },
                        "FontColor" => {
                            let inert = match value.and_then(text_of) {
                                None => value.is_none(),
                                Some(name) => name == "None" || name == "Automatic",
                            };
                            if !inert {
                                return Err(Unsupported::render(
                                    "FontColor needs the WL fallback".to_owned(),
                                ));
                            }
                        }
                        "FontVariations" => {
                            let entries = value.and_then(node_list).unwrap_or_default();
                            let has_underline = entries.iter().any(|entry| {
                                (entry.is(&Head::Rule) || entry.is(&Head::RuleDelayed))
                                    && entry.args().first().and_then(text_of) == Some("Underline")
                                    && entry.args().get(1).and_then(text_of) == Some("True")
                            });
                            if has_underline {
                                underline = true;
                            } else if !entries.is_empty() {
                                self.warn(
                                    format!("{}/FontVariations", head.name()),
                                    "variation list dropped: only Underline -> True is rendered"
                                        .to_owned(),
                                );
                            }
                        }
                        other if DROPPED_FONT_OPTIONS.contains(&other) => {
                            self.warn(
                                format!("{}/{}", head.name(), other),
                                format!(
                                    "option dropped: no native rendering for {:?}",
                                    value.and_then(text_of).unwrap_or("<compound value>")
                                ),
                            );
                        }
                        // Anything else is incidental Cell styling; dropping
                        // it is expected and stays silent.
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let mut out = self.convert(inner, ctx)?.trim().to_owned();
        if italic {
            out = format!("italic({out})");
        }
        if underline {
            out = format!("underline({out})");
        }
        if bold {
            out = format!("bold({out})");
        }
        Ok(out)
    }
}
