fn main() {
    // fm-rs は Swift ブリッジ経由で Apple の FoundationModels フレームワークにアクセスする。
    // 生成されるバイナリは Swift ランタイムライブラリ
    // （例: libswift_Concurrency.dylib）へリンクするが、
    // これらは既定のライブラリ探索パスに含まれない。
    // rpath を追加しないと起動時に次のエラーでクラッシュする:
    //   dyld: Library not loaded: @rpath/libswift_Concurrency.dylib
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    {
        // macOS に常に存在するシステム Swift ランタイム
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

        // Xcode 同梱の Swift を使う環境向けにツールチェーン側のランタイムも追加する
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
