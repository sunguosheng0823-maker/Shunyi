fn main() {
    // Homebrew's static libaom enables VMAF. Keep the preview self-contained.
    println!("cargo:rerun-if-env-changed=SHUNYI_VMAF_LIB_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .cpp(true)
            .file("src/macos.mm")
            .flag("-fobjc-arc")
            .compile("shunyi_macos_capture");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rerun-if-changed=src/macos.mm");
        if let Ok(directory) = std::env::var("SHUNYI_VMAF_LIB_DIR") {
            println!("cargo:rustc-link-search=native={directory}");
            println!("cargo:rustc-link-lib=static=vmaf");
        }
    }
}
