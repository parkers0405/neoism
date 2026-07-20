use super::*;

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};
use tokio::sync::broadcast::error::TryRecvError;

use super::super::{
    lsp_adapters::adapters_for_root, path_to_file_uri, subscribe_diagnostics,
    DiagnosticsEvent,
};

/// Service-level proof that a TCP adapter follows the same persistent-client,
/// route, cache, health, and reconnect path as stdio adapters. The listener is
/// deterministic and the adapter is declared entirely through neoism.json;
/// neither the service nor client knows anything about Godot.
#[test]
fn configured_tcp_adapter_reconnects_without_stale_diagnostic_versions() {
    let workspace = TestWorkspace::new("configured-tcp-reconnect");
    let file = workspace.path.join("scroll_errors.gd");
    fs::write(&file, "extends Node\nvar value = broken_v1\n").expect("write TCP fixture");

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake TCP LSP");
    let port = listener.local_addr().expect("listener address").port();
    write_tcp_config(&workspace.path, port);
    let expected_uri = path_to_file_uri(&file);
    let server = thread::spawn(move || run_reconnecting_server(listener, expected_uri));

    let adapter = adapters_for_root(&workspace.path)
        .into_iter()
        .find(|adapter| adapter.id == "protocol-tcp")
        .expect("configured TCP adapter");
    let service = service();
    service.clear_diagnostics(&workspace.path, &file);
    let mut events = subscribe_diagnostics();

    service
        .sync(
            &workspace.path,
            &file,
            Some("extends Node\nvar value = broken_v1\n"),
            &adapter,
        )
        .expect("connect and didOpen first TCP session");
    let first = wait_for_event(&mut events, &file, "tcp diagnostic broken_v1");
    assert_eq!(first.server_id, "protocol-tcp");
    assert_eq!(first.language, "gdscript-test");
    assert_eq!(
        first.diagnostics[0].language.as_deref(),
        Some("gdscript-test")
    );
    assert!(service
        .live_languages(&workspace.path)
        .contains("gdscript-test"));

    let hover = service
        .hover(&workspace.path, &file, 1, 8, &adapter)
        .expect("hover over first TCP session");
    assert_eq!(hover[0].contents, "fake TCP hover: broken_v1");
    wait_for_broken(service, &workspace.path, &adapter);

    // The service intentionally throttles failed endpoints. Once that bounded
    // window passes, the very next operation must establish a new session.
    // Crucially, didOpen restarts at version zero; replacement-client setup
    // must reset the old version guard so this publication is accepted.
    thread::sleep(LSP_RECONNECT_BACKOFF + Duration::from_millis(50));
    service
        .sync(
            &workspace.path,
            &file,
            Some("extends Node\nvar value = broken_v2\n"),
            &adapter,
        )
        .expect("reconnect TCP adapter after bounded backoff");
    let second = wait_for_event(&mut events, &file, "tcp diagnostic broken_v2");
    assert_eq!(second.server_id, "protocol-tcp");
    assert_eq!(
        service.cached_diagnostics(&workspace.path, &file),
        second.diagnostics,
        "replacement session must replace, not lose, the version-zero publish"
    );

    let hover = service
        .hover(&workspace.path, &file, 1, 8, &adapter)
        .expect("hover over replacement TCP session");
    assert_eq!(hover[0].contents, "fake TCP hover: broken_v2");
    wait_for_broken(service, &workspace.path, &adapter);

    let observed = server.join().expect("fake reconnect server thread");
    assert_eq!(observed.len(), 2);
    assert!(observed.iter().all(|session| session.initialized));
    assert!(observed
        .iter()
        .all(|session| session.did_open_language == "gdscript"));
    assert!(observed.iter().all(|session| session.did_open_version == 0));
    assert_eq!(observed[0].text_marker, "broken_v1");
    assert_eq!(observed[1].text_marker, "broken_v2");

    service.clear_diagnostics(&workspace.path, &file);
}

fn write_tcp_config(root: &Path, port: u16) {
    let config = json!({
        "lsp": {
            "protocol-tcp": {
                "name": "Configured TCP Protocol Test",
                "transport": {
                    "kind": "tcp",
                    "host": "127.0.0.1",
                    "port": port,
                },
                "routes": [{
                    "id": "gdscript-test",
                    "documentLanguageId": "gdscript",
                    "extensions": ["gd"],
                }],
                "capabilities": {
                    "diagnostics": true,
                    "hover": true,
                },
            }
        }
    });
    fs::write(
        root.join("neoism.json"),
        serde_json::to_vec_pretty(&config).expect("serialize TCP config"),
    )
    .expect("write TCP config");
}

#[derive(Debug, Default)]
struct ObservedSession {
    initialized: bool,
    did_open_language: String,
    did_open_version: i64,
    text_marker: String,
}

fn run_reconnecting_server(
    listener: TcpListener,
    expected_uri: String,
) -> Vec<ObservedSession> {
    (0..2)
        .map(|_| {
            let (stream, _) = listener.accept().expect("accept fake TCP LSP client");
            run_session(stream, &expected_uri)
        })
        .collect()
}

fn run_session(stream: TcpStream, expected_uri: &str) -> ObservedSession {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bound fake TCP session reads");
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake TCP reader"));
    let mut writer = stream;
    let mut observed = ObservedSession::default();
    loop {
        let Some(message) = read_message(&mut reader).expect("read fake TCP message")
        else {
            return observed;
        };
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => write_message(
                &mut writer,
                &json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": {
                        "capabilities": {
                            "hoverProvider": true,
                            "textDocumentSync": 1,
                        }
                    },
                }),
            )
            .expect("write initialize response"),
            Some("initialized") => observed.initialized = true,
            Some("textDocument/didOpen") => {
                assert_eq!(message["params"]["textDocument"]["uri"], expected_uri);
                observed.did_open_language = message["params"]["textDocument"]
                    ["languageId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                observed.did_open_version = message["params"]["textDocument"]["version"]
                    .as_i64()
                    .unwrap_or(-1);
                let text = message["params"]["textDocument"]["text"]
                    .as_str()
                    .unwrap_or_default();
                observed.text_marker = if text.contains("broken_v2") {
                    "broken_v2"
                } else {
                    "broken_v1"
                }
                .to_string();
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": expected_uri,
                            "version": 0,
                            "diagnostics": [{
                                "range": {
                                    "start": { "line": 1, "character": 12 },
                                    "end": { "line": 1, "character": 21 },
                                },
                                "severity": 1,
                                "source": "protocol-tcp",
                                "message": format!("tcp diagnostic {}", observed.text_marker),
                            }],
                        },
                    }),
                )
                .expect("publish fake TCP diagnostics");
            }
            Some("textDocument/hover") => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {
                            "contents": format!("fake TCP hover: {}", observed.text_marker),
                        },
                    }),
                )
                .expect("write fake TCP hover");
                return observed;
            }
            _ => {}
        }
    }
}

fn wait_for_event(
    receiver: &mut tokio::sync::broadcast::Receiver<DiagnosticsEvent>,
    file: &Path,
    message: &str,
) -> DiagnosticsEvent {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match receiver.try_recv() {
            Ok(event)
                if event.file == file.to_string_lossy()
                    && event.diagnostics.len() == 1
                    && event.diagnostics[0].message == message =>
            {
                return event;
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) | Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Closed) => panic!("diagnostics event bus closed"),
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {message:?} on {}",
            file.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_broken(service: &LspService, root: &Path, adapter: &LanguageAdapter) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if service.broken_reason(root, adapter).is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for TCP disconnect health state"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let content_length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing length"))?;
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

struct TestWorkspace {
    path: PathBuf,
}

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "neoism-lsp-service-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create TCP service workspace");
        Self { path }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
