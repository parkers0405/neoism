// Emits GIT_HASH at build time so the About modal can show the build
// commit. Falls back gracefully (option_env! → None) when git is absent
// or this isn't a checkout, so source builds still compile.
use std::process::Command;

fn main() {
    // Windows defaults the main thread to a 1 MiB stack. Native graphics
    // adapter enumeration can exhaust it during startup (before a Rust panic
    // hook can report anything). Reserve 8 MiB, like a typical Unix main
    // thread; pages are committed on demand. RUST_MIN_STACK only affects
    // Rust-spawned threads, not this thread. Limit the flag to the desktop
    // executable and use the target environment, not the build host.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bin=neoism=/STACK:8388608");
        } else {
            println!("cargo:rustc-link-arg-bin=neoism=-Wl,--stack,8388608");
        }
    }

    prepare_debug_artifact_worker();

    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(hash) = hash {
        println!("cargo:rustc-env=GIT_HASH={hash}");
    }
    // Re-run when HEAD moves so the shown commit stays current.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

fn prepare_debug_artifact_worker() {
    for variable in ["NEOISM_CEF_RUNTIME", "NEOISM_CEF_DIR", "CEF_PATH"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    // Independent workspace/target avoids linking Chromium into the desktop or
    // deadlocking a recursive Cargo invocation on the desktop's target lock.
    if std::env::var_os("CARGO_FEATURE_CEF_ARTIFACTS").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
        || std::env::var("PROFILE").as_deref() != Ok("debug")
        || std::env::var("HOST").ok() != std::env::var("TARGET").ok()
        || std::env::var_os("NEOISM_CEF_RUNTIME").is_some()
    {
        return;
    }
    let manifest =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let worker = manifest.join("../cef-runtime");
    for path in ["Cargo.toml", "Cargo.lock", "build.rs", "src"] {
        println!("cargo:rerun-if-changed={}", worker.join(path).display());
    }
    for path in ["Cargo.toml", "src"] {
        println!(
            "cargo:rerun-if-changed={}",
            manifest
                .join("../interactive-artifacts")
                .join(path)
                .display()
        );
    }
    let cef = std::env::var_os("NEOISM_CEF_DIR")
        .or_else(|| std::env::var_os("CEF_PATH"))
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".local/share/cef"))
        });
    let Some(cef) = cef.filter(|path| path.is_dir()) else {
        println!("cargo:warning=CEF distribution missing: install prebuilt CEF and set NEOISM_CEF_DIR. See neoism-frontend/cef-runtime/README.md. The desktop will report unavailable live HTML instead of using Servo.");
        return;
    };
    let status =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["build", "--locked", "--jobs", "2", "--manifest-path"])
            .arg(worker.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(worker.join("target"))
            .env("CEF_PATH", &cef)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .status()
            .expect("cannot start the CEF worker debug build");
    assert!(status.success(), "CEF worker debug build failed; desktop build stopped rather than leaving a stale live HTML engine");
}
