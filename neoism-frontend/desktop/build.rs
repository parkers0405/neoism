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
