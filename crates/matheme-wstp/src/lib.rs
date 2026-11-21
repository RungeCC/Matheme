//! Shared WSTP link wrapper, persistent [`KernelSession`], WSTP protocol
//! symbols, and Wolfram installation helpers for both the LibraryLink bridge
//! (`matheme`) and the standalone CLI (`matheme-cli`).
//!
//! Keeping all WSTP transport code in this crate preserves `matheme-wl`'s
//! guarantee of having zero WSTP / native-library dependency.

pub mod kernel;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use matheme_wl::{Expr, Symbol};
use wolfram_expr::Symbol as RawSymbol;
pub use wstp::Link;

pub use kernel::{KernelError, KernelSession};

/// Default maximum startup packets drained while waiting for `InputNamePacket`.
pub const DEFAULT_HANDSHAKE_MAX_PACKETS: usize = 32;

/// Default evaluation timeout for client WSTP sessions (`matheme-cli`).
pub const DEFAULT_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default evaluation timeout for synchronous LibraryLink link callbacks.
pub const DEFAULT_LIBRARY_LINK_TIMEOUT: Duration = Duration::from_secs(5);

/// Default timeout spent draining trailing packets during post-error resync.
pub const DEFAULT_RESYNC_TIMEOUT: Duration = Duration::from_millis(250);

/// Function type used by [`WstpLink`] to qualify unqualified symbol names read
/// over WSTP.
pub type SymbolResolver = fn(&str) -> RawSymbol;

/// WSTP wire-protocol packet heads and kernel control symbols.
pub mod protocol {
    use matheme_wl::Symbol;

    pub const EVALUATE_PACKET: Symbol = Symbol::from_static("System`EvaluatePacket");
    pub const RETURN_PACKET: Symbol = Symbol::from_static("System`ReturnPacket");
    pub const RETURN_TEXT_PACKET: Symbol = Symbol::from_static("System`ReturnTextPacket");
    pub const RETURN_EXPRESSION_PACKET: Symbol =
        Symbol::from_static("System`ReturnExpressionPacket");
    pub const INPUT_NAME_PACKET: Symbol = Symbol::from_static("System`InputNamePacket");
    pub const OUTPUT_NAME_PACKET: Symbol = Symbol::from_static("System`OutputNamePacket");
    pub const MESSAGE_PACKET: Symbol = Symbol::from_static("System`MessagePacket");
    pub const TEXT_PACKET: Symbol = Symbol::from_static("System`TextPacket");
    pub const TO_EXPRESSION: Symbol = Symbol::from_static("System`ToExpression");
    pub const EXIT: Symbol = Symbol::from_static("System`Exit");
}

/// Wolfram installation path conventions and kernel discovery globals shared by
/// discovery and the LibraryLink bridge.
pub mod install {
    use super::*;

    /// ``System`$InstallationDirectory``.
    pub const INSTALLATION_DIRECTORY: Symbol = Symbol::from_static("System`$InstallationDirectory");

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
    /// Link/session was left desynchronized (resync failed or transport broke)
    /// and must be reconnected before reuse.
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

    /// Returns `true` when the link cannot be safely reused without reconnecting.
    pub fn is_fatal_to_link(&self) -> bool {
        matches!(self, Self::Poisoned(_) | Self::Transport { .. })
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
            Self::Transport {
                code: Some(code),
                message,
            } => write!(f, "WSTP transport error ({code}): {message}"),
            Self::Transport {
                code: None,
                message,
            } => write!(f, "WSTP transport error: {message}"),
            Self::Protocol(msg) => write!(f, "WSTP protocol error: {msg}"),
            Self::PacketLimit { max_packets } => {
                write!(
                    f,
                    "exceeded WSTP packet limit ({max_packets}) before ReturnPacket"
                )
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

/// How [`WstpLink::eval_with`] recovers when reading a `ReturnPacket` fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPolicy {
    /// Do not attempt packet resynchronization; return the original error as-is.
    None,
    /// Drain up to `max_packets` within `resync_timeout` until a `ReturnPacket`
    /// is consumed; upgrade to [`WstpError::Poisoned`] if resync does not succeed.
    Resync,
}

/// Configuration for [`WstpLink::eval_with`] and [`WstpLink::read_return_packet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalOptions {
    pub max_packets: usize,
    pub timeout: Duration,
    pub resync_timeout: Duration,
    pub prompts: PromptPolicy,
    pub recovery: RecoveryPolicy,
}

impl EvalOptions {
    /// Preset for `matheme-cli` client WSTP sessions.
    pub const fn client() -> Self {
        Self {
            max_packets: 256,
            timeout: DEFAULT_CLIENT_TIMEOUT,
            resync_timeout: DEFAULT_RESYNC_TIMEOUT,
            prompts: PromptPolicy::Ignore,
            recovery: RecoveryPolicy::Resync,
        }
    }

    /// Preset for synchronous LibraryLink link callbacks.
    pub const fn library_link() -> Self {
        Self {
            max_packets: 16,
            timeout: DEFAULT_LIBRARY_LINK_TIMEOUT,
            resync_timeout: DEFAULT_RESYNC_TIMEOUT,
            prompts: PromptPolicy::Error,
            recovery: RecoveryPolicy::Resync,
        }
    }

    /// Number of trailing packets to drain when resynchronizing after a failed
    /// evaluation, derived directly from `max_packets`.
    pub const fn resync_limit(&self) -> usize {
        self.max_packets
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

/// Borrowed wrapper around a [`wstp::Link`] providing configurable
/// context-resolving reads, `Expr`-based I/O, bounded deadlines, and a unified
/// `EvaluatePacket` → `ReturnPacket` flow with automatic packet resync.
pub struct WstpLink<'a> {
    link: &'a mut Link,
    resolver: SymbolResolver,
}

impl<'a> WstpLink<'a> {
    pub fn new(link: &'a mut Link) -> Self {
        Self {
            link,
            resolver: resolve_unqualified_symbol,
        }
    }

    /// Override the unqualified symbol resolver used by [`Self::read_expr`].
    pub fn with_resolver(mut self, resolver: SymbolResolver) -> Self {
        self.resolver = resolver;
        self
    }

    /// Borrow the underlying raw [`wstp::Link`] for transport-specific calls
    /// (such as LibraryLink's `processWSLINK` callback).
    pub fn raw(&mut self) -> &mut Link {
        self.link
    }

    /// Wait up to `timeout` for incoming data on the link.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), WstpError> {
        if self.link.is_ready() {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        while !self.link.is_ready() {
            let now = Instant::now();
            if now >= deadline {
                return Err(WstpError::Timeout(format!(
                    "no WSTP response within {:.1}s",
                    timeout.as_secs_f64()
                )));
            }
            let remaining = deadline - now;
            std::thread::sleep(remaining.min(Duration::from_millis(5)));
        }
        Ok(())
    }

    /// Read one expression from the link, resolving unqualified symbols via the
    /// configured [`SymbolResolver`].
    pub fn read_expr(&mut self) -> Result<Expr, WstpError> {
        let resolver = self.resolver;
        self.link
            .get_expr_with_resolver(&mut |name: &str| Some(resolver(name)))
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

    /// Write ``System`EvaluatePacket[expr]`` to the link without flushing or
    /// dispatching.
    pub fn put_eval_packet(&mut self, expr: &Expr) -> Result<(), WstpError> {
        let packet = eval_packet_expr(expr);
        self.put_expr(&packet)
    }

    /// Resynchronize after a protocol, packet-limit, or timeout failure by
    /// advancing past any partial packet and draining up to `max_packets`
    /// within `timeout` until a `ReturnPacket` is consumed.
    ///
    /// Returns `true` only when a `ReturnPacket` was cleanly drained so the
    /// link is back in sync; returns `false` if resync timed out, errored, or
    /// hit `max_packets` without seeing a return packet.
    pub fn resync_until_return(&mut self, max_packets: usize, timeout: Duration) -> bool {
        if self.link.new_packet().is_err() {
            self.link.clear_error();
            if self.link.new_packet().is_err() {
                return false;
            }
        }
        let deadline = Instant::now() + timeout;
        for _ in 0..max_packets {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if self.wait_ready(remaining).is_err() {
                return false;
            }
            let Ok(pkt) = self.read_expr() else {
                self.link.clear_error();
                let _ = self.link.new_packet();
                return false;
            };
            if matches!(classify_packet(&pkt), PacketAction::Return(_)) {
                return true;
            }
        }
        false
    }

    /// Drain startup packets until the kernel's initial `InputNamePacket`,
    /// waiting up to `timeout` total across all startup packets.
    pub fn drain_until_input_name(
        &mut self,
        max_packets: usize,
        timeout: Duration,
    ) -> Result<(), WstpError> {
        let deadline = Instant::now() + timeout;
        for _ in 0..max_packets {
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.wait_ready(remaining)?;
            let packet = self.read_expr()?;
            if packet
                .as_normal()
                .is_some_and(|n| n.has_head(&protocol::INPUT_NAME_PACKET))
            {
                return Ok(());
            }
        }
        Err(WstpError::protocol(
            "did not receive initial InputNamePacket from Wolfram kernel",
        ))
    }

    /// Read packets until a return packet arrives within `options.timeout` and
    /// `options.max_packets`, collecting sideband `MessagePacket` / `TextPacket`
    /// output along the way.
    pub fn read_return_packet(&mut self, options: EvalOptions) -> Result<EvalOutput, WstpError> {
        let mut messages = Vec::new();
        let mut prints = Vec::new();
        let mut pending_message = false;
        let deadline = Instant::now() + options.timeout;

        for _ in 0..options.max_packets {
            let remaining = deadline.saturating_duration_since(Instant::now());
            self.wait_ready(remaining)?;
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
    /// 3. Drain packets until `ReturnPacket[result]` within `options.timeout`;
    /// 4. On protocol / packet-limit / timeout failure, apply `options.recovery`
    ///    and return [`WstpError::Poisoned`] if resynchronization fails.
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
        match self.read_return_packet(options) {
            Ok(out) => Ok(out),
            Err(err) => {
                if options.recovery == RecoveryPolicy::Resync
                    && matches!(
                        err,
                        WstpError::Protocol(_)
                            | WstpError::PacketLimit { .. }
                            | WstpError::Timeout(_)
                    )
                {
                    if self.resync_until_return(options.resync_limit(), options.resync_timeout) {
                        Err(err)
                    } else {
                        Err(WstpError::Poisoned(format!(
                            "{err}; failed to resynchronize WSTP packet stream"
                        )))
                    }
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Evaluate a Wolfram Language expression on a client WSTP link using [`EvalOptions::client`].
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<EvalOutput, WstpError> {
        self.eval_with(expr, EvalOptions::client(), |link| {
            link.flush().map_err(WstpError::from)
        })
    }

    /// Evaluate a Wolfram Language code string via ``System`ToExpression[code]``.
    pub fn eval_code(&mut self, wl_code: &str) -> Result<EvalOutput, WstpError> {
        self.eval_expr(&to_expression_expr(wl_code))
    }

    /// Send `EvaluatePacket[Exit[]]` and flush, returning any transport error.
    pub fn send_exit(&mut self) -> Result<(), WstpError> {
        let exit = eval_packet_expr(&Expr::normal(&protocol::EXIT, Vec::<Expr>::new()));
        self.put_expr(&exit)?;
        self.flush()
    }
}

/// Resolve an unqualified symbol name received over WSTP into `` System` ``.
///
/// The Wolfram Kernel transmits non-`` System` `` symbols (including `` Global` ``
/// and package contexts) fully qualified on WSTP links, and only strips the
/// `` System` `` context prefix — matching `wolfram-library-link`'s own WSTP
/// argument reader. Callers with custom `$Context` / `$ContextPath` rules may
/// override this via [`WstpLink::with_resolver`].
pub fn resolve_unqualified_symbol(name: &str) -> RawSymbol {
    let mut buf = String::with_capacity(7 + name.len());
    buf.push_str("System`");
    buf.push_str(name);
    RawSymbol::new(&buf)
}

fn eval_packet_expr(expr: &Expr) -> Expr {
    Expr::normal(&protocol::EVALUATE_PACKET, [expr.clone()])
}

fn to_expression_expr(wl_code: &str) -> Expr {
    Expr::normal(&protocol::TO_EXPRESSION, [Expr::string(wl_code)])
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
    if (normal.has_head(&protocol::RETURN_PACKET)
        || normal.has_head(&protocol::RETURN_TEXT_PACKET)
        || normal.has_head(&protocol::RETURN_EXPRESSION_PACKET))
        && normal.len() == 1
    {
        return PacketAction::Return(normal.elements()[0].clone());
    }
    if normal.has_head(&protocol::MESSAGE_PACKET) {
        return PacketAction::MessageHeader;
    }
    if normal.has_head(&protocol::TEXT_PACKET) && normal.len() == 1 {
        let elem = &normal.elements()[0];
        let text = elem
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| elem.to_string());
        return PacketAction::Text(text);
    }
    if normal.has_head(&protocol::INPUT_NAME_PACKET)
        || normal.has_head(&protocol::OUTPUT_NAME_PACKET)
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
        let ret = Expr::normal(&protocol::RETURN_PACKET, [Expr::from(42_i64)]);
        assert_eq!(
            classify_packet(&ret),
            PacketAction::Return(Expr::from(42_i64))
        );

        let ret_text = Expr::normal(&protocol::RETURN_TEXT_PACKET, [Expr::from("ok")]);
        assert_eq!(
            classify_packet(&ret_text),
            PacketAction::Return(Expr::from("ok"))
        );

        let ret_expr = Expr::normal(&protocol::RETURN_EXPRESSION_PACKET, [Expr::from(7_i64)]);
        assert_eq!(
            classify_packet(&ret_expr),
            PacketAction::Return(Expr::from(7_i64))
        );

        let msg = Expr::normal(
            &protocol::MESSAGE_PACKET,
            [Expr::symbol(Symbol::system("Power")), Expr::from("infy")],
        );
        assert_eq!(classify_packet(&msg), PacketAction::MessageHeader);

        let text = Expr::normal(&protocol::TEXT_PACKET, [Expr::from("hello")]);
        assert_eq!(classify_packet(&text), PacketAction::Text("hello".into()));

        let input_name = Expr::normal(&protocol::INPUT_NAME_PACKET, [Expr::from("In[1]:= ")]);
        assert_eq!(classify_packet(&input_name), PacketAction::Prompt);

        let other = Expr::from("raw");
        assert_eq!(classify_packet(&other), PacketAction::Unexpected);
    }

    #[test]
    fn resolves_unqualified_symbols_to_system_context() {
        assert_eq!(
            resolve_unqualified_symbol("ReturnPacket").as_str(),
            "System`ReturnPacket"
        );
        assert_eq!(
            resolve_unqualified_symbol("Integrate").as_str(),
            "System`Integrate"
        );
        assert_eq!(resolve_unqualified_symbol("Show").as_str(), "System`Show");
        assert_eq!(
            resolve_unqualified_symbol("ImportByteArray").as_str(),
            "System`ImportByteArray"
        );
        assert_eq!(
            resolve_unqualified_symbol("$InstallationDirectory").as_str(),
            "System`$InstallationDirectory"
        );
    }

    #[test]
    fn eval_options_and_errors_expose_structured_presets() {
        let client = EvalOptions::client();
        assert_eq!(client.max_packets, 256);
        assert_eq!(client.resync_limit(), 256);
        assert_eq!(client.timeout, DEFAULT_CLIENT_TIMEOUT);
        assert_eq!(client.prompts, PromptPolicy::Ignore);
        assert_eq!(client.recovery, RecoveryPolicy::Resync);

        let ll = EvalOptions::library_link();
        assert_eq!(ll.max_packets, 16);
        assert_eq!(ll.resync_limit(), 16);
        assert_eq!(ll.timeout, DEFAULT_LIBRARY_LINK_TIMEOUT);
        assert_eq!(ll.prompts, PromptPolicy::Error);
        assert_eq!(ll.recovery, RecoveryPolicy::Resync);

        let err = WstpError::PacketLimit { max_packets: 16 };
        assert!(err.to_string().contains("16"));
        assert!(!err.is_fatal_to_link());
        assert!(WstpError::Poisoned("desync".into()).is_fatal_to_link());
        assert!(WstpError::transport("broken").is_fatal_to_link());
    }

    #[test]
    fn builds_eval_and_to_expression_packets() {
        let eval_pkt = eval_packet_expr(&to_expression_expr("1 + 1"));
        let outer = eval_pkt.as_normal().expect("EvaluatePacket");
        assert!(outer.has_head(&protocol::EVALUATE_PACKET));
        let inner = outer.elements()[0].as_normal().expect("ToExpression");
        assert!(inner.has_head(&protocol::TO_EXPRESSION));
        assert_eq!(inner.elements()[0].as_str(), Some("1 + 1"));
    }

    #[test]
    fn wolfram_font_dir_appends_standard_subpath() {
        let dir = wolfram_truetype_font_dir(Path::new("C:/Wolfram"));
        assert!(dir.ends_with(Path::new("SystemFiles/Fonts/TrueType")));
    }
}
