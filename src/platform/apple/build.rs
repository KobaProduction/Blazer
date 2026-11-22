fn main() {
    #[cfg(feature = "packet-tunnel")]
    if let Ok(os @ ("ios" | "macos")) = std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        cc::Build::new()
            .files([
                "../apple/tunnel/manager.mm",
                "../apple/tunnel/control_socket.mm",
                "../apple/tunnel/activation.mm",
            ])
            .cpp(true)
            .std("c++17")
            .flag("-fobjc-arc")
            .cargo_metadata(false)
            .compile("blazer-apple-tunnel-manager");
        let output = std::env::var("OUT_DIR").expect("Cargo sets OUT_DIR for the build script");
        println!("cargo:rustc-link-search=native={output}");
        println!("cargo:rustc-link-lib=static:+whole-archive=blazer-apple-tunnel-manager");
        println!("cargo:rustc-link-lib=c++");
        println!("cargo:rustc-link-lib=framework=NetworkExtension");
        println!("cargo:rustc-link-lib=framework=Foundation");
        if os == "macos" {
            println!("cargo:rustc-link-lib=framework=SystemExtensions");
        }
        println!("cargo:rerun-if-changed=../apple/tunnel");
    }
}
