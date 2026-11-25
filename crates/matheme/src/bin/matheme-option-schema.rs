use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::write(
        workspace.join("Kernel/ArgumentTypes.wl"),
        matheme::wolfram_argument_types(),
    )
}
