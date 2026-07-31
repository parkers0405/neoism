// Emits GIT_HASH at build time so the About modal can show the build
// commit. Falls back gracefully (option_env! → None) when git is absent
// or this isn't a checkout, so source builds still compile.
use std::process::Command;

fn main() {
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
