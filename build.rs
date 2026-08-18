fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");

    // Tell cargo about our custom cfg flags
    println!("cargo::rustc-check-cfg=cfg(debug_build)");
    println!("cargo::rustc-check-cfg=cfg(release_build)");

    // Detect if this is a release build
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let is_release = profile == "release";

    // Set cfg flag for debug logging (enabled in dev, disabled in release)
    if is_release {
        println!("cargo:rustc-cfg=release_build");
    } else {
        println!("cargo:rustc-cfg=debug_build");
        // Include defmt.x linker script only for debug builds
        println!("cargo:rustc-link-arg=-Tdefmt.x");
    }
}
