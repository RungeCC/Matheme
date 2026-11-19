//! Persistent WSTP client session communicating with an external Wolfram Kernel.
//!
//! This is the inverse of the LibraryLink `cdylib`: instead of Wolfram loading
//! Rust in-process, `matheme-cli` actively launches (or connects to) a
//! `WolframKernel` over WSTP, sends `EvaluatePacket[...]` expressions, and
//! decodes `ReturnPacket[...]` responses into [`matheme_wl::Expr`] / Typst
//! [`typst::foundations::Value`].

use std::fmt;
use std::path::Path;

use matheme_wl::{Expr, Symbol};
use typst::foundations::Value;
use wolfram_expr::{Expr as RawExpr, ExprKind, Symbol as RawSymbol};
use wstp::Link;

/// Result of one Wolfram Kernel evaluation.
#[derive(Debug, Clone)]
pub struct EvalOutput {
    pub expr: Expr,
    pub messages: Vec<String>,
    pub prints: Vec<String>,
}

#[derive(Debug)]
pub struct KernelError(pub String);

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for KernelError {}

/// A live WSTP session attached to a `WolframKernel` process.
pub struct KernelSession {
    link: Link,
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
            .map_err(|err| KernelError(format!("failed to launch Wolfram kernel ({path_str}): {err}")))?;
        link.activate()
            .map_err(|err| KernelError(format!("failed to activate WSTP link: {err}")))?;

        let mut session = Self { link };
        session.drain_startup()?;
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
            .map_err(|err| KernelError(format!("failed to connect to WSTP link {link_name:?}: {err}")))?;
        link.activate()
            .map_err(|err| KernelError(format!("failed to activate WSTP link: {err}")))?;

        let mut session = Self { link };
        session.drain_startup()?;
        Ok(session)
    }

    /// Read the kernel's initial `InputNamePacket["In[1]:= "]` banner.
    fn drain_startup(&mut self) -> Result<(), KernelError> {
        let input_name_head = RawSymbol::new("System`InputNamePacket");
        for _ in 0..32 {
            let packet = self.read_packet()?;
            if let ExprKind::Normal(normal) = packet.kind() {
                if normal.has_head(&input_name_head) {
                    return Ok(());
                }
            }
        }
        Err(KernelError(
            "did not receive initial InputNamePacket from Wolfram kernel".into(),
        ))
    }

    fn read_packet(&mut self) -> Result<RawExpr, KernelError> {
        self.link
            .get_expr_with_resolver(&mut |name: &str| {
                Some(RawSymbol::new(&format!("System`{name}")))
            })
            .map_err(|err| KernelError(format!("WSTP read error: {err}")))
    }

    /// Evaluate a raw Wolfram Language expression via `EvaluatePacket[expr]`.
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<EvalOutput, KernelError> {
        let packet = RawExpr::normal(
            RawExpr::symbol(RawSymbol::new("System`EvaluatePacket")),
            vec![expr.raw().clone()],
        );
        self.link
            .put_expr(&packet)
            .map_err(|err| KernelError(format!("WSTP write error: {err}")))?;
        self.link
            .flush()
            .map_err(|err| KernelError(format!("WSTP flush error: {err}")))?;

        let return_head = RawSymbol::new(Symbol::RETURN_PACKET.as_str());
        let text_head = RawSymbol::new("System`TextPacket");
        let message_head = RawSymbol::new("System`MessagePacket");
        let input_name_head = RawSymbol::new("System`InputNamePacket");
        let output_name_head = RawSymbol::new("System`OutputNamePacket");

        let mut messages = Vec::new();
        let mut prints = Vec::new();
        let mut pending_message = false;

        for _ in 0..256 {
            let pkt = self.read_packet()?;
            match pkt.kind() {
                ExprKind::Normal(normal)
                    if normal.has_head(&return_head) && normal.elements().len() == 1 =>
                {
                    return Ok(EvalOutput {
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
                    if normal.has_head(&input_name_head) || normal.has_head(&output_name_head) =>
                {
                    continue;
                }
                other => {
                    return Err(KernelError(format!(
                        "unexpected WSTP packet while waiting for ReturnPacket: {other}"
                    )));
                }
            }
        }

        Err(KernelError(
            "exceeded WSTP packet limit before ReturnPacket".into(),
        ))
    }

    /// Evaluate a Wolfram Language code string via `ToExpression[code]`.
    pub fn eval_code(&mut self, wl_code: &str) -> Result<EvalOutput, KernelError> {
        let to_expr = RawExpr::normal(
            RawExpr::symbol(RawSymbol::new("System`ToExpression")),
            vec![RawExpr::string(wl_code)],
        );
        self.eval_expr(&Expr::from_raw(to_expr))
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

    /// Ask the kernel to exit cleanly before closing the link.
    pub fn close(mut self) {
        let exit = RawExpr::normal(
            RawExpr::symbol(RawSymbol::new("System`EvaluatePacket")),
            vec![RawExpr::normal(
                RawExpr::symbol(RawSymbol::new("System`Exit")),
                vec![],
            )],
        );
        let _ = self.link.put_expr(&exit);
        let _ = self.link.flush();
    }
}

impl Drop for KernelSession {
    fn drop(&mut self) {
        let exit = RawExpr::normal(
            RawExpr::symbol(RawSymbol::new("System`EvaluatePacket")),
            vec![RawExpr::normal(
                RawExpr::symbol(RawSymbol::new("System`Exit")),
                vec![],
            )],
        );
        let _ = self.link.put_expr(&exit);
        let _ = self.link.flush();
    }
}
