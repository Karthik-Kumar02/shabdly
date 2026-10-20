fn main() {
    // whisper.cpp's Metal code uses @available checks, which need clang's
    // runtime library when the app targets older macOS (as release builds do).
    // Rust links with -nodefaultlibs, so add it explicitly.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::process::Command::new("clang").arg("--print-runtime-dir").output().ok();
        if let Some(dir) = dir.filter(|o| o.status.success()) {
            println!("cargo:rustc-link-search=native={}", String::from_utf8_lossy(&dir.stdout).trim());
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
    }
    tauri_build::build()
}
