//! End-to-end install tests that hit the REAL network (GitHub releases, the
//! npm registry, the Node dist mirror) and exercise the exact pipeline the
//! extensions page drives. `#[ignore]` by default so `cargo test` stays
//! offline/fast; run explicitly with:
//!   cargo test -p neoism-extensions --test e2e_install -- --ignored --nocapture
//!
//! Each test installs into a scratch HOME so it never touches the developer's
//! real `~/.local/share/neoism` and is independent of what's on the system.

use std::collections::BTreeMap;
use std::path::PathBuf;

use neoism_extensions::{
    install, ExtensionManifest, GithubAsset, InstallKind, ProgressEvent, RunSpec,
};
use tokio::sync::mpsc::unbounded_channel;

/// Point the extensions dir at a throwaway location so a test install is
/// hermetic. `paths::extensions_dir()` resolves from `dirs::data_dir()`,
/// which honors `XDG_DATA_HOME` on Linux and `HOME` on macOS — set both.
fn scratch_data_home(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("neoism-e2e-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XDG_DATA_HOME", &dir);
    std::env::set_var("HOME", &dir);
    dir
}

async fn run_install(manifest: ExtensionManifest) -> Result<PathBuf, String> {
    let (tx, mut rx) = unbounded_channel::<ProgressEvent>();
    let pump = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            eprintln!("  progress: {ev:?}");
        }
    });
    let handle = install(manifest, tx);
    let joined = handle.join().await.expect("install task panicked");
    let _ = pump.await;
    match joined {
        Ok(entry) => {
            eprintln!("  OK: {:?}", entry.bin_path);
            Ok(entry.bin_path.unwrap_or_default())
        }
        Err(error) => {
            eprintln!("  ERR: {error}");
            Err(error.to_string())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "hits the network"]
async fn e2e_github_release_elixir_ls() {
    scratch_data_home("elixir-ls");
    // The exact manifest the extensions page logged for elixir-ls: a
    // GithubRelease whose only Unix asset carries the generic `unix` target.
    let mut execs = BTreeMap::new();
    execs.insert("elixir-ls".to_string(), "language_server.sh".to_string());
    let manifest = ExtensionManifest {
        id: "elixir-ls".to_string(),
        name: "elixir-ls".to_string(),
        version: "v0.31.1".to_string(),
        description: String::new(),
        author: String::new(),
        downloads: None,
        categories: vec![],
        languages: vec!["elixir".to_string()],
        repository_url: None,
        homepage: None,
        executables: vec!["language_server.sh".to_string()],
        install: InstallKind::GithubRelease {
            owner: "elixir-lsp".to_string(),
            repo: "elixir-ls".to_string(),
            tag: "v0.31.1".to_string(),
            assets: vec![GithubAsset {
                target: "unix".to_string(),
                file: "elixir-ls-v0.31.1.zip".to_string(),
                bin: "language_server.sh".to_string(),
                executables: execs,
            }],
        },
        run: Some(RunSpec {
            command: vec!["language_server.sh".to_string()],
            env: BTreeMap::new(),
        }),
        env_keys: vec![],
    };
    let bin = run_install(manifest).await.expect("elixir-ls install failed");
    assert!(bin.exists(), "installed bin should exist at {bin:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "hits the network; downloads a managed Node (~30MB) + the npm package"]
async fn e2e_npm_via_managed_node_bash_language_server() {
    scratch_data_home("bash-ls");
    // An npm LSP: this MUST provision the managed Node (no system Node needed)
    // and install the package with it — the Zed-style zero-setup path.
    let manifest = ExtensionManifest {
        id: "bash-language-server".to_string(),
        name: "bash-language-server".to_string(),
        version: "latest".to_string(),
        description: String::new(),
        author: String::new(),
        downloads: None,
        categories: vec![],
        languages: vec!["bash".to_string()],
        repository_url: None,
        homepage: None,
        executables: vec!["bash-language-server".to_string()],
        install: InstallKind::Npm {
            package: "bash-language-server".to_string(),
            version: "latest".to_string(),
            extra_packages: vec![],
        },
        run: Some(RunSpec {
            command: vec!["bash-language-server".to_string()],
            env: BTreeMap::new(),
        }),
        env_keys: vec![],
    };
    let bin = run_install(manifest)
        .await
        .expect("bash-language-server (npm/managed-node) install failed");
    assert!(bin.exists(), "installed npm bin should exist at {bin:?}");
}
