// Embed Info.plist in the __TEXT,__info_plist section of the Mach-O so macOS
// TCC identifies this binary by CFBundleIdentifier. Without an embedded
// Info.plist, TCC stores grants keyed on (path, responsible-process), which
// differs between Terminal-launched and launchd-launched runs — the same
// binary gets silently denied from one context while allowed from another.
// With CFBundleIdentifier present, TCC keys on bundle id and the grant
// works uniformly regardless of spawn context.

fn main() {
    if !cfg!(target_os = "macos") {
        return;
    }

    let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    println!(
        "cargo:rustc-link-arg-bin=alvum=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
