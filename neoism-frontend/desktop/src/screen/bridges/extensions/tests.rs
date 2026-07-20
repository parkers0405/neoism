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
fn godot_syntax_catalog_contains_scripts_and_resources() {
    let gdscript = treesitter_parser_manifest("gdscript").unwrap();
    assert_eq!(gdscript.name, "GDScript Syntax Parser");
    assert!(gdscript
        .languages
        .iter()
        .any(|language| language == "Godot"));
    assert!(gdscript
        .languages
        .iter()
        .any(|language| language == "GDScript"));

    let resources = treesitter_parser_manifest("godot_resource").unwrap();
    assert_eq!(resources.name, "Godot Resources Syntax Parser");
    assert!(resources
        .languages
        .iter()
        .any(|language| language == "Godot"));
    assert!(resources
        .languages
        .iter()
        .any(|language| language == "Godot Resources"));
    assert!(resources
        .repository_url
        .as_deref()
        .is_some_and(|url| url.ends_with("tree-sitter-godot-resource")));

    let shader = treesitter_parser_manifest("glsl").unwrap();
    assert!(shader
        .languages
        .iter()
        .any(|language| language == "Godot Shader"));
}

#[test]
fn built_in_lsp_catalog_is_derived_from_runtime_adapters() {
    let entries = built_in_language_server_entries();
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
}

#[test]
fn lsp_catalog_keeps_packages_visible_and_marks_adapter_capability_truthfully() {
    let docker = installable_manifest("docker-language-server", &["LSP"]);
    assert!(extension_manifest_supported_by_host(&docker));
    assert!(manifest_has_registered_lsp_adapter(&docker));
    assert!(manifest_is_auto_installable_lsp(&docker));
    assert_ne!(
        lsp_source_label(&docker).as_deref(),
        Some("adapter required")
    );

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
    assert_eq!(
        lsp_source_label(&unknown).as_deref(),
        Some("adapter required")
    );

    let spelled_out =
        installable_manifest("not-a-real-language-server", &["Language Server"]);
    assert!(extension_manifest_supported_by_host(&spelled_out));
    assert!(!manifest_is_auto_installable_lsp(&spelled_out));
    assert_eq!(
        lsp_source_label(&spelled_out).as_deref(),
        Some("adapter required")
    );

    let unrelated_tool = installable_manifest("not-an-lsp", &["Formatter"]);
    assert!(extension_manifest_supported_by_host(&unrelated_tool));
    assert!(!manifest_has_registered_lsp_adapter(&unrelated_tool));
}
