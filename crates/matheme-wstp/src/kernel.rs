//! Persistent WSTP client session communicating with an external Wolfram Kernel.
//!
//! Launches a local `WolframKernel` executable or connects to an existing WSTP
//! link by name, runs the startup handshake, tracks owned-vs-attached exit
//! policy and link poisoning, and supports reconnecting after fatal link errors.

use std::path::{Path, PathBuf};
use std::time::Duration;

use matheme_wl::Expr;

use crate::{EvalOutput, Link, WstpError, WstpLink, DEFAULT_HANDSHAKE_MAX_PACKETS};

/// Error returned by [`KernelSession`] operations.
pub type KernelError = WstpError;

const LAUNCH_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
enum SessionTarget {
    Launch(PathBuf),
    Connect(String),
}

/// Tracks whether a session owns its kernel process and has already sent `Exit[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExitPolicy {
    owns_kernel: bool,
    closed: bool,
}

impl ExitPolicy {
    fn owned() -> Self {
        Self {
            owns_kernel: true,
            closed: false,
        }
    }

    fn attached() -> Self {
        Self {
            owns_kernel: false,
            closed: false,
        }
    }

    /// Returns `true` at most once, and only when this session owns the kernel.
    fn take_exit(&mut self) -> bool {
        if !self.owns_kernel || self.closed {
            return false;
        }
        self.closed = true;
        true
    }
}

/// A live WSTP session attached to a `WolframKernel` process.
pub struct KernelSession {
    link: Link,
    target: SessionTarget,
    exit_policy: ExitPolicy,
    /// When `true` (owned launched kernels), the kernel's top-level `$Context`
    /// is isolated to ``"Runge`Matheme`WSTP`"`` with ``$ContextPath = {"System`"}`` so
    /// WSTP transmits all non-`` System` `` symbols (including `` Global` ``) with
    /// explicit context prefixes, while user code still evaluates in the user's
    /// logical `$Context` / `$ContextPath`.
    isolate_wire_context: bool,
    poisoned: bool,
}

/// Initialization snippet run once on owned launched kernels so WSTP
/// serialization preserves exact contexts for all non-`` System` `` symbols.
const WIRE_CONTEXT_INIT_WL: &str = r#"
Runge`Matheme`WSTP`$Context = System`$Context;
Runge`Matheme`WSTP`$ContextPath = System`$ContextPath;
System`$Context = "Runge`Matheme`WSTP`";
System`$ContextPath = {"System`"};
System`Null
"#;

/// Build an expression that evaluates `wl_code` in the session's logical
/// `$Context` / `$ContextPath` (persisting any `Needs[...]` / `Begin[...]`
/// changes) while letting the kernel return under the isolated wire context.
fn isolated_eval_expr(wl_code: &str) -> Expr {
    use matheme_wl::{list, normal, Symbol};
    let ctx_var = Symbol::from_static("Runge`Matheme`WSTP`$Context");
    let path_var = Symbol::from_static("Runge`Matheme`WSTP`$ContextPath");
    let sys_ctx = Symbol::from_static("System`$Context");
    let sys_path = Symbol::from_static("System`$ContextPath");
    let set = Symbol::from_static("System`Set");
    let block = Symbol::from_static("System`Block");
    let with_local = Symbol::from_static("Internal`WithLocalSettings");
    let compound = Symbol::from_static("System`CompoundExpression");

    let bindings = list![
        normal!(&set, &sys_ctx, &ctx_var),
        normal!(&set, &sys_path, &path_var),
    ];
    let body = normal!(&crate::protocol::TO_EXPRESSION, wl_code);
    let save_ctx = normal!(
        &compound,
        normal!(&set, &ctx_var, &sys_ctx),
        normal!(&set, &path_var, &sys_path),
    );
    let inner = normal!(&with_local, Expr::null(), body, save_ctx);
    normal!(&block, bindings, inner)
}

impl KernelSession {
    fn open_target(target: &SessionTarget) -> Result<(Link, ExitPolicy, bool), KernelError> {
        match target {
            SessionTarget::Launch(kernel_path) => {
                // WSTP launch syntax quotes the executable path inside -linkname.
                let path_str = kernel_path.to_string_lossy();
                let linkname = format!("\"{path_str}\" -wstp");
                let argv = [
                    "matheme",
                    "-linkmode",
                    "launch",
                    "-linkname",
                    &linkname,
                ];
                let mut link = Link::open_with_args(&argv).map_err(|err| {
                    KernelError::transport(format!(
                        "failed to launch Wolfram kernel ({path_str}): {err}"
                    ))
                })?;
                link.activate().map_err(|err| {
                    KernelError::transport(format!("failed to activate WSTP link: {err}"))
                })?;
                let mut wstp = WstpLink::new(&mut link);
                wstp.drain_until_input_name(
                    DEFAULT_HANDSHAKE_MAX_PACKETS,
                    LAUNCH_HANDSHAKE_TIMEOUT,
                )?;
                wstp.eval_code(WIRE_CONTEXT_INIT_WL)?;
                Ok((link, ExitPolicy::owned(), true))
            }
            SessionTarget::Connect(link_name) => {
                let argv = [
                    "matheme",
                    "-linkmode",
                    "connect",
                    "-linkname",
                    link_name.as_str(),
                ];
                let mut link = Link::open_with_args(&argv).map_err(|err| {
                    KernelError::transport(format!(
                        "failed to connect to WSTP link {link_name:?}: {err}"
                    ))
                })?;
                link.activate().map_err(|err| {
                    KernelError::transport(format!("failed to activate WSTP link: {err}"))
                })?;
                WstpLink::new(&mut link).drain_until_input_name(
                    DEFAULT_HANDSHAKE_MAX_PACKETS,
                    CONNECT_HANDSHAKE_TIMEOUT,
                )?;
                // Never mutate `$Context` / `$ContextPath` on an externally
                // owned kernel attached via `--link-name`.
                Ok((link, ExitPolicy::attached(), false))
            }
        }
    }

    /// Launch a local `WolframKernel` executable and connect over WSTP.
    pub fn launch(kernel_path: &Path) -> Result<Self, KernelError> {
        let target = SessionTarget::Launch(kernel_path.to_path_buf());
        let (link, exit_policy, isolate_wire_context) = Self::open_target(&target)?;
        Ok(Self {
            link,
            target,
            exit_policy,
            isolate_wire_context,
            poisoned: false,
        })
    }

    /// Connect to an already-running WSTP link by name.
    pub fn connect(link_name: &str) -> Result<Self, KernelError> {
        let target = SessionTarget::Connect(link_name.to_owned());
        let (link, exit_policy, isolate_wire_context) = Self::open_target(&target)?;
        Ok(Self {
            link,
            target,
            exit_policy,
            isolate_wire_context,
            poisoned: false,
        })
    }

    /// Re-establish a poisoned or broken WSTP session against the original target.
    pub fn reconnect(&mut self) -> Result<(), KernelError> {
        self.send_exit();
        let (link, exit_policy, isolate_wire_context) = Self::open_target(&self.target)?;
        self.link = link;
        self.exit_policy = exit_policy;
        self.isolate_wire_context = isolate_wire_context;
        self.poisoned = false;
        Ok(())
    }

    /// Whether the underlying WSTP link is poisoned and needs [`Self::reconnect`].
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Borrow a [`WstpLink`] wrapper over the underlying link.
    pub fn wstp(&mut self) -> WstpLink<'_> {
        WstpLink::new(&mut self.link)
    }

    /// Evaluate a Wolfram Language [`Expr`] over WSTP.
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<EvalOutput, KernelError> {
        if self.poisoned {
            return Err(WstpError::Poisoned(
                "previous WSTP failure left the kernel link desynchronized; call reconnect() before retrying".into(),
            ));
        }
        match self.wstp().eval_expr(expr) {
            Ok(out) => Ok(out),
            Err(err) => {
                if err.is_fatal_to_link() {
                    self.poisoned = true;
                }
                Err(err)
            }
        }
    }

    /// Evaluate a Wolfram Language code string via ``System`ToExpression[code]``.
    pub fn eval_code(&mut self, wl_code: &str) -> Result<EvalOutput, KernelError> {
        if self.poisoned {
            return Err(WstpError::Poisoned(
                "previous WSTP failure left the kernel link desynchronized; call reconnect() before retrying".into(),
            ));
        }
        let result = if self.isolate_wire_context {
            let expr = isolated_eval_expr(wl_code);
            self.wstp().eval_expr(&expr)
        } else {
            self.wstp().eval_code(wl_code)
        };
        match result {
            Ok(out) => Ok(out),
            Err(err) => {
                // `WstpLink::eval_with` already attempted resync on recoverable
                // protocol/packet-limit/timeout errors and upgrades to `Poisoned`
                // only if resync fails. Mark the session poisoned only on fatal errors.
                if err.is_fatal_to_link() {
                    self.poisoned = true;
                }
                Err(err)
            }
        }
    }

    fn send_exit(&mut self) {
        if self.poisoned {
            // Never write `Exit[]` onto an already desynchronized/broken link;
            // simply mark closed and let `Link::drop` close the handle.
            let _ = self.exit_policy.take_exit();
            return;
        }
        if !self.exit_policy.take_exit() {
            return;
        }
        let _ = self.wstp().send_exit();
    }

    /// Ask an owned kernel to exit cleanly before closing the link.
    ///
    /// For sessions created via [`Self::connect`], this only closes the link
    /// without terminating the external kernel.
    pub fn close(mut self) {
        self.send_exit();
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        self.send_exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_session_exits_once() {
        let mut policy = ExitPolicy::owned();
        assert!(policy.take_exit(), "first close/drop should send Exit[]");
        assert!(!policy.take_exit(), "subsequent drop must not resend Exit[]");
    }

    #[test]
    fn attached_session_never_exits_external_kernel() {
        let mut policy = ExitPolicy::attached();
        assert!(
            !policy.take_exit(),
            "attached --link-name session must never kill external kernel"
        );
        assert!(!policy.take_exit());
    }
}
