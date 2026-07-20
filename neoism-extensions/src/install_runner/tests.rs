use super::*;

fn asset(target: &str, file: &str, bin: &str) -> GithubAsset {
    GithubAsset {
        target: target.to_string(),
        file: file.to_string(),
        bin: bin.to_string(),
        executables: std::collections::BTreeMap::new(),
    }
}

#[test]
fn current_target_is_known() {
    let t = current_target();
    assert!(
        matches!(
            t,
            "linux_x64_gnu"
                | "linux_arm64_gnu"
                | "darwin_x64"
                | "darwin_arm64"
                | "win_x64"
                | "win_arm64"
                | "unknown"
        ),
        "unexpected target string: {t}"
    );
}

#[test]
fn github_url_format() {
    let url = github_asset_url("rust-lang", "rust-analyzer", "2024-01-01", "ra.gz");
    assert_eq!(
        url,
        "https://github.com/rust-lang/rust-analyzer/releases/download/2024-01-01/ra.gz"
    );
}

#[test]
fn pick_asset_matches_current_target() {
    let assets = vec![
        asset("linux_x64_gnu", "linux.gz", "tool"),
        asset("linux_arm64_gnu", "linux-arm.gz", "tool"),
        asset("darwin_x64", "mac.gz", "tool"),
        asset("darwin_arm64", "mac-arm.gz", "tool"),
        asset("win_x64", "win.zip", "tool.exe"),
    ];
    let picked = pick_asset(&assets, current_target());
    if matches!(
        current_target(),
        "linux_x64_gnu" | "linux_arm64_gnu" | "darwin_x64" | "darwin_arm64" | "win_x64"
    ) {
        assert!(picked.is_some());
    }
}

#[test]
fn unsupported_github_manifest_is_hidden_on_current_host() {
    let manifest = ExtensionManifest {
        id: "other-platform".into(),
        name: "Other platform".into(),
        version: "1".into(),
        description: String::new(),
        author: String::new(),
        downloads: None,
        categories: Vec::new(),
        languages: Vec::new(),
        repository_url: None,
        homepage: None,
        executables: Vec::new(),
        install: InstallKind::GithubRelease {
            owner: "owner".into(),
            repo: "repo".into(),
            tag: "v1".into(),
            assets: vec![asset("definitely_not_this_host", "tool.gz", "tool")],
        },
        run: None,
        env_keys: Vec::new(),
    };
    assert!(!supported_on_current_host(&manifest));
}

#[test]
fn cargo_manifest_is_installable_on_current_host() {
    let manifest = ExtensionManifest {
        id: "cargo-tool".into(),
        name: "Cargo tool".into(),
        version: "1".into(),
        description: String::new(),
        author: String::new(),
        downloads: None,
        categories: Vec::new(),
        languages: Vec::new(),
        repository_url: None,
        homepage: None,
        executables: Vec::new(),
        install: InstallKind::Cargo {
            crate_name: "cargo-tool".into(),
            version: "1".into(),
            features: Vec::new(),
        },
        run: None,
        env_keys: Vec::new(),
    };
    assert!(supported_on_current_host(&manifest));
}

#[test]
fn pick_asset_returns_none_for_unknown() {
    let assets = vec![asset("linux_x64_gnu", "f", "b")];
    assert!(pick_asset(&assets, "fictional_target").is_none());
}

#[test]
fn pick_asset_accepts_platform_independent_release() {
    let assets = vec![asset("", "server.jar", "server")];
    assert_eq!(
        pick_asset(&assets, current_target()).map(|asset| asset.file.as_str()),
        Some("server.jar")
    );
}

#[test]
fn asset_matching_uses_ranked_platform_compatibility() {
    let assets = vec![
        asset("linux_x64_musl", "musl.tar.gz", "tool"),
        asset("", "portable.tar.gz", "tool"),
        asset("unix", "unix.tar.gz", "tool"),
        asset("linux", "linux.tar.gz", "tool"),
        asset("linux_x64", "linux-x64.tar.gz", "tool"),
        asset("linux_x64_gnu", "gnu.tar.gz", "tool"),
    ];
    assert_eq!(
        pick_asset(&assets, "linux_x64_gnu").map(|asset| asset.file.as_str()),
        Some("gnu.tar.gz")
    );

    let without_exact = &assets[..5];
    assert_eq!(
        pick_asset(without_exact, "linux_x64_gnu").map(|asset| asset.file.as_str()),
        Some("linux-x64.tar.gz")
    );
    assert_ne!(
        pick_asset(without_exact, "linux_x64_gnu").map(|asset| asset.file.as_str()),
        Some("musl.tar.gz"),
        "an ABI-specific musl build must never satisfy a GNU host"
    );
}

#[test]
fn asset_matching_falls_back_through_family_unix_and_portable() {
    let family = vec![
        asset("", "portable.zip", "tool"),
        asset("unix", "unix.zip", "tool"),
        asset("darwin", "mac.zip", "tool"),
    ];
    assert_eq!(
        pick_asset(&family, "darwin_arm64").map(|asset| asset.file.as_str()),
        Some("mac.zip")
    );
    assert_eq!(
        pick_asset(&family[..2], "darwin_arm64").map(|asset| asset.file.as_str()),
        Some("unix.zip")
    );
    assert_eq!(
        pick_asset(&family[..1], "darwin_arm64").map(|asset| asset.file.as_str()),
        Some("portable.zip")
    );
    assert_eq!(
        pick_asset(&family, "win_arm64").map(|asset| asset.file.as_str()),
        Some("portable.zip"),
        "Windows must not select a Unix-family asset"
    );
}

#[test]
fn npm_bin_path_resolves_under_node_modules_dot_bin() {
    let install = Path::new("/tmp/fake-install");
    let bin_dir = install.join("node_modules").join(".bin");
    let resolved = npm_bin_path(&bin_dir, "server-filesystem");
    #[cfg(not(windows))]
    assert_eq!(
        resolved,
        PathBuf::from("/tmp/fake-install/node_modules/.bin/server-filesystem")
    );
    #[cfg(windows)]
    assert_eq!(
        resolved,
        PathBuf::from("/tmp/fake-install/node_modules/.bin/server-filesystem.cmd")
    );
}

#[test]
fn venv_bin_path_resolves_under_venv_bin() {
    let venv = Path::new("/tmp/fake/venv");
    let resolved = venv_bin_path(venv, "black");
    #[cfg(not(target_os = "windows"))]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/venv/bin/black"));
    #[cfg(target_os = "windows")]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/venv/Scripts/black.exe"));
}

#[test]
fn venv_pip_path_resolves_under_venv_bin() {
    let venv = Path::new("/tmp/fake/venv");
    let resolved = venv_pip_path(venv);
    #[cfg(not(target_os = "windows"))]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/venv/bin/pip"));
    #[cfg(target_os = "windows")]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/venv/Scripts/pip.exe"));
}

#[test]
fn cargo_bin_path_resolves_under_private_install_root() {
    let install_dir = Path::new("/tmp/fake/cargo-tool");
    let resolved = cargo_bin_path(install_dir, "nil");
    #[cfg(not(target_os = "windows"))]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/cargo-tool/bin/nil"));
    #[cfg(target_os = "windows")]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/cargo-tool/bin/nil.exe"));
}

#[test]
fn go_bin_path_resolves_under_private_gobin() {
    let install_dir = Path::new("/tmp/fake/gopls");
    let resolved = go_bin_path(install_dir, "gopls");
    #[cfg(not(target_os = "windows"))]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/gopls/bin/gopls"));
    #[cfg(target_os = "windows")]
    assert_eq!(resolved, PathBuf::from("/tmp/fake/gopls/bin/gopls.exe"));
}

#[test]
fn npm_companion_specs_share_the_primary_install_plan() {
    let specs = npm_package_specs(
        "typescript-language-server",
        "5.3.0",
        &[
            "typescript@6.0.3".to_string(),
            "@vue/typescript-plugin".to_string(),
        ],
    )
    .unwrap();
    assert_eq!(
        specs,
        vec![
            "typescript-language-server@5.3.0",
            "typescript@6.0.3",
            "@vue/typescript-plugin"
        ]
    );
}

#[tokio::test]
async fn npm_multi_binary_manifest_resolves_every_private_shim() {
    let install_dir = reconciliation_dir("npm-multi-bin");
    let bin_dir = install_dir.join("node_modules").join(".bin");
    let cli = npm_bin_path(&bin_dir, "pyright");
    let server = npm_bin_path(&bin_dir, "pyright-langserver");
    write_test_executable(&cli);
    write_test_executable(&server);
    let manifest = npm_reconciliation_manifest("pyright", "pyright-langserver");
    let mut manifest = manifest;
    manifest.executables = vec!["pyright".into(), "pyright-langserver".into()];
    let names = declared_executable_names(&manifest).unwrap();
    let resolved = resolve_installed_binaries(&manifest, &install_dir, &server, &names)
        .await
        .unwrap();
    assert_eq!(
        resolved,
        vec![
            ("pyright-langserver".to_string(), server),
            ("pyright".to_string(), cli),
        ]
    );
    let _ = std::fs::remove_dir_all(install_dir);
}

#[test]
fn package_manager_arguments_reject_option_and_whitespace_injection() {
    assert!(npm_package_specs("--ignore-scripts", "1", &[]).is_err());
    assert!(npm_package_specs("safe", "1", &["bad package".to_string()]).is_err());
    assert!(split_gem_package_spec("--install-dir").is_err());
    assert!(split_gem_package_spec("ruby-lsp@0.26.10").is_ok());
}

#[tokio::test]
async fn gem_shim_restores_private_gem_environment() {
    let dir = reconciliation_dir("gem-shim");
    let gem_home = dir.join("gem-home");
    let wrapper = dir.join("gem-wrappers").join("solargraph");
    write_test_executable(&wrapper);
    let shim = gem_shim_path(&dir, "solargraph");
    write_gem_shim(&shim, &gem_home, &wrapper).await.unwrap();
    let contents = std::fs::read_to_string(&shim).unwrap();
    assert!(contents.contains("GEM_HOME="));
    assert!(contents.contains("GEM_PATH="));
    assert!(contents.contains("solargraph"));
    assert!(valid_executable_file(&shim));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn release_interpreter_recipe_becomes_a_fixed_argv_launcher() {
    let dir = reconciliation_dir("dotnet-launcher");
    let payload = dir.join("libexec").join("Server.dll");
    std::fs::create_dir_all(payload.parent().unwrap()).unwrap();
    std::fs::write(&payload, b"managed assembly").unwrap();

    let launcher =
        materialize_github_executable(&dir, "server", "dotnet:libexec/Server.dll")
            .await
            .unwrap();
    assert!(valid_executable_file(&launcher));
    let contents = std::fs::read_to_string(&launcher).unwrap();
    assert!(contents.contains("dotnet"));
    assert!(contents.contains("Server.dll"));
    assert!(!contents.contains("sh -c"));
    assert!(
        materialize_github_executable(&dir, "escape", "dotnet:../outside.dll")
            .await
            .is_err()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn github_manifest_requires_a_resolvable_primary_recipe() {
    let mut manifest = reconciliation_manifest("omnisharp", "OmniSharp");
    manifest.executables = vec!["OmniSharp".to_string()];
    assert!(!supported_on_current_host(&manifest));
    let InstallKind::GithubRelease { assets, .. } = &mut manifest.install else {
        unreachable!()
    };
    assets[0].executables.insert(
        "OmniSharp".to_string(),
        "dotnet:libexec/OmniSharp.dll".to_string(),
    );
    assert!(supported_on_current_host(&manifest));
}

#[test]
fn parse_percent_extracts_first_percent_number() {
    assert_eq!(parse_percent("Downloading wheel (45%) ..."), Some(45));
    assert_eq!(parse_percent("info attempt #2 [done in 50ms]"), None);
    assert_eq!(parse_percent("100% complete"), Some(100));
    assert_eq!(parse_percent("nothing"), None);
}

#[test]
fn install_error_display_variants() {
    let e = InstallError::Network("boom".to_string());
    assert_eq!(format!("{e}"), "network error: boom");

    let e = InstallError::MissingTool("npm");
    assert_eq!(format!("{e}"), "tool `npm` not found on PATH");

    let e = InstallError::NoAssetForTarget("linux_x64_gnu".to_string());
    assert_eq!(format!("{e}"), "no asset matches target `linux_x64_gnu`");

    let e = InstallError::ParseManifest("oops".to_string());
    assert_eq!(format!("{e}"), "config parse error: oops");

    let e = InstallError::NotImplemented;
    assert_eq!(format!("{e}"), "not yet implemented");

    let e = InstallError::Zip("bad zip".to_string());
    assert_eq!(format!("{e}"), "zip error: bad zip");
}

#[test]
fn install_kind_tag_matches_discriminant() {
    let npm = InstallKind::Npm {
        package: "x".to_string(),
        version: "1".to_string(),
        extra_packages: Vec::new(),
    };
    assert_eq!(install_kind_tag(&npm), "npm");

    let pip = InstallKind::Pip {
        package: "x".to_string(),
        version: "1".to_string(),
    };
    assert_eq!(install_kind_tag(&pip), "pip");

    let gh = InstallKind::GithubRelease {
        owner: "o".to_string(),
        repo: "r".to_string(),
        tag: "t".to_string(),
        assets: vec![],
    };
    assert_eq!(install_kind_tag(&gh), "github_release");

    let go = InstallKind::Go {
        package: "golang.org/x/tools/gopls".to_string(),
        version: "v0.23.0".to_string(),
    };
    assert_eq!(install_kind_tag(&go), "go");

    let gem = InstallKind::Gem {
        package: "solargraph".to_string(),
        version: "0.60.2".to_string(),
        extra_packages: Vec::new(),
    };
    assert_eq!(install_kind_tag(&gem), "gem");
}

#[test]
fn download_percent_requires_a_real_total() {
    assert_eq!(download_percent(5, Some(10)), Some(50));
    assert_eq!(download_percent(20, Some(10)), Some(100));
    assert_eq!(download_percent(5, None), None);
    assert_eq!(download_percent(5, Some(0)), None);
}

#[test]
fn release_asset_name_rejects_paths() {
    assert_eq!(
        safe_asset_name("rust-analyzer.gz").unwrap(),
        "rust-analyzer.gz"
    );
    assert!(safe_asset_name("../rust-analyzer.gz").is_err());
    assert!(safe_asset_name("dir/rust-analyzer.gz").is_err());
    assert!(safe_asset_name("dir\\rust-analyzer.gz").is_err());
}

#[test]
fn partial_download_guard_cleans_up_unless_committed() {
    let dir = std::env::temp_dir().join(format!(
        "neoism-partial-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let abandoned = dir.join("abandoned.part");
    std::fs::write(&abandoned, b"partial").unwrap();
    drop(RemovePartialOnDrop::new(abandoned.clone()));
    assert!(!abandoned.exists());

    let committed = dir.join("committed.part");
    std::fs::write(&committed, b"complete").unwrap();
    let mut guard = RemovePartialOnDrop::new(committed.clone());
    guard.disarm();
    drop(guard);
    assert!(committed.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn tar_xz_release_is_extracted_instead_of_linked_as_an_archive() {
    let dir = reconciliation_dir("tar-xz");
    let staged = dir.join("zls.tar.xz");
    {
        let file = std::fs::File::create(&staged).unwrap();
        let encoder = xz2::write::XzEncoder::new(file, 1);
        let mut archive = tar::Builder::new(encoder);
        let payload = b"#!/bin/sh\nexit 0\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "zls", &payload[..])
            .unwrap();
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap();
    }
    let target = dir.join("expected").join("zls");
    extract_asset(&staged, &dir, &target, "zls-x86_64-linux.tar.xz")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(dir.join("zls")).unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
    assert!(
        !target.exists(),
        "an archive must not be renamed as the binary"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn install_handle_cancel_aborts_the_background_task() {
    let task = tokio::spawn(async {
        std::future::pending::<()>().await;
        unreachable!("cancelled task must not resume")
    });
    let handle = InstallHandle::from_task("cancel-test", task);
    handle.cancel();
    let error = handle.join().await.unwrap_err();
    assert!(error.is_cancelled());
}

#[test]
fn background_record_commit_preserves_existing_entries() {
    let dir = std::env::temp_dir().join(format!(
        "neoism-install-record-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("installed.json");
    let entry = |id: &str| InstalledEntry {
        id: id.to_string(),
        version: "1.0.0".to_string(),
        install_kind: "test".to_string(),
        bin_path: Some(dir.join(id)),
        installed_at: 1,
    };

    persist_installed_entry_to(&path, entry("first")).unwrap();
    persist_installed_entry_to(&path, entry("rust-analyzer")).unwrap();

    let index = InstalledIndex::load_from(&path).unwrap();
    assert!(index.is_installed("first"));
    assert!(index.is_installed("rust-analyzer"));
    let _ = std::fs::remove_dir_all(dir);
}

fn reconciliation_manifest(id: &str, command: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: id.to_string(),
        name: id.to_string(),
        version: "2026-07-13".to_string(),
        description: String::new(),
        author: String::new(),
        downloads: None,
        categories: vec!["LSP".to_string()],
        languages: vec!["Test".to_string()],
        repository_url: None,
        homepage: None,
        executables: vec![command.to_string()],
        install: InstallKind::GithubRelease {
            owner: "owner".to_string(),
            repo: id.to_string(),
            tag: "2026-07-13".to_string(),
            assets: vec![asset(current_target(), "server.gz", command)],
        },
        run: Some(crate::manifest::RunSpec {
            command: vec![command.to_string()],
            env: std::collections::BTreeMap::new(),
        }),
        env_keys: Vec::new(),
    }
}

fn npm_reconciliation_manifest(id: &str, command: &str) -> ExtensionManifest {
    let mut manifest = reconciliation_manifest(id, command);
    manifest.version = "5.6.0".to_string();
    manifest.install = InstallKind::Npm {
        package: id.to_string(),
        version: manifest.version.clone(),
        extra_packages: Vec::new(),
    };
    manifest
}

fn go_reconciliation_manifest(id: &str, command: &str) -> ExtensionManifest {
    let mut manifest = reconciliation_manifest(id, command);
    manifest.version = "v0.23.0".to_string();
    manifest.install = InstallKind::Go {
        package: "golang.org/x/tools/gopls".to_string(),
        version: manifest.version.clone(),
    };
    manifest
}

fn gem_reconciliation_manifest(id: &str, command: &str) -> ExtensionManifest {
    let mut manifest = reconciliation_manifest(id, command);
    manifest.version = "0.60.2".to_string();
    manifest.install = InstallKind::Gem {
        package: id.to_string(),
        version: manifest.version.clone(),
        extra_packages: Vec::new(),
    };
    manifest
}

fn reconciliation_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "neoism-reconcile-{label}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_test_executable(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"managed executable").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn link_test_managed_bin(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::fs::copy(target, link).unwrap();
}

#[test]
fn reconciliation_recovers_only_a_valid_managed_install() {
    let extensions_dir = reconciliation_dir("valid");
    let index_path = extensions_dir.join("installed.json");
    let mut manifest = reconciliation_manifest("rust-analyzer", "rust-analyzer");
    let platform_bin = "rust-analyzer-x86_64-unknown-linux-gnu";
    let InstallKind::GithubRelease { assets, .. } = &mut manifest.install else {
        unreachable!()
    };
    assets[0].bin = platform_bin.to_string();
    let target = extensions_dir
        .join("installed")
        .join("rust-analyzer")
        .join(platform_bin);
    let managed_bin = extensions_dir.join("bin").join("rust-analyzer");
    write_test_executable(&target);
    link_test_managed_bin(&target, &managed_bin);

    let report = reconcile_managed_installs_from(
        std::slice::from_ref(&manifest),
        &extensions_dir,
        &index_path,
    )
    .unwrap();

    assert_eq!(report.recovered, vec!["rust-analyzer"]);
    let entry = report.index.get("rust-analyzer").unwrap();
    assert_eq!(entry.version, manifest.version);
    assert_eq!(entry.bin_path.as_deref(), Some(managed_bin.as_path()));
    let persisted = InstalledIndex::load_from(&index_path).unwrap();
    assert!(persisted.is_installed("rust-analyzer"));
    let _ = std::fs::remove_dir_all(extensions_dir);
}

#[test]
fn reconciliation_recovers_an_npm_managed_shim() {
    let extensions_dir = reconciliation_dir("npm");
    let index_path = extensions_dir.join("installed.json");
    let manifest =
        npm_reconciliation_manifest("bash-language-server", "bash-language-server");
    let shim = extensions_dir
        .join("installed")
        .join("bash-language-server")
        .join("node_modules")
        .join(".bin")
        .join("bash-language-server");
    let managed_bin = extensions_dir.join("bin").join("bash-language-server");
    write_test_executable(&shim);
    link_test_managed_bin(&shim, &managed_bin);

    let report =
        reconcile_managed_installs_from(&[manifest], &extensions_dir, &index_path)
            .unwrap();

    assert_eq!(report.recovered, vec!["bash-language-server"]);
    assert_eq!(
        report
            .index
            .get("bash-language-server")
            .and_then(|entry| entry.bin_path.as_deref()),
        Some(managed_bin.as_path())
    );
    let _ = std::fs::remove_dir_all(extensions_dir);
}

#[test]
fn reconciliation_recovers_a_private_go_binary() {
    let extensions_dir = reconciliation_dir("go");
    let index_path = extensions_dir.join("installed.json");
    let manifest = go_reconciliation_manifest("gopls", "gopls");
    let private_bin =
        go_bin_path(&extensions_dir.join("installed").join("gopls"), "gopls");
    let managed_bin = extensions_dir.join("bin").join("gopls");
    write_test_executable(&private_bin);
    link_test_managed_bin(&private_bin, &managed_bin);

    let report = reconcile_managed_installs_from(
        std::slice::from_ref(&manifest),
        &extensions_dir,
        &index_path,
    )
    .unwrap();

    assert_eq!(report.recovered, vec!["gopls"]);
    assert_eq!(report.index.get("gopls").unwrap().install_kind, "go");
    let _ = std::fs::remove_dir_all(extensions_dir);
}

#[test]
fn reconciliation_recovers_a_private_gem_shim() {
    let extensions_dir = reconciliation_dir("gem");
    let index_path = extensions_dir.join("installed.json");
    let manifest = gem_reconciliation_manifest("solargraph", "solargraph");
    let shim = gem_shim_path(
        &extensions_dir.join("installed").join("solargraph"),
        "solargraph",
    );
    let managed_bin = extensions_dir.join("bin").join("solargraph");
    write_test_executable(&shim);
    link_test_managed_bin(&shim, &managed_bin);

    let report = reconcile_managed_installs_from(
        std::slice::from_ref(&manifest),
        &extensions_dir,
        &index_path,
    )
    .unwrap();

    assert_eq!(report.recovered, vec!["solargraph"]);
    assert_eq!(report.index.get("solargraph").unwrap().install_kind, "gem");
    let _ = std::fs::remove_dir_all(extensions_dir);
}

#[test]
fn reconciliation_never_claims_an_arbitrary_path_binary() {
    let root = reconciliation_dir("path-only");
    let extensions_dir = root.join("extensions");
    let index_path = extensions_dir.join("installed.json");
    let external = root.join("path-bin").join("rust-analyzer");
    write_test_executable(&external);
    let manifest = reconciliation_manifest("rust-analyzer", "rust-analyzer");

    let report =
        reconcile_managed_installs_from(&[manifest], &extensions_dir, &index_path)
            .unwrap();

    assert!(report.recovered.is_empty());
    assert!(!report.index.is_installed("rust-analyzer"));
    assert!(!index_path.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn reconciliation_rejects_a_managed_link_that_escapes_its_install_dir() {
    let root = reconciliation_dir("escape");
    let extensions_dir = root.join("extensions");
    let index_path = extensions_dir.join("installed.json");
    let install_dir = extensions_dir.join("installed").join("rust-analyzer");
    std::fs::create_dir_all(&install_dir).unwrap();
    let expected = install_dir.join("rust-analyzer");
    let external = root.join("outside").join("rust-analyzer");
    let managed_bin = extensions_dir.join("bin").join("rust-analyzer");
    write_test_executable(&expected);
    write_test_executable(&external);
    link_test_managed_bin(&external, &managed_bin);
    let manifest = reconciliation_manifest("rust-analyzer", "rust-analyzer");

    let report =
        reconcile_managed_installs_from(&[manifest], &extensions_dir, &index_path)
            .unwrap();

    assert!(report.recovered.is_empty());
    assert!(!report.index.is_installed("rust-analyzer"));
    let _ = std::fs::remove_dir_all(root);
}
