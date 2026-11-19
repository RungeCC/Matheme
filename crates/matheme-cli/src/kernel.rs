//! Persistent WSTP client session communicating with an external Wolfram Kernel.
//!
//! This is the inverse of the LibraryLink `cdylib`: instead of Wolfram loading
//! Rust in-process, `matheme-cli` actively launches (or connects to) a
//! `WolframKernel` over WSTP, sends `EvaluatePacket[...]` expressions, and
//! decodes `ReturnPacket[...]` responses into [`matheme_wl::Expr`] / Typst
//! [`typst::foundations::Value`].

use std::path::Path;

use matheme_wstp::{Link, WstpLink};
use typst::foundations::Value;

/// Result of one Wolfram Kernel evaluation.
pub type EvalOutput = matheme_wstp::EvalOutput;

/// Error returned by [`KernelSession`] operations.
pub type KernelError = matheme_wstp::WstpError;

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
    exit_policy: ExitPolicy,
}

impl KernelSession {
    /// Launch a local `WolframKernel` executable and connect over WSTP.
    pub fn launch(kernel_path: &Path) -> Result<Self, KernelError> {
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
        let mut link = Link::open_with_args(&argv)
            .map_err(|err| KernelError::new(format!("failed to launch Wolfram kernel ({path_str}): {err}")))?;
        link.activate()
            .map_err(|err| KernelError::new(format!("failed to activate WSTP link: {err}")))?;

        let mut session = Self {
            link,
            exit_policy: ExitPolicy::owned(),
        };
        session.wstp().drain_until_input_name(32)?;
        Ok(session)
    }

    /// Connect to an already-running WSTP link by name.
    pub fn connect(link_name: &str) -> Result<Self, KernelError> {
        let argv = [
            "matheme",
            "-linkmode",
            "connect",
            "-linkname",
            link_name,
        ];
        let mut link = Link::open_with_args(&argv)
            .map_err(|err| KernelError::new(format!("failed to connect to WSTP link {link_name:?}: {err}")))?;
        link.activate()
            .map_err(|err| KernelError::new(format!("failed to activate WSTP link: {err}")))?;

        let mut session = Self {
            link,
            exit_policy: ExitPolicy::attached(),
        };
        session.wstp().drain_until_input_name(32)?;
        Ok(session)
    }

    fn wstp(&mut self) -> WstpLink<'_> {
        WstpLink::new(&mut self.link)
    }

    /// Evaluate a Wolfram Language code string via `ToExpression[code]`.
    pub fn eval_code(&mut self, wl_code: &str) -> Result<EvalOutput, KernelError> {
        self.wstp().eval_code(wl_code)
    }

    /// Evaluate Wolfram Language code and convert the result into a native
    /// Typst [`Value`] via [`matheme_wl::to_typst`].
    pub fn eval_to_typst(
        &mut self,
        wl_code: &str,
    ) -> Result<(Value, Vec<String>, EvalOutput), KernelError> {
        let output = self.eval_code(wl_code)?;
        let mut warnings = Vec::new();
        let value = output.expr.to_typst(&mut warnings);
        Ok((value, warnings, output))
    }

    fn send_exit(&mut self) {
        if !self.exit_policy.take_exit() {
            return;
        }
        self.wstp().send_exit();
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
