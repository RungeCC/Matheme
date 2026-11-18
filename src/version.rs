//! `MathemeVersion` endpoint reporting Typst, Matheme DLL, LibraryLink and WSTP versions.

use matheme_wl::{assoc, rule, Entry};

use crate::call::{self, Answer, Endpoint, Failure, Outcome};
use crate::wolfram;

pub struct Version;

impl Endpoint for Version {
    const NAME: &'static str = "version";
    const HOST_FONTS: bool = false;
    type Input = ();

    fn parse(args: &[Entry<'_>]) -> Result<(), Failure> {
        call::ensure_empty(args, Self::NAME)
    }

    fn invoke((): ()) -> Answer {
        // Typst derives its version from its build-time TYPST_VERSION env var
        // (see typst_utils::version).
        let typst_version = typst::utils::version();
        let dll = wolfram::dll_info();
        let wstp_expr = match dll.wstp_runtime {
            Some((interface, revision, build)) => assoc! {
                "Protocol" -> dll.wstp_protocol,
                "Interface" -> interface,
                "Revision" -> revision,
                "Build" -> build,
            },
            None => assoc! {
                "Protocol" -> dll.wstp_protocol,
                "Interface" -> dll.wstp_compiled_interface,
                "Revision" -> dll.wstp_compiled_revision,
            },
        };

        Outcome::new([
            rule!("Matheme" -> env!("CARGO_PKG_VERSION")),
            rule!("Typst" -> typst_version.raw()),
            rule!("TypstCommit" -> typst_version.commit()),
            rule!("Target" -> dll.target),
            rule!("LibraryLink" -> assoc! {
                "CompiledVersion" -> dll.library_link_compiled,
                "RuntimeVersion" -> dll.library_link_runtime,
            }),
            rule!("WSTP" -> wstp_expr),
        ])
        .into()
    }
}
