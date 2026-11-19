//! Shared WSTP link wrapper and evaluation helpers for both the LibraryLink
//! bridge (`matheme`) and the standalone CLI (`matheme-cli`).
//!
//! Keeping all WSTP transport code in this crate preserves `matheme-wl`'s
//! guarantee of having zero WSTP / native-library dependency.

use std::fmt;
use std::path::{Path, PathBuf};

use matheme_wl::{Expr, Symbol};
use wolfram_expr::Symbol as RawSymbol;
pub use wstp::Link;

/// Error returned by [`WstpLink`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WstpError(pub String);

impl WstpError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for WstpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for WstpError {}

/// Result of evaluating a Wolfram Language expression over WSTP, including any
/// sideband `MessagePacket` / `TextPacket` output received before `ReturnPacket`.
#[derive(Debug, Clone)]
pub struct EvalOutput {
    pub expr: Expr,
    pub messages: Vec<String>,
    pub prints: Vec<String>,
}

/// Borrowed wrapper around a [`wstp::Link`] providing context-resolving reads,
/// `Expr`-based I/O, and a unified `EvaluatePacket` → `ReturnPacket` flow.
pub struct WstpLink<'a> {
    link: &'a mut Link,
}

impl<'a> WstpLink<'a> {
    pub fn new(link: &'a mut Link) -> Self {
        Self { link }
    }

    /// Borrow the underlying raw [`wstp::Link`] for transport-specific calls
    /// (such as LibraryLink's `processWSLINK` callback).
    pub fn raw(&mut self) -> &mut Link {
        self.link
    }

    /// Read one expression from the link, resolving unqualified symbols into
    /// `System\`` (Wolfram 15+ omits contexts on packet heads like `ReturnPacket`).
    pub fn read_expr(&mut self) -> Result<Expr, WstpError> {
        self.link
            .get_expr_with_resolver(&mut |name: &str| {
                Some(RawSymbol::new(&format!("System`{name}")))
            })
            .map(Expr::from_raw)
            .map_err(|err| WstpError(format!("WSTP read error: {err}")))
    }

    /// Write an [`Expr`] to the link.
    pub fn put_expr(&mut self, expr: &Expr) -> Result<(), WstpError> {
        self.link
            .put_expr(expr.raw())
            .map_err(|err| WstpError(format!("WSTP write error: {err}")))
    }

    /// Flush pending outgoing packets on the link.
    pub fn flush(&mut self) -> Result<(), WstpError> {
        self.link
            .flush()
            .map_err(|err| WstpError(format!("WSTP flush error: {err}")))
    }

    /// Write `System\`EvaluatePacket[expr]` to the link without flushing or
    /// dispatching.
    pub fn put_eval_packet(&mut self, expr: &Expr) -> Result<(), WstpError> {
        let packet = eval_packet_expr(expr);
        self.put_expr(&packet)
    }

    /// Drain startup packets until the kernel's initial `InputNamePacket`.
    pub fn drain_until_input_name(&mut self, max_packets: usize) -> Result<(), WstpError> {
        let input_name = Symbol::system("InputNamePacket");
        for _ in 0..max_packets {
            let packet = self.read_expr()?;
            if packet
                .as_normal()
                .is_some_and(|n| n.has_head(&input_name))
            {
                return Ok(());
            }
        }
        Err(WstpError(
            "did not receive initial InputNamePacket from Wolfram kernel".into(),
        ))
    }

    /// Read packets until `ReturnPacket[expr]` arrives, collecting sideband
    /// `MessagePacket` / `TextPacket` output along the way.
    pub fn read_return_packet(
        &mut self,
        max_packets: usize,
        ignore_prompt_packets: bool,
    ) -> Result<EvalOutput, WstpError> {
        let mut messages = Vec::new();
        let mut prints = Vec::new();
        let mut pending_message = false;

        for _ in 0..max_packets {
            let packet = self.read_expr()?;
            match classify_packet(&packet) {
                PacketAction::Return(expr) => {
                    return Ok(EvalOutput {
                        expr,
                        messages,
                        prints,
                    });
                }
                PacketAction::MessageHeader => {
                    pending_message = true;
                }
                PacketAction::Text(text) => {
                    if pending_message {
                        messages.push(text);
                        pending_message = false;
                    } else {
                        prints.push(text);
                    }
                }
                PacketAction::Prompt if ignore_prompt_packets => continue,
                PacketAction::Prompt | PacketAction::Unexpected => {
                    return Err(WstpError(format!(
                        "unexpected WSTP packet while waiting for ReturnPacket: {packet}"
                    )));
                }
            }
        }

        Err(WstpError(
            "exceeded WSTP packet limit before ReturnPacket".into(),
        ))
    }

    /// Unified evaluation pipeline:
    /// 1. Send `EvaluatePacket[expr]`;
    /// 2. Run `dispatch` (e.g. `flush()` for client links or `processWSLINK` for LibraryLink);
    /// 3. Drain packets until `ReturnPacket[result]`.
    pub fn eval_with<F>(
        &mut self,
        expr: &Expr,
        max_packets: usize,
        ignore_prompt_packets: bool,
        dispatch: F,
    ) -> Result<EvalOutput, WstpError>
    where
        F: FnOnce(&mut Link) -> Result<(), WstpError>,
    {
        self.put_eval_packet(expr)?;
        dispatch(self.link)?;
        self.read_return_packet(max_packets, ignore_prompt_packets)
    }

    /// Evaluate a Wolfram Language expression on a client WSTP link (flush + read
    /// up to 256 packets, ignoring `InputNamePacket` / `OutputNamePacket`).
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<EvalOutput, WstpError> {
        self.eval_with(expr, 256, true, |link| {
            link.flush()
                .map_err(|err| WstpError(format!("WSTP flush error: {err}")))
        })
    }

    /// Evaluate a Wolfram Language code string via `System\`ToExpression[code]`.
    pub fn eval_code(&mut self, wl_code: &str) -> Result<EvalOutput, WstpError> {
        self.eval_expr(&to_expression_expr(wl_code))
    }

    /// Send `EvaluatePacket[Exit[]]` and flush, ignoring transport errors.
    pub fn send_exit(&mut self) {
        let exit = eval_packet_expr(&Expr::normal(Symbol::system("Exit"), Vec::<Expr>::new()));
        let _ = self.put_expr(&exit);
        let _ = self.flush();
    }
}

/// Standard subdirectory under a Wolfram installation containing bundled TrueType fonts.
pub fn wolfram_truetype_font_dir(install_dir: &Path) -> PathBuf {
    install_dir
        .join("SystemFiles")
        .join("Fonts")
        .join("TrueType")
}

fn eval_packet_expr(expr: &Expr) -> Expr {
    Expr::normal(Symbol::system("EvaluatePacket"), [expr.clone()])
}

fn to_expression_expr(wl_code: &str) -> Expr {
    Expr::normal(Symbol::system("ToExpression"), [Expr::string(wl_code)])
}

#[derive(Debug, PartialEq, Eq)]
enum PacketAction {
    Return(Expr),
    MessageHeader,
    Text(String),
    Prompt,
    Unexpected,
}

fn classify_packet(packet: &Expr) -> PacketAction {
    let Some(normal) = packet.as_normal() else {
        return PacketAction::Unexpected;
    };
    if normal.has_head(&Symbol::RETURN_PACKET) && normal.len() == 1 {
        return PacketAction::Return(normal.elements()[0].clone());
    }
    if normal.has_head(&Symbol::system("MessagePacket")) {
        return PacketAction::MessageHeader;
    }
    if normal.has_head(&Symbol::system("TextPacket")) && normal.len() == 1 {
        let elem = &normal.elements()[0];
        let text = elem
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| elem.to_string());
        return PacketAction::Text(text);
    }
    if normal.has_head(&Symbol::system("InputNamePacket"))
        || normal.has_head(&Symbol::system("OutputNamePacket"))
    {
        return PacketAction::Prompt;
    }
    PacketAction::Unexpected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_standard_wstp_packets() {
        let ret = Expr::normal(Symbol::RETURN_PACKET, [Expr::from(42_i64)]);
        assert_eq!(classify_packet(&ret), PacketAction::Return(Expr::from(42_i64)));

        let msg = Expr::normal(
            Symbol::system("MessagePacket"),
            [Expr::symbol(Symbol::system("Power")), Expr::from("infy")],
        );
        assert_eq!(classify_packet(&msg), PacketAction::MessageHeader);

        let text = Expr::normal(Symbol::system("TextPacket"), [Expr::from("hello")]);
        assert_eq!(classify_packet(&text), PacketAction::Text("hello".into()));

        let input_name = Expr::normal(Symbol::system("InputNamePacket"), [Expr::from("In[1]:= ")]);
        assert_eq!(classify_packet(&input_name), PacketAction::Prompt);

        let other = Expr::from("raw");
        assert_eq!(classify_packet(&other), PacketAction::Unexpected);
    }

    #[test]
    fn builds_eval_and_to_expression_packets() {
        let eval_pkt = eval_packet_expr(&to_expression_expr("1 + 1"));
        let outer = eval_pkt.as_normal().expect("EvaluatePacket");
        assert!(outer.has_head(&Symbol::system("EvaluatePacket")));
        let inner = outer.elements()[0].as_normal().expect("ToExpression");
        assert!(inner.has_head(&Symbol::system("ToExpression")));
        assert_eq!(inner.elements()[0].as_str(), Some("1 + 1"));
    }

    #[test]
    fn wolfram_font_dir_appends_standard_subpath() {
        let dir = wolfram_truetype_font_dir(Path::new("C:/Wolfram"));
        assert!(dir.ends_with(Path::new("SystemFiles/Fonts/TrueType")));
    }
}
