fn main() {
    #[cfg(target_os = "macos")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/macos_fast_video.m");
        cc::Build::new()
            .file("src/macos_fast_video.m")
            .flag("-fobjc-arc")
            .flag("-Wno-deprecated-declarations")
            .compile("truongpaper_macos_fast_video");
        for framework in ["Foundation", "AVFoundation", "CoreMedia", "CoreGraphics"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
