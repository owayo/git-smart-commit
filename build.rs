fn main() {
    // fm-rs uses Swift bridging to access Apple's FoundationModels framework.
    // The resulting binary links against Swift runtime libraries (e.g. libswift_Concurrency.dylib),
    // which are not on the default library search path.
    // Without these rpaths, the binary crashes at launch with:
    //   dyld: Library not loaded: @rpath/libswift_Concurrency.dylib
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    {
        // System Swift runtime (always available on macOS)
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

        // Xcode toolchain Swift runtime (needed when using Xcode's bundled Swift)
        if let Ok(output) = std::process::Command::new("xcrun")
            .args(["--toolchain", "default", "--find", "swift"])
            .output()
        {
            let swift_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(toolchain) = std::path::Path::new(&swift_path)
                .parent()
                .and_then(|p| p.parent())
            {
                let lib_path = toolchain.join("lib/swift/macosx");
                if lib_path.exists() {
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_path.display());
                }
            }
        }
    }
}
