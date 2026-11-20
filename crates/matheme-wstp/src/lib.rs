//! Shared WSTP link wrapper, evaluation helpers, and Wolfram installation
//! directory helpers for both the LibraryLink bridge (`matheme`) and the
//! standalone CLI (`matheme-cli`).
//!
//! Keeping all WSTP transport code in this crate preserves `matheme-wl`'s
//! guarantee of having zero WSTP / native-library dependency.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use matheme_wl::{Expr, Symbol};
use wolfram_expr::Symbol as RawSymbol;
pub use wstp::Link;

/// Wolfram installation path conventions shared by discovery and the bridge.
pub mod install {
    use super::*;

    /// Standard subdirectory under a Wolfram installation containing bundled
    /// TrueType fonts.
    pub fn wolfram_truetype_font_dir(install_dir: &Path) -> PathBuf {
        install_dir
            .join("SystemFiles")
            .join("Fonts")
            .join("TrueType")
    }
}

pub use install::wolfram_truetype_font_dir;

/// Structured error returned by [`WstpLink`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WstpError {
    /// Low-level WSTP transport or encoding error (preserving WSTP error code
    /// when available).
    Transport {
        code: Option<i32>,
        message: String,
    },
    /// Unexpected WSTP packet or malformed envelope.
    Protocol(String),
    /// Exceeded the configured maximum packet count before receiving a return packet.
    PacketLimit { max_packets: usize },
    /// Timed out waiting for data from the remote kernel or link peer.
    Timeout(String),
    /// Link/session was previously desynchronized by an error and refuses reuse.
    Poisoned(String),
}

impl WstpError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            code: None,
            message: message.into(),
        }
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }
}

impl From<wstp::Error> for WstpError {
    fn from(err: wstp::Error) -> Self {
        Self::Transport {
            code: err.code(),
            message: err.to_string(),
        }
    }
}

impl fmt::Display for WstpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { code: Some(code), message } => {
                write!(f, "WSTP transport error ({code}): {message}")
            }
            Self::Transport { code: None, message } => {
                write!(f, "WSTP transport error: {message}")
            }
            Self::Protocol(msg) => write!(f, "WSTP protocol error: {msg}"),
            Self::PacketLimit { max_packets } => {
                write!(f, "exceeded WSTP packet limit ({max_packets}) before ReturnPacket")
            }
            Self::Timeout(msg) => write!(f, "WSTP timeout: {msg}"),
            Self::Poisoned(msg) => write!(f, "WSTP session poisoned: {msg}"),
        }
    }
}

impl std::error::Error for WstpError {}

/// How [`WstpLink::read_return_packet`] treats `InputNamePacket` and
/// `OutputNamePacket` prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptPolicy {
    /// Skip prompt packets (standard for client sessions attached to a REPL kernel).
    Ignore,
    /// Fail immediately if a prompt packet appears unexpectedly (LibraryLink).
    Error,
}

/// Configuration for [`WstpLink::eval_with`] and [`WstpLink::read_return_packet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalOptions {
    pub max_packets: usize,
    pub prompts: PromptPolicy,
}

impl EvalOptions {
    /// Preset for `matheme-cli` client WSTP sessions.
    pub const fn client() -> Self {
        Self {
            max_packets: 256,
            prompts: PromptPolicy::Ignore,
        }
    }

    /// Preset for synchronous LibraryLink link callbacks.
    pub const fn library_link() -> Self {
        Self {
            max_packets: 16,
            prompts: PromptPolicy::Error,
        }
    }
}

/// Result of evaluating a Wolfram Language expression over WSTP, including any
/// sideband `MessagePacket` / `TextPacket` output received before `ReturnPacket`.
#[derive(Debug, Clone)]
pub struct EvalOutput {
    pub expr: Expr,
    pub messages: Vec<String>,
    pub prints: Vec<String>,
}

/// Borrowed wrapper around a [`wstp::Link`] providing context-resolving reads,
/// `Expr`-based I/O, handshake timeouts, and a unified `EvaluatePacket` → `ReturnPacket` flow.
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

    /// Wait up to `timeout` for incoming data on the link.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), WstpError> {
        let deadline = Instant::now() + timeout;
        while !self.link.is_ready() {
            if Instant::now() >= deadline {
                return Err(WstpError::Timeout(format!(
                    "no WSTP response within {:.1}s",
                    timeout.as_secs_f64()
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Read one expression from the link, resolving unqualified symbols via
    /// [`resolve_unqualified_symbol`].
    pub fn read_expr(&mut self) -> Result<Expr, WstpError> {
        self.link
            .get_expr_with_resolver(&mut |name: &str| Some(resolve_unqualified_symbol(name)))
            .map(Expr::from_raw)
            .map_err(WstpError::from)
    }

    /// Write an [`Expr`] to the link.
    pub fn put_expr(&mut self, expr: &Expr) -> Result<(), WstpError> {
        self.link.put_expr(expr.raw()).map_err(WstpError::from)
    }

    /// Flush pending outgoing packets on the link.
    pub fn flush(&mut self) -> Result<(), WstpError> {
        self.link.flush().map_err(WstpError::from)
    }

    /// Write `System\`EvaluatePacket[expr]` to the link without flushing or
    /// dispatching.
    pub fn put_eval_packet(&mut self, expr: &Expr) -> Result<(), WstpError> {
        let packet = eval_packet_expr(expr);
        self.put_expr(&packet)
    }

    /// Best-effort resynchronization after a protocol failure: advance past any
    /// partial packet and drain up to a bounded number of immediately-ready
    /// trailing packets so subsequent link traffic is not skewed.
    pub fn try_resync_packet(&mut self) {
        let _ = self.link.new_packet();
        for _ in 0..8 {
            if !self.link.is_ready() {
                break;
            }
            let Ok(pkt) = self.read_expr() else {
                self.link.clear_error();
                let _ = self.link.new_packet();
                break;
            };
            if matches!(classify_packet(&pkt), PacketAction::Return(_)) {
                break;
            }
        }
    }

    /// Drain startup packets until the kernel's initial `InputNamePacket`,
    /// waiting up to `timeout` for each incoming packet.
    pub fn drain_until_input_name(
        &mut self,
        max_packets: usize,
        timeout: Duration,
    ) -> Result<(), WstpError> {
        let input_name = Symbol::system("InputNamePacket");
        let deadline = Instant::now() + timeout;
        for _ in 0..max_packets {
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.wait_ready(remaining)?;
            let packet = self.read_expr()?;
            if packet
                .as_normal()
                .is_some_and(|n| n.has_head(&input_name))
            {
                return Ok(());
            }
        }
        Err(WstpError::protocol(
            "did not receive initial InputNamePacket from Wolfram kernel",
        ))
    }

    /// Read packets until a return packet arrives, collecting sideband
    /// `MessagePacket` / `TextPacket` output along the way.
    pub fn read_return_packet(&mut self, options: EvalOptions) -> Result<EvalOutput, WstpError> {
        let mut messages = Vec::new();
        let mut prints = Vec::new();
        let mut pending_message = false;

        for _ in 0..options.max_packets {
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
                PacketAction::Prompt if options.prompts == PromptPolicy::Ignore => continue,
                PacketAction::Prompt | PacketAction::Unexpected => {
                    return Err(WstpError::protocol(format!(
                        "unexpected WSTP packet while waiting for ReturnPacket: {packet}"
                    )));
                }
            }
        }

        Err(WstpError::PacketLimit {
            max_packets: options.max_packets,
        })
    }

    /// Unified evaluation pipeline:
    /// 1. Send `EvaluatePacket[expr]`;
    /// 2. Run `dispatch` (e.g. `flush()` for client links or `processWSLINK` for LibraryLink);
    /// 3. Drain packets until `ReturnPacket[result]`.
    pub fn eval_with<F>(
        &mut self,
        expr: &Expr,
        options: EvalOptions,
        dispatch: F,
    ) -> Result<EvalOutput, WstpError>
    where
        F: FnOnce(&mut Link) -> Result<(), WstpError>,
    {
        self.put_eval_packet(expr)?;
        dispatch(self.link)?;
        self.read_return_packet(options)
    }

    /// Evaluate a Wolfram Language expression on a client WSTP link using [`EvalOptions::client`].
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<EvalOutput, WstpError> {
        self.eval_with(expr, EvalOptions::client(), |link| {
            link.flush().map_err(WstpError::from)
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

/// Resolve an unqualified symbol name received over WSTP.
///
/// Known WSTP packet heads, `$...` kernel globals, and standard `System\``
/// builtins resolve to `System\``; all other unqualified symbols resolve to
/// `Global\`` instead of falsely promoting user symbols into `System\``.
pub fn resolve_unqualified_symbol(name: &str) -> RawSymbol {
    if name.starts_with('$') || is_known_system_symbol(name) {
        RawSymbol::new(&format!("System`{name}"))
    } else {
        RawSymbol::new(&format!("Global`{name}"))
    }
}

fn is_known_system_symbol(name: &str) -> bool {
    matches!(
        name,
        "EvaluatePacket"
            | "ReturnPacket"
            | "ReturnTextPacket"
            | "ReturnExpressionPacket"
            | "InputNamePacket"
            | "OutputNamePacket"
            | "MessagePacket"
            | "TextPacket"
            | "DisplayPacket"
            | "DisplayEndPacket"
            | "MenuPacket"
            | "InputPacket"
            | "EnterTextPacket"
            | "EnterExpressionPacket"
            | "SuspendPacket"
            | "ResumePacket"
            | "List"
            | "Rule"
            | "RuleDelayed"
            | "Association"
            | "Span"
            | "True"
            | "False"
            | "Null"
            | "None"
            | "All"
            | "Automatic"
            | "Full"
            | "ByteArray"
            | "Missing"
            | "Indeterminate"
            | "DirectedInfinity"
            | "Complex"
            | "Rational"
            | "Infinity"
            | "Overflow"
            | "Underflow"
            | "RGBColor"
            | "GrayLevel"
            | "Hue"
            | "CMYKColor"
            | "XYZColor"
            | "LABColor"
            | "LCHColor"
            | "LUVColor"
            | "Opacity"
            | "DateObject"
            | "TimeObject"
            | "Quantity"
            | "Failed"
            | "Aborted"
            | "Canceled"
            | "Hold"
            | "HoldForm"
            | "HoldComplete"
            | "HoldPattern"
            | "Sequence"
            | "Graphics"
            | "Pi"
            | "E"
            | "I"
            | "Degree"
            | "GoldenRatio"
            | "Catalan"
            | "EulerGamma"
            | "Glaisher"
            | "Khinchin"
            | "Plus"
            | "Times"
            | "Power"
            | "Equal"
            | "Less"
            | "Greater"
            | "And"
            | "Or"
            | "Not"
            | "Sin"
            | "Cos"
            | "Tan"
            | "Exp"
            | "Log"
            | "Sqrt"
            | "Left"
            | "Right"
            | "Top"
            | "Bottom"
            | "Center"
            | "ToExpression"
            | "Exit"
            | "PlotRange"
            | "PlotRangePadding"
            | "PlotRangeClipping"
            | "ImagePadding"
            | "ImageSize"
            | "AspectRatio"
            | "FilledCurve"
            | "JoinedCurve"
            | "CurveClosed"
            | "GeometricTransformation"
            | "Inset"
            | "Style"
            | "FaceForm"
            | "EdgeForm"
            | "Thickness"
            | "Dashing"
            | "CapForm"
            | "JoinForm"
            | "Line"
            | "Point"
            | "Polygon"
            | "Rectangle"
            | "Circle"
            | "Disk"
    )
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
    if (normal.has_head(&Symbol::RETURN_PACKET)
        || normal.has_head(&Symbol::system("ReturnTextPacket"))
        || normal.has_head(&Symbol::system("ReturnExpressionPacket")))
        && normal.len() == 1
    {
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
    fn classifies_standard_and_variant_return_packets() {
        let ret = Expr::normal(Symbol::RETURN_PACKET, [Expr::from(42_i64)]);
        assert_eq!(classify_packet(&ret), PacketAction::Return(Expr::from(42_i64)));

        let ret_text = Expr::normal(Symbol::system("ReturnTextPacket"), [Expr::from("ok")]);
        assert_eq!(classify_packet(&ret_text), PacketAction::Return(Expr::from("ok")));

        let ret_expr = Expr::normal(
            Symbol::system("ReturnExpressionPacket"),
            [Expr::from(7_i64)],
        );
        assert_eq!(classify_packet(&ret_expr), PacketAction::Return(Expr::from(7_i64)));

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
    fn resolves_unqualified_system_and_user_symbols_accurately() {
        assert_eq!(resolve_unqualified_symbol("ReturnPacket").as_str(), "System`ReturnPacket");
        assert_eq!(resolve_unqualified_symbol("List").as_str(), "System`List");
        assert_eq!(
            resolve_unqualified_symbol("$InstallationDirectory").as_str(),
            "System`$InstallationDirectory"
        );
        assert_eq!(resolve_unqualified_symbol("myVar").as_str(), "Global`myVar");
        assert_eq!(resolve_unqualified_symbol("x").as_str(), "Global`x");
    }

    #[test]
    fn eval_options_and_errors_expose_structured_presets() {
        let client = EvalOptions::client();
        assert_eq!(client.max_packets, 256);
        assert_eq!(client.prompts, PromptPolicy::Ignore);

        let ll = EvalOptions::library_link();
        assert_eq!(ll.max_packets, 16);
        assert_eq!(ll.prompts, PromptPolicy::Error);

        let err = WstpError::PacketLimit { max_packets: 16 };
        assert!(err.to_string().contains("16"));
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
