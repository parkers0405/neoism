use super::*;

use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

/// Exercises the real Content-Length framing and shared JSON-RPC router over
/// TCP. The fixture identifies as GDScript because Godot's editor-owned LSP is
/// the first production TCP adapter, but no behavior in `LspClient` is
/// Godot-specific.
#[test]
fn tcp_transport_runs_full_document_and_server_request_lifecycle() {
    let workspace = TempWorkspace::new("tcp-transport");
    let file = workspace.path.join("scroll_errors.gd");
    let source = "extends Node\nvar label = \"aé😀中z\"";
    fs::write(&file, source).expect("write GDScript fixture");

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake TCP LSP");
    let port = listener.local_addr().expect("listener address").port();
    let server = thread::spawn(move || run_fake_tcp_server(listener, true));
    let godot = super::super::lsp_adapters::LanguageAdapter::from_builtin(
        super::super::lsp_languages::adapter_by_id("godot")
            .expect("built-in Godot adapter"),
    );

    let mut client = LspClient::connect_tcp(
        &workspace.path,
        &workspace.path,
        "godot-test",
        "godot",
        &godot.routes,
        "127.0.0.1",
        port,
    )
    .expect("connect fake TCP LSP");
    let initialized = client
        .initialize(&workspace.path)
        .expect("initialize fake TCP LSP");
    assert!(initialized.hover_provider);
    client
        .open_document(&file, "gdscript")
        .expect("send didOpen over TCP");

    let published = client
        .wait_for_notification("textDocument/publishDiagnostics", Duration::from_secs(2))
        .expect("wait for TCP diagnostics")
        .expect("TCP diagnostics publication");
    assert_eq!(published["version"], 0);
    assert_eq!(published["diagnostics"][0]["message"], "fake Godot error");
    assert_eq!(
        published["diagnostics"][0]["range"],
        json!({
            "start": { "line": 1, "character": 14 },
            "end": { "line": 1, "character": 20 },
        }),
        "UTF-16 diagnostics must be normalized to UTF-8 byte columns"
    );

    let hover = client
        .request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_file_uri(&file) },
                // Byte column after the astral character. In UTF-16 this is
                // character 17, not byte character 20.
                "position": { "line": 1, "character": 20 },
            }),
            Duration::from_secs(2),
        )
        .expect("hover over TCP");
    assert_eq!(hover["contents"]["value"], "fake Godot hover");
    assert_eq!(
        hover["range"]["end"]["character"], 20,
        "URI-less hover ranges use the request document for normalization"
    );
    client
        .change_document(&file, 1, "extends Node\nvar label = \"changed\"")
        .expect("send incremental didChange over TCP");
    client
        .save_document(&file)
        .expect("send advertised didSave");
    client
        .close_document(&file)
        .expect("send advertised didClose");
    client.shutdown().expect("shutdown TCP LSP");

    let observed = server.join().expect("fake TCP LSP thread");
    assert!(observed.initialized);
    assert_eq!(observed.did_open_language, "gdscript");
    assert_eq!(observed.did_open_version, 0);
    assert_eq!(observed.configuration_reply, json!([null]));
    assert_eq!(observed.position_encodings, ["utf-8", "utf-16", "utf-32"]);
    assert_eq!(
        observed.hover_character, 17,
        "Neoism byte columns must be encoded as negotiated UTF-16 units"
    );
    assert_eq!(
        (observed.change_end_line, observed.change_end_character),
        (1, 20)
    );
    assert!(observed.change_had_range);
    assert!(observed.did_save);
    assert_eq!(
        observed.did_save_text.as_deref(),
        Some("extends Node\nvar label = \"changed\""),
        "didSave must include the current document snapshot when the server advertises save.includeText"
    );
    assert!(observed.did_close);
}

#[test]
fn tcp_transport_omits_save_text_when_the_server_does_not_request_it() {
    let workspace = TempWorkspace::new("tcp-save-without-text");
    let file = workspace.path.join("save.gd");
    fs::write(&file, "extends Node\n").expect("write GDScript fixture");

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake TCP LSP");
    let port = listener.local_addr().expect("listener address").port();
    let server = thread::spawn(move || run_fake_tcp_server(listener, false));
    let godot = super::super::lsp_adapters::LanguageAdapter::from_builtin(
        super::super::lsp_languages::adapter_by_id("godot")
            .expect("built-in Godot adapter"),
    );

    let mut client = LspClient::connect_tcp(
        &workspace.path,
        &workspace.path,
        "godot-save-test",
        "godot",
        &godot.routes,
        "127.0.0.1",
        port,
    )
    .expect("connect fake TCP LSP");
    client
        .initialize(&workspace.path)
        .expect("initialize fake TCP LSP");
    client
        .open_document(&file, "gdscript")
        .expect("send didOpen over TCP");
    client
        .change_document(&file, 1, "extends Node\nvar value = 42\n")
        .expect("send didChange over TCP");
    client
        .save_document(&file)
        .expect("send advertised didSave");
    client
        .close_document(&file)
        .expect("send advertised didClose");
    client.shutdown().expect("shutdown TCP LSP");

    let observed = server.join().expect("fake TCP LSP thread");
    assert!(observed.did_save);
    assert_eq!(
        observed.did_save_text, None,
        "didSave must omit text unless save.includeText is explicitly true"
    );
}

/// Manual integration proof for the editor-owned Godot 4.7 language server.
/// It is ignored because it requires the local `godot` binary and permission
/// to bind/connect loopback port 6005. The deterministic fake above remains
/// the hermetic CI test for the same transport and JSON-RPC lifecycle.
#[test]
#[ignore = "requires local Godot 4.7 editor and loopback port 6005"]
fn real_godot_4_7_publishes_gdscript_fixture_diagnostics() {
    TcpListener::bind(("127.0.0.1", 6005))
        .expect("Godot LSP port 6005 is already in use; close the running editor first");

    let version = Command::new("godot")
        .arg("--version")
        .output()
        .expect("run local Godot binary");
    assert!(version.status.success(), "Godot --version failed");
    let version = String::from_utf8_lossy(&version.stdout);
    assert!(
        version.starts_with("4.7."),
        "this smoke test targets Godot 4.7, found {version:?}"
    );

    let workspace = TempWorkspace::new("real-godot");
    copy_godot_fixture(&workspace.path);
    let file = workspace.path.join("scroll_errors.gd");
    let log = workspace.path.join("godot.log");
    let child = Command::new("godot")
        .args(["--headless", "--editor", "--path"])
        .arg(&workspace.path)
        .arg("--log-file")
        .arg(&log)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start headless Godot editor");
    let _godot = ChildGuard(child);

    let godot = super::super::lsp_adapters::LanguageAdapter::from_builtin(
        super::super::lsp_languages::adapter_by_id("godot")
            .expect("built-in Godot adapter"),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut client = loop {
        match LspClient::connect_tcp(
            &workspace.path,
            &workspace.path,
            "godot-smoke",
            "godot",
            &godot.routes,
            "127.0.0.1",
            6005,
        ) {
            Ok(client) => break client,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!(
                "Godot LSP did not listen within 10 seconds: {error}; log: {}",
                fs::read_to_string(&log).unwrap_or_default()
            ),
        }
    };
    client
        .initialize(&workspace.path)
        .expect("initialize real Godot LSP");
    client
        .open_document(&file, "gdscript")
        .expect("open broken GDScript fixture");

    let deadline = Instant::now() + Duration::from_secs(10);
    let diagnostics = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "Godot did not publish diagnostics; log: {}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        if let Some(published) = client
            .wait_for_notification("textDocument/publishDiagnostics", remaining)
            .expect("read real Godot diagnostics")
        {
            let diagnostics = published["diagnostics"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !diagnostics.is_empty() {
                break diagnostics;
            }
        }
    };
    assert!(
        diagnostics.len() >= 3,
        "expected the three intentional fixture failures, got {diagnostics:#?}"
    );
    let messages = diagnostics
        .iter()
        .filter_map(|item| item.get("message").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        messages.contains("String"),
        "missing type error: {messages}"
    );
    assert!(
        messages.contains("missing_scroll_fixture_value"),
        "missing undeclared identifier error: {messages}"
    );

    client.shutdown().expect("shutdown real Godot LSP client");
}

fn copy_godot_fixture(destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/editor-diagnostics/godot");
    for name in ["project.godot", "scroll_scene.tscn", "scroll_errors.gd"] {
        fs::copy(source.join(name), destination.join(name))
            .unwrap_or_else(|error| panic!("copy Godot fixture {name}: {error}"));
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Debug, Default)]
struct ObservedLifecycle {
    initialized: bool,
    did_open_language: String,
    did_open_version: i64,
    configuration_reply: Value,
    position_encodings: Vec<String>,
    hover_character: i64,
    change_end_line: i64,
    change_end_character: i64,
    change_had_range: bool,
    did_save: bool,
    did_save_text: Option<String>,
    did_close: bool,
}

fn run_fake_tcp_server(
    listener: TcpListener,
    include_save_text: bool,
) -> ObservedLifecycle {
    let (stream, _) = listener.accept().expect("accept fake LSP client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bound fake LSP client reads");
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake reader"));
    let mut writer = stream;
    let mut observed = ObservedLifecycle::default();
    loop {
        let Some(message) =
            read_lsp_message(&mut reader).expect("read client LSP message")
        else {
            break;
        };
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                observed.position_encodings = message
                    .pointer("/params/capabilities/general/positionEncodings")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                write_lsp_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {
                            "capabilities": {
                                "positionEncoding": "utf-16",
                                "hoverProvider": true,
                                "textDocumentSync": {
                                    "openClose": true,
                                    "change": 2,
                                    "save": { "includeText": include_save_text },
                                },
                            }
                        }
                    }),
                )
                .expect("write initialize response");
            }
            Some("initialized") => {
                observed.initialized = true;
                write_lsp_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": "configuration-1",
                        "method": "workspace/configuration",
                        "params": { "items": [{ "section": "godot" }] },
                    }),
                )
                .expect("write server configuration request");
            }
            Some("textDocument/didOpen") => {
                observed.did_open_language = message["params"]["textDocument"]
                    ["languageId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                observed.did_open_version = message["params"]["textDocument"]["version"]
                    .as_i64()
                    .unwrap_or(-1);
                write_lsp_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": message["params"]["textDocument"]["uri"],
                            "version": 0,
                            "diagnostics": [{
                                "range": {
                                    "start": { "line": 1, "character": 14 },
                                    "end": { "line": 1, "character": 17 },
                                },
                                "severity": 1,
                                "source": "godot-test",
                                "message": "fake Godot error",
                            }],
                        }
                    }),
                )
                .expect("publish fake diagnostics");
            }
            Some("textDocument/hover") => {
                observed.hover_character = message["params"]["position"]["character"]
                    .as_i64()
                    .unwrap_or(-1);
                write_lsp_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {
                            "contents": {
                                "kind": "markdown",
                                "value": "fake Godot hover",
                            },
                            "range": {
                                "start": { "line": 1, "character": 14 },
                                "end": { "line": 1, "character": 17 },
                            },
                        }
                    }),
                )
                .expect("write hover response");
            }
            Some("textDocument/didChange") => {
                observed.change_had_range =
                    message.pointer("/params/contentChanges/0/range").is_some();
                observed.change_end_line = message
                    .pointer("/params/contentChanges/0/range/end/line")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1);
                observed.change_end_character = message
                    .pointer("/params/contentChanges/0/range/end/character")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1);
            }
            Some("textDocument/didSave") => {
                observed.did_save = true;
                observed.did_save_text = message
                    .pointer("/params/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("textDocument/didClose") => observed.did_close = true,
            Some("shutdown") => {
                write_lsp_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": null,
                    }),
                )
                .expect("write shutdown response");
            }
            Some("exit") => break,
            None if message.get("id") == Some(&json!("configuration-1")) => {
                observed.configuration_reply = message["result"].clone();
            }
            _ => {}
        }
    }
    observed
}

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("neoism-lsp-{name}-{nonce}"));
        fs::create_dir_all(&path).expect("create temp workspace");
        Self { path }
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
