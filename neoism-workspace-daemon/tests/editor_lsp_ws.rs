//! Native editor websocket → workspace-owned real stdio LSP, no external server.
//! One integration test owns its process environment and restores it on drop.
use futures::{SinkExt, StreamExt};
use neoism_protocol::editor::{
    EditorClientMessage as Request, EditorLspAction as Action,
    EditorServerMessage as Reply,
};
use neoism_workspace_daemon::{
    auth::AuthService,
    handshake::PairingTokenStore,
    hosts::PairedHostStore,
    server::{self, AppState},
    workspace::WorkspaceManager,
};
use std::{path::Path, time::Duration};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Env(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Env {
    fn set(values: &[(&'static str, &Path)]) -> Self {
        let mut old = Vec::new();
        for (key, value) in values {
            old.push((*key, std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        for key in ["NEOISM_REQUIRE_AUTH", "NEOISM_DAEMON_TOKEN"] {
            old.push((key, std::env::var_os(key)));
            std::env::remove_var(key);
        }
        Self(old)
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            if let Some(v) = v {
                std::env::set_var(k, v);
            } else {
                std::env::remove_var(k);
            }
        }
    }
}
async fn send(client: &mut Client, id: u64, root: &Path, request: Request) {
    let envelope = serde_json::json!({"Editor":{"request_id":id,"workspace_root":root,"message":request}});
    client
        .send(Message::Text(envelope.to_string()))
        .await
        .unwrap();
}
async fn recv(client: &mut Client, id: u64) -> Reply {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let Message::Text(text) = client.next().await.expect("socket ended").unwrap()
            else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value
                .pointer("/EditorReply/request_id")
                .and_then(|v| v.as_u64())
                == Some(id)
            {
                return serde_json::from_value(value["EditorReply"]["message"].clone())
                    .unwrap();
            }
        }
    })
    .await
    .expect("editor reply timeout")
}
fn open(file: &Path, surface: &str) -> Request {
    Request::OpenBuffer {
        path: file.into(),
        text: Some("fn shared() { shared(); } // unsaved-host\n".into()),
        line: None,
        character: None,
        surface_id: Some(surface.into()),
    }
}
fn query(file: &Path, seq: u64, action: Action) -> Request {
    Request::LspQueryAt {
        seq,
        action,
        path: file.into(),
        line: 0,
        character: 4,
        text: Some("new_name".into()),
        buffer_text: Some("fn shared() { shared(); } // unsaved-host\n".into()),
        open_paths: vec![file.into()],
        surface_id: Some("pane-a".into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_editor_lsp_real_protocol_two_clients_actions_and_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let registry = temp.path().join("registry.json");
    let _env = Env::set(&[
        ("NEOISM_CONFIG_DIR", &config),
        ("NEOISM_DAEMON_DATA_DIR", &data),
        ("NEOISM_WORKSPACE_REGISTRY", &registry),
    ]);
    let root = temp.path().join("workspace 项目 space");
    std::fs::create_dir_all(root.join(".neoism")).unwrap();
    #[cfg(windows)]
    let root = dunce::canonicalize(root).unwrap();
    #[cfg(not(windows))]
    let root = root.canonicalize().unwrap();
    let file = root.join("main 文件 %20.neolsptest");
    let other = root.join("other.neolsptest");
    let source = "fn shared() { shared(); }\n";
    std::fs::write(&file, source).unwrap();
    std::fs::write(&other, source).unwrap();
    let binary = temp.path().join(format!(
        "editor-lsp-fixture{}",
        std::env::consts::EXE_SUFFIX
    ));
    let status = std::process::Command::new(
        std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
    )
    .args(["--edition=2021", "-o"])
    .arg(&binary)
    .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/editor_lsp.rs"))
    .status()
    .unwrap();
    assert!(status.success(), "compile native stdio fixture");
    let log = temp.path().join("lsp.log");
    let quoted_uri = |path: &Path| {
        serde_json::to_string(url::Url::from_file_path(path).unwrap().as_str()).unwrap()
    };
    std::fs::write(root.join(".neoism/config.json"), serde_json::to_vec(&serde_json::json!({"agent":{"lsp":{"editor-fixture":{
        "name":"Editor protocol fixture", "language":"editor-fixture", "extensions":["neolsptest"],
        "command":[binary,quoted_uri(&file),quoted_uri(&other),log]
    }}}})).unwrap()).unwrap();
    let auth = AuthService::bootstrap(&data).unwrap();
    let denied = auth.registry.issue("no-read", Default::default()).unwrap();
    let read_only = auth
        .registry
        .issue(
            "read-only",
            [neoism_protocol::pairing::Permission::ReadFiles]
                .into_iter()
                .collect(),
        )
        .unwrap();
    let runtime = neoism_agent_server::language_server::LspRuntime::new(
        neoism_agent_neoism_adapter::neoism_services(),
    );
    let app = server::router(AppState {
        lsp_runtime: runtime.clone(),
        auth,
        sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
        workspaces: WorkspaceManager::bootstrap(),
        pairing_tokens: PairingTokenStore::load(&config).unwrap(),
        paired_hosts: PairedHostStore::load(&data).unwrap(),
        crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("ws://{addr}/session");
    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // ReadFiles is enforced before any LSP work, with the original ID and surface.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", denied.raw_token).parse().unwrap(),
    );
    let (mut denied_client, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    send(&mut denied_client, 99, &root, open(&file, "denied-pane")).await;
    assert!(
        matches!(recv(&mut denied_client,99).await, Reply::Error {ref surface_id, ref message} if surface_id.as_deref() == Some("denied-pane") && message.contains("ReadFiles"))
    );
    denied_client.close(None).await.unwrap();
    let mut request = url.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", read_only.raw_token).parse().unwrap(),
    );
    let (mut reader, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    send(&mut reader, 100, &root, query(&file, 100, Action::Rename)).await;
    assert!(
        matches!(recv(&mut reader,100).await, Reply::Error {ref message,..} if message.contains("WriteFiles"))
    );
    reader.close(None).await.unwrap();

    // No process-global throttle may suppress B's initial snapshot. IDs may
    // deliberately collide on independent sockets; both must receive a reply.
    send(&mut a, 1, &root, open(&file, "pane-a")).await;
    let first = recv(&mut a, 1).await;
    assert!(
        matches!(first, Reply::LspSnapshot { ref servers, .. } if servers.iter().any(|s| s.state == "connected")),
        "{first:?}"
    );
    let immediate = std::time::Instant::now();
    send(&mut b, 1, &root, open(&file, "pane-b")).await;
    let second = recv(&mut b, 1).await;
    assert!(
        matches!(second, Reply::LspSnapshot { ref surface_id, .. } if surface_id.as_deref() == Some("pane-b")),
        "{second:?}"
    );
    assert!(immediate.elapsed() < Duration::from_secs(3));
    assert!(
        matches!(recv(&mut b, 0).await, Reply::Diagnostics { ref surface_id, ref items, .. } if surface_id.as_deref() == Some("pane-b") && !items.is_empty())
    );

    let actions = [
        Action::Completion,
        Action::Hover,
        Action::SignatureHelp,
        Action::Definition,
        Action::References,
        Action::DocumentSymbols,
        Action::DocumentHighlight,
        Action::CodeActions,
        Action::Format,
    ];
    let mut selected = None;
    for (offset, action) in actions.into_iter().enumerate() {
        let id = 10 + offset as u64;
        send(&mut a, id, &root, query(&file, id, action)).await;
        let reply = recv(&mut a, id).await;
        match (action, &reply) {
            (Action::Completion, Reply::LspCompletions { seq, items, .. }) => {
                assert_eq!(*seq, id);
                assert!(items.iter().any(|i| i.label == "host_completion"));
            }
            (Action::Hover, Reply::LspHoverResult { contents, .. }) => {
                assert!(contents.contains("unsaved-host"))
            }
            (Action::SignatureHelp, Reply::LspHoverResult { contents, .. }) => {
                assert!(contents.contains("shared(value)"))
            }
            (Action::Definition, Reply::LspQueryResult { locations, .. }) => {
                assert_eq!(locations.len(), 1);
                assert_eq!(
                    locations[0].host_path.as_deref(),
                    file.to_str(),
                    "host decodes and canonicalizes URI before sending"
                );
                assert_eq!(locations[0].uri, file.to_string_lossy());
                assert_eq!((locations[0].line, locations[0].character), (0, 3));
            }
            (Action::References, Reply::LspQueryResult { references, .. }) => {
                assert_eq!(references.len(), 1);
                assert_eq!(
                    references[0].path,
                    file.file_name().unwrap().to_string_lossy()
                );
                assert!(references[0].text.contains("unsaved-host"));
            }
            (Action::DocumentSymbols, Reply::LspQueryResult { symbols, .. }) => {
                assert_eq!(symbols[0].name, "shared")
            }
            (Action::DocumentHighlight, Reply::LspQueryResult { highlights, .. }) => {
                assert_eq!(highlights, &vec![(0, 3, 9)])
            }
            (Action::CodeActions, Reply::LspQueryResult { code_actions, .. }) => {
                assert_eq!(code_actions[0].title, "Host fix");
                selected = Some(code_actions[0].clone());
            }
            (Action::Format, Reply::LspQueryResult { edits, .. }) => {
                assert_eq!(edits[0].edits[0].new_text, "formatted")
            }
            _ => panic!("wrong reply to {action:?}: {reply:?}"),
        }
    }
    // Resolve and execute on the originating host server. Open-file edits are
    // typed wire results, unopened-file edits happen only beside the daemon.
    send(
        &mut a,
        30,
        &root,
        Request::ApplyLspCodeActionAt {
            buffer_text: None,
            seq: 30,
            action: selected.clone().unwrap(),
            open_paths: vec![file.clone()],
            surface_id: Some("pane-a".into()),
        },
    )
    .await;
    let applied = recv(&mut a, 30).await;
    assert!(
        matches!(applied, Reply::LspQueryResult {ref edits, ref applied_files, ran_command:true,..} if edits.len()==1 && applied_files == &vec![other.clone()]),
        "{applied:?}"
    );
    if let Reply::LspQueryResult { edits, .. } = &applied {
        assert_eq!(edits[0].path.as_os_str(), file.as_os_str());
    }
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        source,
        "open file remains client-owned"
    );
    assert!(std::fs::read_to_string(&other).unwrap().contains("renamed"));
    std::fs::write(&other, source).unwrap();
    send(&mut a, 31, &root, query(&file, 31, Action::Rename)).await;
    assert!(
        matches!(recv(&mut a,31).await, Reply::LspQueryResult {ref edits,..} if !edits.is_empty())
    );

    // Completion follow-up commands must see the accepted edit, not the text
    // from the preceding completion query. This is one ordered wire request.
    let mut completion_command = selected.as_ref().unwrap().clone();
    completion_command.document_revision.clear();
    completion_command.payload = serde_json::json!({"command":"fixture.finish","title":"Completion","arguments":["completion-expected"]});
    send(
        &mut a,
        35,
        &root,
        Request::ApplyLspCodeActionAt {
            seq: 35,
            action: completion_command,
            buffer_text: Some(
                "fn shared() {} // unsaved-host completion-accepted\n".into(),
            ),
            open_paths: vec![file.clone()],
            surface_id: Some("pane-a".into()),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut a, 35).await,
        Reply::LspQueryResult {
            ran_command: true,
            ..
        }
    ));

    let mut failing_command = selected.as_ref().unwrap().clone();
    failing_command.document_revision.clear();
    failing_command.payload =
        serde_json::json!({"command":"fixture.finish","arguments":["force-error"]});
    send(
        &mut a,
        36,
        &root,
        Request::ApplyLspCodeActionAt {
            seq: 36,
            action: failing_command,
            buffer_text: None,
            open_paths: vec![file.clone()],
            surface_id: Some("pane-a".into()),
        },
    )
    .await;
    assert!(
        matches!(recv(&mut a,36).await, Reply::Error {ref message,..} if message.contains("command failed"))
    );

    // Invalid cross-root query returns a correlated error, then the same
    // connection can retry a valid query. It must not spawn a guest/local LSP.
    let outside = temp.path().join("private.neolsptest");
    std::fs::write(&outside, source).unwrap();
    send(&mut a, 40, &root, query(&outside, 40, Action::Hover)).await;
    assert!(
        matches!(recv(&mut a,40).await, Reply::Error {ref message,..} if message.contains("outside workspace"))
    );
    send(&mut a, 41, &root, query(&file, 41, Action::Hover)).await;
    assert!(
        matches!(recv(&mut a,41).await, Reply::LspHoverResult {ref contents,..} if contents.contains("unsaved-host"))
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), source);

    // Two surfaces on B remain subscribed while A changes a different pane.
    send(&mut b, 50, &root, open(&other, "background-pane")).await;
    assert!(matches!(recv(&mut b, 50).await, Reply::LspSnapshot { .. }));
    let _ = recv(&mut b, 0).await; // initial cached background diagnostic
    let mut changed = query(&file, 51, Action::Hover);
    if let Request::LspQueryAt { buffer_text, .. } = &mut changed {
        *buffer_text = Some("fn shared() {} // unsaved-host changed\n".into());
    }
    send(&mut a, 51, &root, changed).await;
    assert!(matches!(
        recv(&mut a, 51).await,
        Reply::LspHoverResult { .. }
    ));
    let mut surfaces = std::collections::HashSet::new();
    while surfaces.len() < 2 {
        if let Reply::Diagnostics {
            surface_id: Some(surface),
            file_path: Some(path),
            ..
        } = recv(&mut b, 0).await
        {
            assert!(
                (surface == "pane-b" && path == file)
                    || (surface == "background-pane" && path == other)
            );
            surfaces.insert(surface);
        }
    }
    send(
        &mut b,
        52,
        &root,
        Request::CloseLspBuffer {
            surface_id: Some("background-pane".into()),
        },
    )
    .await;

    send(
        &mut a,
        60,
        &root,
        Request::ApplyLspCodeActionAt {
            buffer_text: None,
            seq: 60,
            action: selected.unwrap(),
            open_paths: vec![file.clone()],
            surface_id: Some("pane-a".into()),
        },
    )
    .await;
    assert!(
        matches!(recv(&mut a,60).await, Reply::Error {ref message,..} if message.contains("stale"))
    );

    let logged = std::fs::read_to_string(&log).unwrap();
    for method in [
        "textDocument/didOpen",
        "textDocument/completion",
        "textDocument/hover",
        "textDocument/definition",
        "textDocument/references",
        "textDocument/signatureHelp",
        "textDocument/codeAction",
        "codeAction/resolve",
        "workspace/executeCommand",
        "textDocument/rename",
        "textDocument/formatting",
        "textDocument/documentSymbol",
        "textDocument/documentHighlight",
    ] {
        assert!(logged.contains(method), "missing real LSP call {method}");
    }
    assert!(logged.contains("unsaved-host"));
    assert!(
        logged.find("completion-accepted").unwrap()
            < logged.rfind("workspace/executeCommand").unwrap()
    );
    a.close(None).await.unwrap();
    b.close(None).await.unwrap();
    runtime.shutdown();
    server.abort();
}
