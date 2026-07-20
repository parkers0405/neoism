use super::*;

fn installable_manifest(command: &str, categories: &[&str]) -> ExtensionManifest {
    ExtensionManifest {
        id: command.to_string(),
        name: command.to_string(),
        version: "1.0.0".into(),
        description: String::new(),
        author: String::new(),
        downloads: None,
        categories: categories
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        languages: Vec::new(),
        repository_url: None,
        homepage: None,
        executables: vec![command.to_string()],
        install: neoism_extensions::InstallKind::Npm {
            package: command.to_string(),
            version: "1.0.0".into(),
            extra_packages: Vec::new(),
        },
        run: Some(neoism_extensions::RunSpec {
            command: vec![command.to_string()],
            env: std::collections::BTreeMap::new(),
        }),
        env_keys: Vec::new(),
    }
}

#[test]
fn remove_managed_python_kernelspec_dir_deletes_stale_kernel_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let kernel_dir = tmp
        .path()
        .join("jupyter")
        .join("share")
        .join("jupyter")
        .join("kernels")
        .join("python3");
    std::fs::create_dir_all(&kernel_dir).unwrap();
    std::fs::write(kernel_dir.join("kernel.json"), "{}").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let first = runtime.block_on(remove_managed_python_kernelspec_dir(&kernel_dir));
    assert!(first.is_ok());
    assert!(!kernel_dir.exists());

    let second = runtime.block_on(remove_managed_python_kernelspec_dir(&kernel_dir));
    assert!(second.is_ok());
}

#[test]
fn install_command_output_detail_preserves_stderr_and_stdout() {
    let detail = install_command_output_detail(b"created venv\n", b"missing lib\n");

    assert!(detail.contains("stderr:\nmissing lib"));
    assert!(detail.contains("stdout:\ncreated venv"));
}

#[test]
fn built_in_syntax_entries_reflect_compiled_in_grammars() {
    let entries = built_in_syntax_entries();
    assert!(!entries.is_empty());
    // Every grammar ships in the binary: no install/uninstall lifecycle.
    assert!(entries
        .iter()
        .all(|entry| entry.status == ExtensionStatus::BuiltIn));
    let rust = entries
        .iter()
        .find(|entry| entry.id == "grammar-rust")
        .expect("compiled-in Rust grammar should be listed");
    assert_eq!(rust.name, "Rust Syntax");
    assert!(rust
        .categories
        .iter()
        .any(|category| category == "Tree-sitter"));
    assert!(rust.languages.iter().any(|language| language == "Rust"));
    assert!(rust.description.contains("compiled into Neoism"));
}

#[test]
fn language_server_catalog_is_derived_from_runtime_adapters() {
    let installed = InstalledIndex::default();
    let entries = language_server_entries(None, &installed);

    // TCP endpoint adapters (host application provides the server).
    let godot = entries
        .iter()
        .find(|entry| entry.id == "builtin-lsp-godot")
        .expect("Godot TCP adapter should be visible in the LSP catalog");
    assert_eq!(godot.status, ExtensionStatus::BuiltIn);
    assert_eq!(godot.lsp_source.as_deref(), Some("built-in/socket"));
    assert!(godot.version.is_empty());
    assert!(godot.categories.iter().any(|category| category == "LSP"));
    assert!(godot.languages.iter().any(|language| language == "Godot"));
    assert!(godot.description.contains("127.0.0.1:6005"));
    assert!(godot.description.contains("must be running"));

    // Stdio adapters are keyed by their catalog package so the install
    // dispatcher and installed.json line up.
    let rust = entries
        .iter()
        .find(|entry| entry.id == "rust-analyzer")
        .expect("Rust adapter should be visible in the LSP catalog");
    assert!(rust.categories.iter().any(|category| category == "LSP"));
    // rust-analyzer advertises formatting, but the Formatters tab is
    // curated for format-first tools — general servers carry the
    // badge-only "Formatting" category instead (see adapter_tab_roles).
    assert!(rust
        .categories
        .iter()
        .any(|category| category == "Formatting"));
    assert!(rust
        .categories
        .iter()
        .all(|category| category != "Formatter"));
    assert!(rust.languages.iter().any(|language| language == "Rust"));
    assert!(rust.description.contains("rust-analyzer"));
    // The source badge always reflects a real engine resolution.
    assert!(matches!(
        rust.lsp_source.as_deref(),
        Some("extension" | "config" | "path" | "missing")
    ));
}

#[test]
fn formatter_and_linter_tabs_are_curated_not_capability_driven() {
    let installed = InstalledIndex::default();
    let entries = language_server_entries(None, &installed);

    // taplo (the TOML adapter) is the only format-first tool in the
    // built-in registry, so it alone joins the Formatters tab.
    let formatters: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.categories.iter().any(|c| c == "Formatter"))
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(formatters, ["taplo"]);

    // No lint-first adapter ships built-in, so the Linters tab holds no
    // language-server cards (the page shows its empty-state hint).
    assert!(entries
        .iter()
        .all(|entry| entry.categories.iter().all(|c| c != "Linter")));

    // Every card still lands on the Language Servers tab.
    assert!(entries.iter().all(|entry| entry
        .categories
        .iter()
        .any(|c| c == "Language Server" || c == "LSP")));
}

#[test]
fn lsp_install_capability_requires_package_and_adapter_agreement() {
    let docker = installable_manifest("docker-language-server", &["LSP"]);
    assert!(extension_manifest_supported_by_host(&docker));
    assert!(manifest_has_registered_lsp_adapter(&docker));
    assert!(manifest_is_auto_installable_lsp(&docker));

    let mut json = installable_manifest("vscode-json-language-server", &["LSP"]);
    json.id = "json-lsp".to_string();
    assert!(manifest_has_registered_lsp_adapter(&json));

    let mut pyright = installable_manifest("pyright-langserver", &["LSP"]);
    pyright.id = "pyright".to_string();
    assert!(manifest_has_registered_lsp_adapter(&pyright));

    // Executable-name coincidence is insufficient: package identity is part
    // of the adapter contract.
    let mut command_name_impostor =
        installable_manifest("docker-language-server", &["LSP"]);
    command_name_impostor.id = "unrelated-docker-tool".to_string();
    assert!(!manifest_has_registered_lsp_adapter(&command_name_impostor));

    let unknown = installable_manifest("not-a-real-language-server", &["LSP"]);
    assert!(extension_manifest_supported_by_host(&unknown));
    assert!(!manifest_has_registered_lsp_adapter(&unknown));
    assert!(!manifest_is_auto_installable_lsp(&unknown));

    let spelled_out =
        installable_manifest("not-a-real-language-server", &["Language Server"]);
    assert!(extension_manifest_supported_by_host(&spelled_out));
    assert!(!manifest_is_auto_installable_lsp(&spelled_out));

    let unrelated_tool = installable_manifest("not-an-lsp", &["Formatter"]);
    assert!(extension_manifest_supported_by_host(&unrelated_tool));
    assert!(!manifest_has_registered_lsp_adapter(&unrelated_tool));
}
