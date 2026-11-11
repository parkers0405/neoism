//! The app answers `--version` with the release it shipped in: the root workspace's version,
//! which the release tag, the updater and the installers all compare against.
use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
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
}
