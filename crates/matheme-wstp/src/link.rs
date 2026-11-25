//! Borrowed WSTP link wrapper: expression I/O, deadline handling, and the
//! `EvaluatePacket` → `ReturnPacket` evaluation pipeline.

use std::time::{Duration, Instant};

use matheme_wl::Expr;
use wolfram_expr::{BigInteger, BigReal, Expr as RawExpr, Symbol as RawSymbol};
use wstp::{Link, TokenType, sys};

use crate::error::WstpError;
use crate::protocol::{self, PacketAction, classify_packet, eval_packet_expr, to_expression_expr};

/// Default maximum startup packets drained while waiting for `InputNamePacket`.
pub(crate) const DEFAULT_HANDSHAKE_MAX_PACKETS: usize = 32;

/// Default evaluation timeout for client WSTP sessions (`matheme-cli`).
const DEFAULT_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default evaluation timeout for synchronous LibraryLink link callbacks.
const DEFAULT_LIBRARY_LINK_TIMEOUT: Duration = Duration::from_secs(5);

/// Default timeout spent draining trailing packets during post-error resync.
const DEFAULT_RESYNC_TIMEOUT: Duration = Duration::from_millis(250);

/// Peek at the physical storage type of the current number token.
///
/// `WSGetType` reports both machine and arbitrary-precision numbers as
/// `WSTKINT` / `WSTKREAL`; `WSGetRawType` distinguishes them. Machine numbers
/// carry a physical width code (`WSTK_WSINT` = 228, `WSTK_CDOUBLE` = 246 here),
/// while numbers transmitted as arbitrary-precision decimal text carry the
/// logical `WSTKINT` = 43 / `WSTKREAL` = 42 codes and must be read through
/// `WSGetNumberAsString`. Confirmed with Wolfram 15.0: `42` → 228,
/// `2^100` → 43, `1.5` → 246, `N[Pi, 50]` → 42.
fn raw_number_type(link: &Link) -> i32 {
    // SAFETY: The borrowed Link keeps the handle alive. WSGetRawType peeks at
    // the current token without taking ownership of the link.
    unsafe { sys::WSGetRawType(link.raw_link()) }
}

/// Read one raw expression, preserving arbitrary-precision numbers that
/// `wstp::Link::get_expr` cannot represent.
fn read_expr(
    link: &mut Link,
    resolve_symbol: fn(&str) -> RawSymbol,
    depth: usize,
) -> Result<RawExpr, WstpError> {
    if depth == 256 {
        return Err(WstpError::protocol("WSTP expression nested too deeply"));
    }
    Ok(match link.get_type()? {
        TokenType::Integer => {
            let raw_type = raw_number_type(link);
            if raw_type == i32::from(sys::WSTKINT) {
                let text = link.get_number_as_string()?;
                match text.parse::<i64>() {
                    Ok(value) => RawExpr::from(value),
                    Err(_) => RawExpr::from(BigInteger(text)),
                }
            } else {
                RawExpr::from(link.get_i64()?)
            }
        }
        TokenType::Real => {
            let raw_type = raw_number_type(link);
            if raw_type == i32::from(sys::WSTKREAL) {
                RawExpr::from(BigReal(link.get_number_as_string()?))
            } else {
                let value = link.get_f64()?;
                if !value.is_finite() {
                    return Err(WstpError::protocol("non-finite WSTP real"));
                }
                RawExpr::from(value)
            }
        }
        TokenType::String => RawExpr::string(link.get_string_ref()?.as_str()),
        TokenType::Symbol => {
            let name = link.get_symbol_ref()?;
            let name = name.as_str();
            RawExpr::symbol(RawSymbol::try_new(name).unwrap_or_else(|| resolve_symbol(name)))
        }
        TokenType::Function => {
            let arity = link.get_arg_count()?;
            let head = read_expr(link, resolve_symbol, depth + 1)?;
            let mut arguments = Vec::with_capacity(arity.min(4096));
            for _ in 0..arity {
                arguments.push(read_expr(link, resolve_symbol, depth + 1)?);
            }
            RawExpr::normal(head, arguments)
        }
    })
}

/// How [`WstpLink::read_return_packet`] treats `InputNamePacket` and
/// `OutputNamePacket` prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptPolicy {
    /// Skip prompt packets (standard for client sessions attached to a REPL kernel).
    Ignore,
    /// Fail immediately if a prompt packet appears unexpectedly (LibraryLink).
    Error,
}

/// Configuration for [`WstpLink::eval_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalOptions {
    max_packets: usize,
    timeout: Duration,
    resync_timeout: Duration,
    prompts: PromptPolicy,
}

impl EvalOptions {
    /// Preset for `matheme-cli` client WSTP sessions.
    pub const fn client() -> Self {
        Self {
            max_packets: 256,
            timeout: DEFAULT_CLIENT_TIMEOUT,
            resync_timeout: DEFAULT_RESYNC_TIMEOUT,
            prompts: PromptPolicy::Ignore,
        }
    }

    /// Preset for synchronous LibraryLink link callbacks.
    pub const fn library_link() -> Self {
        Self {
            max_packets: 16,
            timeout: DEFAULT_LIBRARY_LINK_TIMEOUT,
            resync_timeout: DEFAULT_RESYNC_TIMEOUT,
            prompts: PromptPolicy::Error,
        }
    }

    /// Number of trailing packets to drain when resynchronizing after a failed
    /// evaluation, derived directly from `max_packets`.
    const fn resync_limit(&self) -> usize {
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

/// Borrowed wrapper around a [`wstp::Link`] providing context-resolving reads,
/// `Expr`-based I/O, bounded deadlines, and a unified `EvaluatePacket` →
/// `ReturnPacket` flow with automatic packet resync.
pub struct WstpLink<'a> {
    link: &'a mut Link,
}

impl<'a> WstpLink<'a> {
    pub fn new(link: &'a mut Link) -> Self {
        Self { link }
    }

    /// Wait up to `timeout` for incoming data on the link.
    fn wait_ready(&self, timeout: Duration) -> Result<(), WstpError> {
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

    /// Read one expression from the link, resolving unqualified symbols to
    /// `` System` `` and preserving arbitrary-precision numbers.
    pub fn read_expr(&mut self) -> Result<Expr, WstpError> {
        read_expr(self.link, resolve_unqualified_symbol, 0).map(Expr::from_raw)
    }

    /// Write an [`Expr`] to the link.
    pub fn put_expr(&mut self, expr: &Expr) -> Result<(), WstpError> {
        self.link.put_expr(expr.raw()).map_err(WstpError::from)
    }

    /// Flush pending outgoing packets on the link.
    fn flush(&mut self) -> Result<(), WstpError> {
        self.link.flush().map_err(WstpError::from)
    }

    /// Write ``System`EvaluatePacket[expr]`` to the link without flushing or
    /// dispatching.
    fn put_eval_packet(&mut self, expr: &Expr) -> Result<(), WstpError> {
        let packet = eval_packet_expr(expr);
        self.put_expr(&packet)
    }

    /// Resynchronize after a protocol, packet-limit, or timeout failure by
    /// advancing past any partial packet and draining up to `max_packets`
    /// within `timeout` until a `ReturnPacket` is consumed.
    fn resync_until_return(&mut self, max_packets: usize, timeout: Duration) -> bool {
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
    pub(crate) fn drain_until_input_name(
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
    fn read_return_packet(&mut self, options: EvalOptions) -> Result<EvalOutput, WstpError> {
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
    /// 4. On protocol / packet-limit / timeout failure, resynchronize and return
    ///    [`WstpError::Poisoned`] if that fails.
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
                if matches!(
                    err,
                    WstpError::Protocol(_) | WstpError::PacketLimit { .. } | WstpError::Timeout(_)
                ) {
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
    pub(crate) fn eval_expr(&mut self, expr: &Expr) -> Result<EvalOutput, WstpError> {
        self.eval_with(expr, EvalOptions::client(), |link| {
            link.flush().map_err(WstpError::from)
        })
    }

    /// Evaluate a Wolfram Language code string via ``System`ToExpression[code]``.
    pub(crate) fn eval_code(&mut self, wl_code: &str) -> Result<EvalOutput, WstpError> {
        self.eval_expr(&to_expression_expr(wl_code))
    }

    /// Send `EvaluatePacket[Exit[]]` and flush, returning any transport error.
    pub(crate) fn send_exit(&mut self) -> Result<(), WstpError> {
        let exit = eval_packet_expr(&Expr::normal(&protocol::EXIT, std::iter::empty::<Expr>()));
        self.put_expr(&exit)?;
        self.flush()
    }
}

/// Resolve an unqualified symbol name received over WSTP into `` System` ``.
///
/// The Wolfram Kernel transmits non-`` System` `` symbols (including `` Global` ``
/// and package contexts) fully qualified on WSTP links, and only strips the
/// `` System` `` context prefix — matching `wolfram-library-link`'s own WSTP
/// argument reader.
pub fn resolve_unqualified_symbol(name: &str) -> RawSymbol {
    let mut buf = String::with_capacity(7 + name.len());
    buf.push_str("System`");
    buf.push_str(name);
    RawSymbol::new(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use wolfram_expr::ExprKind;

    #[test]
    fn arbitrary_precision_real_is_read_as_big_real() {
        // WSGetType reports WSTKREAL for both machine and arbitrary-precision
        // reals; only WSGetRawType distinguishes them. Put a decimal-string
        // real on a loopback link to exercise that branch.
        let mut link = Link::new_loopback().unwrap();
        let text = CString::new("3.14159265358979323846`20.").unwrap();
        let written = unsafe { sys::WSPutRealNumberAsString(link.raw_link(), text.as_ptr()) };
        assert_ne!(written, 0, "failed to put arbitrary-precision real");

        let expr = read_expr(&mut link, resolve_unqualified_symbol, 0).unwrap();
        let ExprKind::BigReal(real) = expr.kind() else {
            panic!("expected BigReal, got {expr:?}")
        };
        assert!(real.as_str().starts_with("3.14159"), "{real:?}");
    }
}
