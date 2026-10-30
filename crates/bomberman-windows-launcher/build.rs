fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Let the linker generate the manifest. This uses the same native
        // common controls on MSVC and lld without a separate resource tool.
        println!("cargo:rustc-link-arg-bin=atomic-bomberman-launcher=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=atomic-bomberman-launcher=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
        println!("cargo:rustc-link-arg-bin=atomic-bomberman-launcher=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='amd64' publicKeyToken='6595b64144ccf1df' language='*'");
    }
}
