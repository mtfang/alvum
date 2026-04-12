// Prepares libonnxruntime.dylib for `ort`'s `load-dynamic` feature on macOS.
//
// ort-sys with `download-binaries` downloads a static archive; `load-dynamic`
// needs a shared library at runtime. This script wraps the static archive into
// a dylib and sets ORT_DYLIB_PATH so ort can find it.
fn main() {
    #[cfg(target_os = "macos")]
    build_ort_dylib();
}

#[cfg(target_os = "macos")]
fn build_ort_dylib() {
    use std::{path::PathBuf, process::Command};

    // Locate the static archive downloaded by ort-sys `download-binaries`.
    let home = std::env::var("HOME").unwrap_or_default();
    let cache_root = PathBuf::from(&home).join("Library/Caches/ort.pyke.io/dfbin");
    let target_triple = std::env::var("TARGET").unwrap_or_default();
    let target_cache = cache_root.join(&target_triple);

    // Find the static archive under any hash subdirectory.
    let static_lib = std::fs::read_dir(&target_cache)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path().join("onnxruntime/lib/libonnxruntime.a"))
                .find(|p| p.exists())
        });

    let Some(static_lib) = static_lib else {
        // Library not yet downloaded; ort-sys will fetch it when it next builds.
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let dylib_path = out_dir.join("libonnxruntime.dylib");

    if !dylib_path.exists() {
        let status = Command::new("clang")
            .args([
                "-dynamiclib",
                "-o",
                dylib_path.to_str().unwrap(),
                "-Wl,-all_load",
                static_lib.to_str().unwrap(),
                "-framework",
                "Foundation",
                "-lc++",
            ])
            .status()
            .expect("clang not found — required to wrap libonnxruntime.a into a dylib");
        assert!(status.success(), "failed to build libonnxruntime.dylib from static archive");
    }

    // Tell ort where to find the shared library at runtime.
    println!("cargo:rustc-env=ORT_DYLIB_PATH={}", dylib_path.display());
    println!("cargo:rerun-if-changed={}", static_lib.display());
}
