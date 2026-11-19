//! Shared WSTP link helpers used by both the LibraryLink bridge (`matheme`)
//! and the standalone CLI (`matheme-cli`).

use wolfram_library_link::expr::{Expr as RawExpr, ExprKind, Symbol as RawSymbol};
use wstp::Link;

use crate::{Expr, Symbol};

/// Result of draining packets until a `ReturnPacket[expr]` arrives.
#[derive(Debug, Clone)]
pub struct PacketOutput {
    pub expr: Expr,
    pub messages: Vec<String>,
    pub prints: Vec<String>,
}

/// Read one expression from a WSTP link, resolving unqualified symbols into
/// `System\`` (Wolfram 15+ omits contexts on packet heads such as `ReturnPacket`).
pub fn read_expr_resolved(link: &mut Link) -> Result<RawExpr, wstp::Error> {
    link.get_expr_with_resolver(&mut |name: &str| {
        Some(RawSymbol::new(&format!("System`{name}")))
    })
}

/// Drain WSTP packets until `ReturnPacket[expr]` arrives, collecting any
/// `MessagePacket`/`TextPacket` sideband output along the way.
///
/// When `ignore_prompt_packets` is `true`, `InputNamePacket` and
/// `OutputNamePacket` are skipped (needed by interactive client sessions).
pub fn read_return_packet(
    link: &mut Link,
    max_packets: usize,
    ignore_prompt_packets: bool,
) -> Result<PacketOutput, String> {
    let return_head = RawSymbol::new(Symbol::RETURN_PACKET.as_str());
    let text_head = RawSymbol::new("System`TextPacket");
    let message_head = RawSymbol::new("System`MessagePacket");
    let input_name_head = RawSymbol::new("System`InputNamePacket");
    let output_name_head = RawSymbol::new("System`OutputNamePacket");

    let mut messages = Vec::new();
    let mut prints = Vec::new();
    let mut pending_message = false;

    for _ in 0..max_packets {
        let packet = read_expr_resolved(link).map_err(|err| format!("WSTP read error: {err}"))?;
        match packet.kind() {
            ExprKind::Normal(normal)
                if normal.has_head(&return_head) && normal.elements().len() == 1 =>
            {
                return Ok(PacketOutput {
                    expr: Expr::from_raw(normal.elements()[0].clone()),
                    messages,
                    prints,
                });
            }
            ExprKind::Normal(normal) if normal.has_head(&message_head) => {
                pending_message = true;
            }
            ExprKind::Normal(normal)
                if normal.has_head(&text_head) && normal.elements().len() == 1 =>
            {
                let text = Expr::from_raw_ref(&normal.elements()[0])
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| normal.elements()[0].to_string());
                if pending_message {
                    messages.push(text);
                    pending_message = false;
                } else {
                    prints.push(text);
                }
            }
            ExprKind::Normal(normal)
                if ignore_prompt_packets
                    && (normal.has_head(&input_name_head)
                        || normal.has_head(&output_name_head)) =>
            {
                continue;
            }
            other => {
                return Err(format!(
                    "unexpected WSTP packet while waiting for ReturnPacket: {other}"
                ));
            }
        }
    }

    Err("exceeded WSTP packet limit before ReturnPacket".into())
}
