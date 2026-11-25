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
    // Native Windows release builds can put the installer artwork in the EXE itself too.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = "../../misc/windows/neoism.ico";
        println!("cargo:rerun-if-changed={icon}");
        winres::WindowsResource::new().set_icon(icon).compile()
            .expect("Could not embed the Neoism executable icon");
    }
    build_glass_cursor();
    build_mobile_qr();
}

fn build_mobile_qr() {
    use std::{env, process::Command};

    println!("cargo:rerun-if-changed=native/MobileQR.m");
    println!("cargo:rerun-if-changed=native/MobileDictation.m");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target = env::var("TARGET").unwrap();
    let (sdk, frameworks): (&str, &[&str]) = match os.as_str() {
        "macos" => ("macosx", &["Foundation", "Vision"]),
        "ios" if target.ends_with("-sim") || target.starts_with("x86_64-") => {
            ("iphonesimulator", &["Foundation", "UIKit", "AVFoundation", "Speech"])
        }
        "ios" => ("iphoneos", &["Foundation", "UIKit", "AVFoundation", "Speech"]),
        _ => return,
    };
    let deployment_var = if os == "ios" { "IPHONEOS_DEPLOYMENT_TARGET" } else { "MACOSX_DEPLOYMENT_TARGET" };
    println!("cargo:rerun-if-env-changed={deployment_var}");
    println!("cargo:rerun-if-env-changed=SDKROOT");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    let deployment = Command::new(env::var_os("RUSTC").unwrap())
        .args(["--print", "deployment-target", "--target", &target])
        .output().expect("could not query Rust deployment target");
    assert!(deployment.status.success(), "could not query Rust deployment target");
    let deployment = String::from_utf8(deployment.stdout).unwrap();
    let version = deployment.trim().strip_prefix(&format!("{deployment_var}=")).unwrap();
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("unsupported Apple architecture: {other}"),
    };
    let platform = if os == "macos" { "macosx" } else { "ios" };
    let simulator = if sdk == "iphonesimulator" { "-simulator" } else { "" };
    let clang_target = format!("{arch}-apple-{platform}{version}{simulator}");
    let sdk_path = Command::new("xcrun").args(["--sdk", sdk, "--show-sdk-path"])
        .output().expect("could not locate Apple SDK for QR scanner");
    assert!(sdk_path.status.success(), "could not locate Apple SDK {sdk}");
    let sdk_path = String::from_utf8(sdk_path.stdout).unwrap();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("MobileQR.o");
    let status = Command::new("xcrun")
        .args(["--sdk", sdk, "clang", "-target", &clang_target, "-isysroot", sdk_path.trim(),
            "-fobjc-arc", "-fblocks", "-O2", "-c", "native/MobileQR.m", "-o"])
        .arg(&object).status().expect("QR scanner requires Apple clang");
    assert!(status.success(), "native QR scanner compilation failed");
    let mut archive = Command::new("xcrun");
    archive.args(["--sdk", sdk, "ar", "crs"]).arg(out.join("libNeoismMobileQR.a")).arg(object);
    if os == "ios" {
        let object = out.join("MobileDictation.o");
        let status = Command::new("xcrun").args(["--sdk", sdk, "clang", "-target", &clang_target, "-isysroot", sdk_path.trim(),
            "-fobjc-arc", "-fblocks", "-O2", "-c", "native/MobileDictation.m", "-o"])
            .arg(&object).status().expect("dictation requires Apple clang");
        assert!(status.success(), "native dictation compilation failed");
        archive.arg(object);
    }
    let status = archive.status().expect("could not archive native mobile code");
    assert!(status.success(), "native QR scanner archive failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=NeoismMobileQR");
    println!("cargo:rustc-link-lib=dylib=objc");
    for framework in frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
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
