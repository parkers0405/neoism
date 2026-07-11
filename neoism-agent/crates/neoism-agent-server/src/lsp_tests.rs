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
    time::{SystemTime, UNIX_EPOCH},
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
    assert!(ids.contains(&"javascript"));
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
fn shared_typescript_server_opens_javascript_with_javascript_language_id() {
    assert_eq!(
        protocol_language_id_for_path("typescript", Path::new("src/app.js")),
        "javascript"
    );
    assert_eq!(
        protocol_language_id_for_path("typescript", Path::new("src/app.ts")),
        "typescript"
    );
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
    let uri = path_to_file_uri(&workspace.path.join("src/main.rs"));
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
                "source": "rust-analyzer",
                "message": "cannot find value"
            }]
        }),
    );

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, "src/main.rs");
    assert_eq!(diagnostics[0].severity, "error");
    assert_eq!(diagnostics[0].code.as_deref(), Some("E0425"));
    assert_eq!(diagnostics[0].range.as_ref().unwrap().start.line, 3);
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
