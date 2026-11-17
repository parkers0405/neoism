//! The app answers `--version` with the release it shipped in: the root workspace's version,
//! which the release tag, the updater and the installers all compare against.
use std::path::PathBuf;

fn main() {
    // A shared target can reuse this executable across worktrees; resolve the current root at runtime.
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-env-changed=NEOISM_VERSION");
    let from_workspace = std::fs::read_to_string(&manifest).ok().and_then(|text| {
        let mut in_package = false;
        text.lines().find_map(|line| {
            let line = line.trim();
            if line.starts_with('[') { in_package = line == "[workspace.package]"; return None; }
            let value = line.strip_prefix("version")?.trim_start().strip_prefix('=')?.trim();
            in_package.then(|| value.trim_matches('"').to_owned())
        })
    });
    let version = std::env::var("NEOISM_VERSION").ok().or(from_workspace).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=NEOISM_VERSION={version}");
    build_glass_cursor();
}

fn build_glass_cursor() {
    println!("cargo:rerun-if-changed=native/GlassCursor.swift");
    println!("cargo:rerun-if-changed=native/GlassOptics.swift");
    println!("cargo:rerun-if-changed=native/HumanScroll.swift");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") { return; }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64", "x86_64" => "x86_64", other => panic!("unsupported macOS architecture: {other}"),
    };
    let status = std::process::Command::new("xcrun").args(["swiftc", "-parse-as-library", "-O", "-emit-library", "-static",
        "-target", &format!("{arch}-apple-macosx13.0"), "-module-name", "NeoismGlassCursor",
        "native/GlassCursor.swift", "native/GlassOptics.swift", "native/HumanScroll.swift", "-o"]).arg(out.join("libNeoismGlassCursor.a")).status().expect("Optical cursor requires Xcode and swiftc");
    assert!(status.success(), "Swift/Metal optical cursor compilation failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-lib=static=NeoismGlassCursor");
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=QuartzCore");
    println!("cargo:rustc-link-lib=framework=Metal");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
