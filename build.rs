/// Wolfram 15's WSTP static library exports only `ML*` symbols, while the
/// `wstp` 0.6 crate binds `WS*` names (the traditional aliases).  On MSVC,
/// tell the linker to treat every `WS*` reference as its `ML*` counterpart.
fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ENV");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }

    let ws_symbols = [
        "WSClearError",
        "WSError",
        "WSErrorMessage",
        "WSGetArgCount",
        "WSGetInteger64",
        "WSGetNext",
        "WSGetReal64",
        "WSGetType",
        "WSGetUTF8String",
        "WSGetUTF8Symbol",
        "WSNewPacket",
        "WSPutArgCount",
        "WSPutInteger64",
        "WSPutInteger8Array",
        "WSPutReal64",
        "WSPutType",
        "WSPutUTF8String",
        "WSPutUTF8Symbol",
        "WSReady",
        "WSReleaseErrorMessage",
        "WSReleaseUTF8String",
        "WSReleaseUTF8Symbol",
        "WSTestHead",
    ];

    for symbol in ws_symbols {
        let target = symbol.replacen("WS", "ML", 1);
        println!("cargo:rustc-link-arg=/alternatename:{}={}", symbol, target);
    }
}
