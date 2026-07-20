use super::lsp_parse::{
    parse_completion, parse_diagnostics, parse_hover, parse_workspace_symbols,
};
use super::lsp_query::language_id_for_path as protocol_language_id_for_path;
use super::*;
use std::{
    env,
    fs::{self, File},
    io::Cursor,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!("neoism-lsp-{name}-{nonce}"));
        fs::create_dir_all(&path).expect("create temp workspace");
        Self { path }
    }

    fn touch(&self, relative: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        File::create(path).expect("create temp file");
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn detects_rust_from_cargo_marker_and_extension() {
    let workspace = TempWorkspace::new("rust");
    workspace.touch("Cargo.toml");
    workspace.touch("src/main.rs");

    let statuses = status(&workspace.path);
    let rust = statuses
        .iter()
        .find(|item| item.id == "rust")
        .expect("rust LSP status");

    assert_eq!(rust.name, "Rust");
    assert_eq!(rust.detected.markers, vec!["Cargo.toml"]);
    assert_eq!(rust.detected.extensions.get("rs"), Some(&1));
    assert_eq!(rust.workspace.root, workspace.path.display().to_string());
    assert!(rust.workspace.root_uri.starts_with("file://"));
}

#[test]
fn detects_multiple_languages_and_skips_ignored_directories() {
    let workspace = TempWorkspace::new("mixed");
    workspace.touch("package.json");
    workspace.touch("src/index.ts");
    workspace.touch("script.py");
    workspace.touch("node_modules/ignored.go");

    let statuses = status(&workspace.path);
    let ids: Vec<&str> = statuses.iter().map(|item| item.id.as_str()).collect();

    assert!(ids.contains(&"typescript"));
    assert!(ids.contains(&"python"));
    assert!(!ids.contains(&"go"));
}

#[test]
fn file_scoped_status_does_not_scan_or_report_unrelated_languages() {
    let workspace = TempWorkspace::new("file-scoped-status");
    workspace.touch("Cargo.toml");
    workspace.touch("src/main.rs");
    workspace.touch("package.json");
    workspace.touch("src/index.ts");
    workspace.touch("data/large.json");

    let file = workspace.path.join("data/large.json");
    let scan = workspace_scan_for_file(&workspace.path, &file);
    assert_eq!(scan.files, 0, "known extensions bypass workspace scans");

    let statuses = status_for_file(&workspace.path, &file);
    assert_eq!(
        statuses
            .iter()
            .map(|status| status.id.as_str())
            .collect::<Vec<_>>(),
        vec!["json"]
    );
}

#[test]
fn unknown_file_extension_does_not_inherit_workspace_lsps() {
    let workspace = TempWorkspace::new("unknown-file-language");
    workspace.touch("Cargo.toml");
    workspace.touch("src/main.rs");
    workspace.touch("notes/example.unknown-language");

    let file = workspace.path.join("notes/example.unknown-language");
    let scan = workspace_scan_for_file(&workspace.path, &file);
    assert_eq!(
        scan.files, 0,
        "unknown extensions must not scan the workspace"
    );
    assert!(status_for_file(&workspace.path, &file).is_empty());
    assert_eq!(
        file_query_specs(&workspace.path, &file, LspOperation::Diagnostics).len(),
        0,
        "workspace LSPs must never receive an unknown document language"
    );
}

#[test]
fn extensionless_file_does_not_inherit_workspace_lsps() {
    let workspace = TempWorkspace::new("extensionless-file-language");
    workspace.touch("Cargo.toml");
    workspace.touch("src/main.rs");
    workspace.touch("Makefile");

    let file = workspace.path.join("Makefile");
    let scan = workspace_scan_for_file(&workspace.path, &file);
    assert_eq!(
        scan.files, 0,
        "extensionless files must not scan the workspace"
    );
    assert!(status_for_file(&workspace.path, &file).is_empty());
    assert_eq!(
        file_query_specs(&workspace.path, &file, LspOperation::Diagnostics).len(),
        0,
        "an unrelated extensionless file must not inherit workspace LSPs"
    );
}

/// Manual end-to-end smoke against the extension-managed rust-analyzer. Kept
/// ignored because normal CI must not depend on a machine-installed server.
/// Run with:
/// `cargo test -p neoism-agent-server --lib managed_rust_analyzer_publishes_fixture_diagnostics -- --ignored --nocapture`
#[test]
#[ignore = "requires an installed rust-analyzer"]
fn managed_rust_analyzer_publishes_fixture_diagnostics() {
    // Deliberately pass the outer Neoism workspace, matching the live editor
    // tree. The engine must select the fixture's nearest Cargo.toml as RA's
    // project root while retaining the outer root for cache/event ownership.
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("outer workspace");
    let project_root = workspace_root.join("fixtures/editor-diagnostics/rust");
    let file = project_root.join("src/main.rs");
    let text = fs::read_to_string(&file).expect("read fixture");

    shutdown_all();
    assert_eq!(
        sync_document(&workspace_root, &file, Some(&text)),
        vec!["rust"]
    );
    let status = status_for_file(&workspace_root, &file);
    assert_eq!(status.len(), 1);
    assert_eq!(Path::new(&status[0].workspace.root), project_root);
    let deadline = Instant::now() + Duration::from_secs(30);
    let diagnostics = loop {
        let diagnostics = cached_diagnostics(&workspace_root, &file);
        if diagnostics.len() >= 4 || Instant::now() >= deadline {
            break diagnostics;
        }
        thread::sleep(Duration::from_millis(100));
    };
    shutdown_all();

    assert!(
        diagnostics.len() >= 4,
        "expected four fixture errors, got {diagnostics:#?}"
    );
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("mismatched types")
            || diagnostic.message.contains("expected `i32`")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("missing_scroll_fixture_function")
    }));
    for line in [37, 103, 168, 221] {
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .range
                    .as_ref()
                    .is_some_and(|range| range.start.line == line)
            }),
            "missing Rust fixture diagnostic at line {line}: {diagnostics:#?}"
        );
    }
}

/// Manual end-to-end smoke for a second stdio adapter. This guards against
/// accidentally making diagnostics or server-request routing Rust-specific.
/// Run with:
/// `cargo test -p neoism-agent-server --lib installed_nil_publishes_nix_fixture_diagnostics -- --ignored --nocapture`
#[test]
#[ignore = "requires an installed nil language server"]
fn installed_nil_publishes_nix_fixture_diagnostics() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/editor-diagnostics/nix")
        .canonicalize()
        .expect("Nix fixture workspace");
    let file = root.join("flake.nix");
    let text = fs::read_to_string(&file).expect("read Nix fixture");

    shutdown_all();
    assert_eq!(sync_document(&root, &file, Some(&text)), vec!["nix"]);
    let deadline = Instant::now() + Duration::from_secs(30);
    let diagnostics = loop {
        let diagnostics = cached_diagnostics(&root, &file);
        if diagnostics.len() >= 3 || Instant::now() >= deadline {
            break diagnostics;
        }
        thread::sleep(Duration::from_millis(100));
    };
    shutdown_all();

    assert!(
        diagnostics.len() >= 3,
        "expected all three intentional Nix fixture errors, got {diagnostics:#?}"
    );
    // nil deliberately reports the generic message "Undefined name" and puts
    // the identifier only in its range, so verify the three scroll-spanning
    // fixture lines instead of assuming message text it does not send.
    for line in [50, 117, 210] {
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .range
                    .as_ref()
                    .is_some_and(|range| range.start.line == line)
            }),
            "missing Nix fixture diagnostic at line {line}: {diagnostics:#?}"
        );
    }
    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic.language.as_deref() == Some("nix")));
}

/// Manual end-to-end smoke against Docker's real language server installed by
/// Neoism's managed Mason pipeline. This verifies a filename-routed,
/// extensionless document and the server's required `start --stdio` command.
#[test]
#[ignore = "requires managed docker-language-server"]
fn managed_docker_language_server_publishes_dockerfile_diagnostics() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/editor-diagnostics/docker")
        .canonicalize()
        .expect("Docker fixture workspace");
    let file = root.join("Dockerfile");
    let text = fs::read_to_string(&file).expect("read Docker fixture");

    shutdown_all();
    assert_eq!(sync_document(&root, &file, Some(&text)), vec!["docker"]);
    // Exercise the same write/touch path the daemon uses. The client sends
    // didSave only when the server advertises it, and otherwise relies on the
    // push-diagnostics lifecycle established by didOpen/didChange.
    let _ = touch_document_diagnostics(&root, &file, Some(&text));
    let deadline = Instant::now() + Duration::from_secs(30);
    let diagnostics = loop {
        let diagnostics = cached_diagnostics(&root, &file);
        if diagnostics.len() >= 3 || Instant::now() >= deadline {
            break diagnostics;
        }
        thread::sleep(Duration::from_millis(100));
    };
    shutdown_all();

    assert!(
        diagnostics.len() >= 3,
        "expected all three intentional Dockerfile errors, got {diagnostics:#?}"
    );
    for line in [50, 117, 210] {
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .range
                    .as_ref()
                    .is_some_and(|range| range.start.line == line)
            }),
            "missing Docker fixture diagnostic at line {line}: {diagnostics:#?}"
        );
    }
    assert!(diagnostics
        .iter()
        .all(|diagnostic| { diagnostic.language.as_deref() == Some("dockerfile") }));
}

/// Manual end-to-end smoke for the shared TypeScript/JavaScript adapter.
/// Both document language ids must use the same server and publish live
/// diagnostics from the scroll fixtures. Run with a real
/// `typescript-language-server` on PATH.
#[test]
#[ignore = "requires typescript-language-server and TypeScript"]
fn installed_typescript_server_publishes_ts_and_js_fixture_diagnostics() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/editor-diagnostics/typescript")
        .canonicalize()
        .expect("TypeScript fixture workspace");

    for (name, expected_language, expected_lines) in [
        // Public LspDiagnostic ranges use the same 1-based display
        // coordinates as the other engine query models. The daemon converts
        // them back to its internal zero-based grid exactly once.
        ("scroll_errors.ts", "typescript", [8, 108, 205]),
        ("javascript_scroll_errors.js", "javascript", [6, 109, 204]),
    ] {
        let file = root.join(name);
        let text = fs::read_to_string(&file).expect("read TypeScript fixture");
        shutdown_all();
        assert_eq!(sync_document(&root, &file, Some(&text)), vec!["typescript"]);
        let deadline = Instant::now() + Duration::from_secs(30);
        let diagnostics = loop {
            let diagnostics = cached_diagnostics(&root, &file);
            if diagnostics.len() >= 3 || Instant::now() >= deadline {
                break diagnostics;
            }
            thread::sleep(Duration::from_millis(100));
        };
        assert!(
            diagnostics.len() >= 3,
            "expected all three intentional {expected_language} errors, got {diagnostics:#?}"
        );
        for line in expected_lines {
            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .range
                        .as_ref()
                        .is_some_and(|range| range.start.line == line)
                }),
                "missing {expected_language} fixture diagnostic at line {line}: {diagnostics:#?}"
            );
        }
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.language.as_deref() == Some(expected_language)
        }));

        // Mirror an insert-mode repair without touching disk. A real server
        // must receive didChange and publish the authoritative empty set;
        // clearing diagnostics must not depend on save or a client poll.
        let fixed = text
            .replace("\"not-a-number\"", "0")
            .replace("requiresLabel(42)", "requiresLabel(\"fixed\")")
            .replace("missingTypeScriptFixtureSymbol", "scrollSection001")
            .replace("missingJavaScriptFixtureSymbol", "scrollSection001");
        assert_eq!(
            sync_document(&root, &file, Some(&fixed)),
            vec!["typescript"]
        );
        let clear_deadline = Instant::now() + Duration::from_secs(10);
        let cleared = loop {
            let diagnostics = cached_diagnostics(&root, &file);
            if diagnostics.is_empty() || Instant::now() >= clear_deadline {
                break diagnostics;
            }
            thread::sleep(Duration::from_millis(25));
        };
        assert!(
            cleared.is_empty(),
            "{expected_language} diagnostics did not clear after an unsaved live edit: {cleared:#?}"
        );
        shutdown_all();
    }
}

#[test]
fn shared_typescript_server_opens_javascript_with_javascript_language_id() {
    assert_eq!(
        protocol_language_id_for_path("typescript", Path::new("src/app.js")),
        Some("javascript")
    );
    assert_eq!(
        protocol_language_id_for_path("typescript", Path::new("src/app.ts")),
        Some("typescript")
    );
}

#[test]
fn every_builtin_route_has_a_real_protocol_language_id() {
    for spec in LANGUAGE_SPECS {
        for route in spec.routes {
            assert!(
                !route.document_language_id.is_empty(),
                "{} / {}",
                spec.id,
                route.id
            );
            assert_ne!(
                route.document_language_id, "plaintext",
                "{} / {}",
                spec.id, route.id
            );

            let representative = if let Some(extension) = route.extensions.first() {
                PathBuf::from(format!("fixture.{extension}"))
            } else {
                let pattern = route
                    .filename_patterns
                    .first()
                    .expect("route has an extension or filename pattern");
                PathBuf::from(pattern.replace('*', "dev"))
            };
            assert_eq!(
                spec.language_id_for_path(&representative),
                Some(route.document_language_id),
                "adapter {} did not route {}",
                spec.id,
                representative.display()
            );
        }
    }
}

#[test]
fn docker_filename_routes_beat_generic_extension_adapters() {
    let cases = [
        ("Dockerfile", "dockerfile"),
        ("Dockerfile.dev", "dockerfile"),
        ("ci.Dockerfile", "dockerfile"),
        ("Containerfile", "dockerfile"),
        ("compose.yaml", "dockercompose"),
        ("docker-compose.override.yml", "dockercompose"),
        ("docker-bake.hcl", "dockerbake"),
        ("docker-bake.json", "dockerbake"),
    ];
    for (file, language_id) in cases {
        let adapter = lsp_languages::best_adapter_for_path(Path::new(file))
            .unwrap_or_else(|| panic!("missing adapter for {file}"));
        assert_eq!(adapter.id, "docker", "wrong adapter for {file}");
        assert_eq!(
            adapter.language_id_for_path(Path::new(file)),
            Some(language_id)
        );
    }

    assert_eq!(
        lsp_languages::best_adapter_for_path(Path::new("ordinary.yaml"))
            .map(|spec| spec.id),
        Some("yaml")
    );
    assert_eq!(
        lsp_languages::best_adapter_for_path(Path::new("ordinary.json"))
            .map(|spec| spec.id),
        Some("json")
    );
}

#[test]
fn nix_godot_and_unknown_files_route_without_plaintext_fallbacks() {
    assert_eq!(language_id_for_path("flake.nix"), Some("nix"));
    assert_eq!(
        lsp_languages::document_language_id_for_path(Path::new("flake.nix")),
        Some("nix")
    );
    assert_eq!(language_id_for_path("player.gd"), Some("gdscript"));
    assert_eq!(
        lsp_languages::best_adapter_for_path(Path::new("player.gd")).map(|spec| spec.id),
        Some("godot")
    );
    for file in ["project.godot", "level.tscn", "player.gd.uid", "Makefile"] {
        assert_eq!(
            language_id_for_path(file),
            None,
            "{file} is not a text-document route"
        );
        assert_eq!(
            lsp_languages::document_language_id_for_path(Path::new(file)),
            None,
            "{file} must never fall back to plaintext"
        );
    }
}

#[test]
fn shared_adapters_keep_one_server_identity_and_route_tsx_jsx() {
    let ts = lsp_languages::best_adapter_for_path(Path::new("app.ts"))
        .expect("TypeScript adapter");
    let js = lsp_languages::best_adapter_for_path(Path::new("app.js"))
        .expect("JavaScript adapter");
    assert_eq!(ts.id, "typescript");
    assert_eq!(js.id, ts.id, "JS and TS must share one server identity");
    assert_eq!(
        ts.language_id_for_path(Path::new("app.tsx")),
        Some("typescriptreact")
    );
    assert_eq!(
        ts.language_id_for_path(Path::new("app.jsx")),
        Some("javascriptreact")
    );
}

#[test]
fn file_scoped_status_keeps_server_identity_but_uses_the_document_route() {
    let workspace = TempWorkspace::new("route-status");
    for (file, server_id, language) in [
        ("Dockerfile", "docker", "dockerfile"),
        ("game/player.gd", "godot", "gdscript"),
        ("web/app.js", "typescript", "javascript"),
    ] {
        workspace.touch(file);
        let statuses = status_for_file(&workspace.path, workspace.path.join(file));
        let status = statuses
            .iter()
            .find(|status| status.id == server_id)
            .unwrap_or_else(|| panic!("missing {server_id} status for {file}"));
        assert_eq!(status.language, language, "wrong route for {file}");
    }
}

#[test]
fn returns_empty_status_for_unrecognized_workspace() {
    let workspace = TempWorkspace::new("empty");
    workspace.touch("README.md");

    assert!(status(&workspace.path).is_empty());
}

#[test]
fn status_includes_configured_lsp_servers() {
    let workspace = TempWorkspace::new("configured");
    fs::write(
        workspace.path.join("neoism.json"),
        r#"{
          "lsp": {
            "custom-rust": {
              "name": "Custom Rust",
              "language": "rust",
              "command": ["definitely-not-a-real-lsp-command"],
              "extensions": ["rs"],
              "capabilities": { "formatting": false }
            }
          }
        }"#,
    )
    .expect("write config");

    let statuses = status(&workspace.path);
    let custom = statuses
        .iter()
        .find(|item| item.id == "custom-rust")
        .expect("configured server status");

    assert_eq!(custom.name, "Custom Rust");
    assert_eq!(custom.language, "rust");
    assert_eq!(custom.command_source, LspCommandSource::Missing);
    assert_eq!(custom.status, LspServerState::Error);
    assert!(!custom.capabilities.formatting);
    assert!(!custom.detected.command_available);
    assert_eq!(custom.detected.extensions.get("rs"), Some(&0));
}

#[test]
fn configured_status_supports_generic_tcp_endpoints_without_a_command() {
    let workspace = TempWorkspace::new("configured-tcp");
    fs::write(
        workspace.path.join("neoism.json"),
        r#"{
          "lsp": {
            "remote-gdscript": {
              "name": "Remote GDScript",
              "language": "gdscript",
              "transport": "tcp",
              "host": "127.0.0.2",
              "port": 7005,
              "extensions": ["gd"]
            }
          }
        }"#,
    )
    .expect("write config");

    let configured = status(&workspace.path)
        .into_iter()
        .find(|status| status.id == "remote-gdscript")
        .expect("configured TCP status");
    assert_eq!(configured.command, vec!["tcp://127.0.0.2:7005"]);
    assert_eq!(configured.command_source, LspCommandSource::Config);
    assert!(configured.detected.command_available);
}

#[test]
fn configured_stdio_adapter_adds_a_genuinely_new_language_route() {
    let workspace = TempWorkspace::new("dynamic-stdio-route");
    workspace.touch("src/example.quux");
    fs::write(
        workspace.path.join("neoism.json"),
        r#"{
          "lsp": {
            "quux-lsp": {
              "name": "Quux Language Server",
              "transport": {
                "kind": "stdio",
                "command": ["/bin/sh", "--fake-quux-lsp"],
                "env": { "QUUX_MODE": "strict" }
              },
              "routes": [{
                "id": "quux",
                "documentLanguageId": "quux-script",
                "extensions": [".quux"],
                "filenamePatterns": ["Quuxfile.*"]
              }],
              "markers": ["quux.project.json"],
              "capabilities": {
                "completion": false,
                "formatting": false,
                "codeActions": false,
                "rename": false
              },
              "initializationOptions": { "dialect": 2 },
              "settings": { "quux": { "lint": true } }
            }
          }
        }"#,
    )
    .expect("write dynamic stdio config");

    let file = workspace.path.join("src/example.quux");
    assert_eq!(
        language_id_for_path_in(&workspace.path, &file).as_deref(),
        Some("quux")
    );
    let adapters = file_query_specs(&workspace.path, &file, LspOperation::Hover);
    assert_eq!(adapters.len(), 1);
    assert_eq!(adapters[0].id, "quux-lsp");
    assert_eq!(
        adapters[0].document_language_id_for_path(&file),
        Some("quux-script")
    );
    assert!(
        file_query_specs(&workspace.path, &file, LspOperation::Completion).is_empty()
    );

    let metadata = language_server_adapters_for(&workspace.path)
        .into_iter()
        .find(|adapter| adapter.id == "quux-lsp")
        .expect("dynamic adapter metadata");
    assert_eq!(metadata.origin, LspAdapterOrigin::Configured);
    assert_eq!(
        metadata.environment.get("QUUX_MODE").map(String::as_str),
        Some("strict")
    );
    assert_eq!(
        metadata.initialization_options,
        Some(json!({ "dialect": 2 }))
    );
    assert_eq!(metadata.settings, Some(json!({ "quux": { "lint": true } })));
    assert!(!metadata.capabilities.completion);
    assert!(!metadata.capabilities.formatting);
    assert!(!metadata.capabilities.code_actions);
    assert!(!metadata.capabilities.rename);
}

#[test]
fn document_lifecycle_is_not_gated_by_diagnostics_capability() {
    let workspace = TempWorkspace::new("completion-only-lifecycle");
    workspace.touch("src/example.liveonly");
    fs::write(
        workspace.path.join("neoism.json"),
        r#"{
          "lsp": {
            "live-only-lsp": {
              "command": ["/bin/sh"],
              "routes": [{
                "id": "live-only",
                "documentLanguageId": "live-only",
                "extensions": ["liveonly"]
              }],
              "capabilities": {
                "diagnostics": false,
                "completion": true,
                "hover": false,
                "definition": false,
                "references": false,
                "implementation": false,
                "callHierarchy": false,
                "documentSymbols": false,
                "formatting": false,
                "codeActions": false,
                "rename": false,
                "workspaceSymbols": false
              }
            }
          }
        }"#,
    )
    .expect("write completion-only adapter");

    let file = workspace.path.join("src/example.liveonly");
    assert!(
        file_query_specs(&workspace.path, &file, LspOperation::Diagnostics).is_empty(),
        "fixture intentionally disables diagnostics"
    );
    let lifecycle = file_lifecycle_specs(&workspace.path, &file);
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(lifecycle[0].id, "live-only-lsp");
}

#[test]
fn invalid_custom_route_is_actionable_and_never_falls_back_to_plaintext() {
    let workspace = TempWorkspace::new("invalid-plaintext-route");
    workspace.touch("broken.quux");
    fs::write(
        workspace.path.join("neoism.json"),
        r#"{
          "lsp": {
            "broken-quux": {
              "command": ["/bin/sh"],
              "routes": [{
                "id": "quux",
                "documentLanguageId": "plaintext",
                "extensions": ["quux"]
              }]
            }
          }
        }"#,
    )
    .expect("write invalid adapter config");

    let file = workspace.path.join("broken.quux");
    assert_eq!(language_id_for_path_in(&workspace.path, &file), None);
    assert!(
        file_query_specs(&workspace.path, &file, LspOperation::Diagnostics).is_empty()
    );
    let status = status_for_file(&workspace.path, &file)
        .into_iter()
        .find(|status| status.id == "broken-quux")
        .expect("invalid adapter status");
    assert_eq!(status.status, LspServerState::Error);
    assert!(status
        .detected
        .message
        .as_deref()
        .is_some_and(|message| message.contains("plaintext")));
}

#[test]
fn legacy_builtin_override_keeps_the_builtin_route_table() {
    let workspace = TempWorkspace::new("legacy-builtin-override");
    workspace.touch("src/lib.rs");
    fs::write(
        workspace.path.join("neoism.json"),
        r#"{
          "lsp": {
            "rust": {
              "language": "rust",
              "command": ["/bin/sh"]
            }
          }
        }"#,
    )
    .expect("write legacy override");

    let adapter = lsp_adapters::adapters_for_root(&workspace.path)
        .into_iter()
        .find(|adapter| adapter.id == "rust")
        .expect("overridden Rust adapter");
    assert!(adapter.is_valid());
    assert_eq!(adapter.routes.len(), 1);
    assert_eq!(
        adapter.logical_language_for_path(Path::new("src/lib.rs")),
        Some("rust")
    );
}

#[test]
fn explicit_lsp_false_disables_builtins_and_cache_refreshes() {
    let workspace = TempWorkspace::new("lsp-disable-cache");
    fs::write(workspace.path.join("neoism.json"), r#"{ "lsp": false }"#)
        .expect("disable LSP");
    assert!(lsp_adapters::adapters_for_root(&workspace.path).is_empty());

    fs::write(workspace.path.join("neoism.json"), r#"{ "lsp": true }"#)
        .expect("enable LSP");
    assert!(
        lsp_adapters::adapters_for_root(&workspace.path).is_empty(),
        "the short-lived cache should serve repeated calls in one editor poll"
    );
    thread::sleep(Duration::from_millis(300));
    assert!(lsp_adapters::adapters_for_root(&workspace.path)
        .iter()
        .any(|adapter| adapter.id == "rust"));
    lsp_adapters::invalidate_adapter_cache(&workspace.path);
}

#[test]
fn public_adapter_metadata_is_the_catalog_source_of_truth() {
    let adapters = language_server_adapters();
    let docker = adapters
        .iter()
        .find(|adapter| adapter.id == "docker")
        .expect("Docker adapter metadata");
    assert_eq!(
        docker.transport,
        LspAdapterTransport::Stdio {
            command: vec![
                "docker-language-server".to_string(),
                "start".to_string(),
                "--stdio".to_string(),
            ]
        }
    );
    assert!(docker
        .routes
        .iter()
        .any(|route| route.document_language_id == "dockercompose"));

    let godot = adapters
        .iter()
        .find(|adapter| adapter.id == "godot")
        .expect("Godot adapter metadata");
    assert!(matches!(
        godot.transport,
        LspAdapterTransport::Tcp {
            default_port: 6005,
            ..
        }
    ));
}

#[test]
fn resolves_command_source_for_runtime_launches() {
    let (missing_command, missing_source) = resolve_lsp_command(
        "definitely-not-installed-language-server",
        vec!["definitely-not-installed-language-server".to_string()],
    );
    assert_eq!(
        missing_command,
        vec!["definitely-not-installed-language-server".to_string()]
    );
    assert_eq!(missing_source, LspCommandSource::Missing);

    let (explicit_command, explicit_source) =
        resolve_lsp_command("sh", vec!["/bin/sh".to_string()]);
    assert_eq!(explicit_command, vec!["/bin/sh".to_string()]);
    assert_eq!(explicit_source, LspCommandSource::Config);

    let (path_command, path_source) = resolve_lsp_command("sh", vec!["sh".to_string()]);
    assert_eq!(path_command, vec!["sh".to_string()]);
    assert_eq!(path_source, LspCommandSource::Path);
}

#[test]
fn normalizes_opencode_lsp_operation_aliases() {
    assert_eq!(normalize_operation("findReferences"), "references");
    assert_eq!(normalize_operation("find_references"), "references");
    assert_eq!(normalize_operation("goToImplementation"), "implementation");
    assert_eq!(
        normalize_operation("go-to-implementation"),
        "implementation"
    );
    assert_eq!(
        normalize_operation("prepareCallHierarchy"),
        "prepare_call_hierarchy"
    );
    assert_eq!(normalize_operation("incomingCalls"), "incoming_calls");
    assert_eq!(normalize_operation("outgoingCalls"), "outgoing_calls");
    assert_eq!(normalize_operation("diagnostics"), "diagnostics");
}

#[test]
fn parses_lsp_framed_messages() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "result": [{ "name": "main" }]
    })
    .to_string();
    let framed = format!(
        "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let mut reader = Cursor::new(framed.into_bytes());

    let message = read_lsp_message(&mut reader)
        .expect("read LSP message")
        .expect("message");

    assert_eq!(message.get("id").and_then(Value::as_i64), Some(7));
    assert_eq!(
        message.pointer("/result/0/name").and_then(Value::as_str),
        Some("main")
    );
}

#[test]
fn parses_workspace_symbol_shapes() {
    let workspace = TempWorkspace::new("symbol-parser");
    workspace.touch("src/main.rs");
    let uri = path_to_file_uri(&workspace.path.join("src/main.rs"));
    let result = json!([
        {
            "name": "main",
            "kind": 12,
            "location": {
                "uri": uri,
                "range": { "start": { "line": 4, "character": 2 } }
            }
        },
        {
            "name": "Thing",
            "kind": 5,
            "location": {
                "targetUri": uri,
                "targetSelectionRange": { "start": { "line": 9, "character": 0 } }
            }
        }
    ]);

    let symbols = parse_workspace_symbols(&workspace.path, "rust", result);

    assert_eq!(
        symbols,
        vec![
            WorkspaceSymbol {
                name: "main".to_string(),
                kind: "function".to_string(),
                path: "src/main.rs".to_string(),
                line: Some(5),
                language: Some("rust".to_string()),
            },
            WorkspaceSymbol {
                name: "Thing".to_string(),
                kind: "class".to_string(),
                path: "src/main.rs".to_string(),
                line: Some(10),
                language: Some("rust".to_string()),
            }
        ]
    );
}

#[test]
fn parses_lsp_diagnostics() {
    let workspace = TempWorkspace::new("diagnostics-parser");
    workspace.touch("src/main.rs");
    workspace.touch("src/lib.rs");
    let uri = path_to_file_uri(&workspace.path.join("src/main.rs"));
    let related_uri = path_to_file_uri(&workspace.path.join("src/lib.rs"));
    let diagnostics = parse_diagnostics(
        &workspace.path,
        &workspace.path.join("src/main.rs"),
        "rust",
        json!({
            "uri": uri,
            "diagnostics": [{
                "range": {
                    "start": { "line": 2, "character": 4 },
                    "end": { "line": 2, "character": 8 }
                },
                "severity": 1,
                "code": "E0425",
                "codeDescription": { "href": "https://example.invalid/E0425" },
                "source": "rust-analyzer",
                "message": "cannot find value",
                "tags": [1, 2],
                "relatedInformation": [{
                    "location": {
                        "uri": related_uri,
                        "range": {
                            "start": { "line": 4, "character": 1 },
                            "end": { "line": 4, "character": 5 }
                        }
                    },
                    "message": "declared here"
                }],
                "data": { "serverFixId": 42 }
            }]
        }),
    );

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, "src/main.rs");
    assert_eq!(diagnostics[0].severity, "error");
    assert_eq!(diagnostics[0].code.as_deref(), Some("E0425"));
    assert_eq!(
        diagnostics[0].code_description.as_deref(),
        Some("https://example.invalid/E0425")
    );
    assert_eq!(diagnostics[0].range.as_ref().unwrap().start.line, 3);
    assert_eq!(diagnostics[0].tags, ["unnecessary", "deprecated"]);
    assert_eq!(diagnostics[0].related_information.len(), 1);
    assert_eq!(diagnostics[0].related_information[0].path, "src/lib.rs");
    assert_eq!(
        diagnostics[0].related_information[0].message,
        "declared here"
    );
    assert_eq!(diagnostics[0].data.as_ref().unwrap()["serverFixId"], 42);
}

#[test]
fn workspace_symbols_queries_fake_stdio_server() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let workspace = TempWorkspace::new("fake-server");
    workspace.touch("src/main.rs");
    let script = workspace.path.join("fake_lsp.py");
    let uri = path_to_file_uri(&workspace.path.join("src/main.rs"));
    fs::write(
            &script,
            format!(
                r#"
import json
import sys

SYMBOL_URI = {uri:?}

def read_message():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        name, value = line.decode("ascii").split(":", 1)
        headers[name.lower()] = value.strip()
    body = sys.stdin.buffer.read(int(headers["content-length"]))
    return json.loads(body)

def send(message):
    body = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    method = message.get("method")
    if method == "initialize":
        send({{"jsonrpc":"2.0","id":message["id"],"result":{{"capabilities":{{"workspaceSymbolProvider": True}}}}}})
    elif method == "workspace/symbol":
        send({{"jsonrpc":"2.0","id":message["id"],"result":[
            {{"name":"main","kind":12,"location":{{"uri":SYMBOL_URI,"range":{{"start":{{"line":2,"character":0}}}}}}}}
        ]}})
    elif method == "shutdown":
        send({{"jsonrpc":"2.0","id":message["id"],"result":None}})
    elif method == "exit":
        sys.exit(0)
"#
            ),
        )
        .expect("write fake LSP server");

    let command = vec!["python3".to_string(), script.display().to_string()];

    let symbols =
        query_workspace_symbols_with_command(&workspace.path, "main", &command, "rust")
            .expect("query fake LSP server");

    assert_eq!(
        symbols,
        vec![WorkspaceSymbol {
            name: "main".to_string(),
            kind: "function".to_string(),
            path: "src/main.rs".to_string(),
            line: Some(3),
            language: Some("rust".to_string()),
        }]
    );
}

#[test]
fn parse_completion_reads_completion_list_and_maps_fields() {
    // CompletionList form with items exercising kind mapping, textEdit
    // precedence over insertText, MarkupContent docs, and preselect.
    let response = json!({
        "isIncomplete": false,
        "items": [
            {
                "label": "println!",
                "kind": 3,
                "detail": "macro",
                "insertText": "println!",
                "textEdit": {
                    "range": {
                        "start": {"line": 2, "character": 4},
                        "end": {"line": 2, "character": 7}
                    },
                    "newText": "println!($0)"
                },
                "sortText": "0001",
                "preselect": true,
                "documentation": {"kind": "markdown", "value": "Prints to stdout."}
            },
            { "label": "  ", "kind": 6 },
            { "label": "value", "kind": 6 }
        ]
    });

    let items = parse_completion(response);

    assert_eq!(items.len(), 2, "blank-label item is dropped");
    let first = &items[0];
    assert_eq!(first.label, "println!");
    assert_eq!(first.kind, "function");
    assert_eq!(first.detail.as_deref(), Some("macro"));
    // textEdit.newText wins over insertText.
    assert_eq!(first.insert_text, "println!($0)");
    assert_eq!(first.sort_text.as_deref(), Some("0001"));
    assert!(first.preselect);
    assert_eq!(first.documentation.as_deref(), Some("Prints to stdout."));
    // Bare CompletionItem[] also parses; label is the insert fallback.
    let bare = parse_completion(json!([{ "label": "foo", "kind": 6 }]));
    assert_eq!(bare.len(), 1);
    assert_eq!(bare[0].insert_text, "foo");
    assert_eq!(bare[0].kind, "variable");
    assert!(!bare[0].preselect);
}

#[test]
fn parse_completion_expands_list_defaults_and_rejects_snippet_items() {
    let items = parse_completion(json!({
        "itemDefaults": {
            "editRange": {
                "insert": {
                    "start": {"line": 4, "character": 2},
                    "end": {"line": 4, "character": 5}
                },
                "replace": {
                    "start": {"line": 4, "character": 2},
                    "end": {"line": 4, "character": 8}
                }
            },
            "insertTextFormat": 1,
            "data": {"request": 17}
        },
        "items": [
            {
                "label": "details",
                "textEditText": "details"
            },
            {
                "label": "snippetOnly",
                "insertText": "snippet(${1:value})$0",
                "insertTextFormat": 2
            }
        ]
    }));
    assert_eq!(
        items.len(),
        1,
        "snippet syntax must never be inserted literally"
    );
    let payload = &items[0].payload;
    assert_eq!(
        payload.pointer("/data/request").and_then(Value::as_u64),
        Some(17)
    );
    assert_eq!(
        payload
            .pointer("/textEdit/insert/start/line")
            .and_then(Value::as_u64),
        Some(4)
    );
    assert_eq!(
        payload
            .pointer("/textEdit/replace/end/character")
            .and_then(Value::as_u64),
        Some(8)
    );
    assert_eq!(
        payload.pointer("/textEdit/newText").and_then(Value::as_str),
        Some("details")
    );
}

#[test]
fn parse_hover_caps_oversized_documentation() {
    let hover = parse_hover(
        std::path::Path::new("/workspace"),
        std::path::Path::new("/workspace/src/lib.rs"),
        "rust",
        json!({
            "contents": {
                "kind": "markdown",
                "value": "x".repeat(10_000)
            }
        }),
    )
    .expect("hover result");

    assert!(hover.contents.len() < 10_000);
    assert!(hover.contents.ends_with("[hover documentation truncated]"));
}
