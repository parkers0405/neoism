//! Real wire -> shared editor contracts, intentionally independent of Screen.
//! No GUI/GPU or desktop activation/adoption is exercised here. RemoteFiles is
//! cfg-excluded from the headless client shim, so reads use its actual public
//! DaemonClientHandle dispatch API, not a replica of RemoteFiles or auth logic.
//! Run: bash scripts/test-server-contracts.sh
//! Coverage: HelloOnly and FastBearer each exercise code + markdown with two
//! websocket peers, real file bytes, CRDT snapshots, presence and edits. A third
//! shared pane reopens from stale disk and adopts the current daemon snapshot.
//! This does NOT invoke Screen::remote_editor_files, fresh/restored tab activation,
//! app joined-workspace adoption, or SubscribeWorkspace; those desktop wiring
//! contracts belong to the separate production/UI tests, not this wire fixture.
use super::*;
use neoism_protocol::{
    crdt::*,
    files::{FilesClientMessage, FilesServerMessage},
};
use neoism_ui::{
    editor::{
        code::{doc_sync::CodeDocBinding, CodePane},
        crdt::CrdtTextUpdate,
        markdown::{
            doc_sync::{lines_to_text, MarkdownDocBinding},
            MarkdownPane,
        },
    },
    services::FileOpenSource,
};
use std::path::Path;

// Keep real pane/binding pairs together; only test orchestration is shared.
enum Editor {
    Code(CodePane, CodeDocBinding),
    Markdown(MarkdownPane, MarkdownDocBinding),
}
impl Editor {
    fn host(path: &Path, id: u64, buffer: &str, markdown: bool) -> Self {
        if markdown {
            Self::Markdown(
                MarkdownPane::load_with_source(path.into(), FileOpenSource::Host),
                MarkdownDocBinding::new(id, buffer),
            )
        } else {
            Self::Code(
                CodePane::load_with_source(path.into(), FileOpenSource::Host),
                CodeDocBinding::new(id, buffer),
            )
        }
    }
    fn text(&self) -> String {
        match self {
            Self::Code(p, _) => lines_to_text(&p.buffer.lines),
            Self::Markdown(p, _) => lines_to_text(&p.lines),
        }
    }
    fn ready(&self) -> bool {
        match self {
            Self::Code(p, _) => p.workspace_sync_ready(),
            Self::Markdown(p, _) => p.workspace_sync_ready(),
        }
    }
    fn hydrate(&mut self, text: &str) {
        match self {
            Self::Code(p, _) => {
                p.mark_remote_loading();
                p.apply_remote_source(text);
                assert!(!p.is_dirty());
            }
            Self::Markdown(p, _) => {
                p.mark_remote_loading();
                p.apply_remote_source(text);
                assert!(!p.is_dirty());
            }
        }
    }
    fn seed(&mut self, bytes: &[u8]) {
        match self {
            Self::Code(p, b) => {
                b.seed_from_snapshot(bytes, &mut p.buffer).unwrap();
                assert!(!p.is_dirty());
            }
            Self::Markdown(p, b) => {
                b.seed_from_snapshot(bytes, p).unwrap();
                assert!(!p.is_dirty());
            }
        }
    }
    fn edit(&mut self, text: &str) -> CrdtTextUpdate {
        match self {
            Self::Code(p, b) => {
                p.buffer.cursor_line = 0;
                p.buffer.cursor_col = 0;
                p.buffer.insert_text(text);
                b.flush_local(&p.buffer).expect("real code edit")
            }
            Self::Markdown(p, b) => {
                p.cursor_line = 0;
                p.cursor_col = 0;
                p.insert_text(text);
                b.flush_local(p).expect("real markdown edit")
            }
        }
    }
    fn apply(&mut self, e: &CrdtSyncEnvelope) {
        match self {
            Self::Code(p, b) => {
                assert!(b
                    .apply_remote(e.origin_client_id, &e.update_v1, &mut p.buffer)
                    .unwrap()
                    .flushed_local
                    .is_none());
                assert!(b.flush_local(&p.buffer).is_none(), "no code echo");
            }
            Self::Markdown(p, b) => {
                assert!(b
                    .apply_remote(e.origin_client_id, &e.update_v1, p)
                    .unwrap()
                    .flushed_local
                    .is_none());
                assert!(b.flush_local(p).is_none(), "no markdown echo");
            }
        }
    }
}

async fn connect(opts: DaemonClientOptions) -> DaemonClient {
    let mut client =
        tokio::time::timeout(BOUND, DaemonClient::connect_with_options(opts))
            .await
            .unwrap()
            .unwrap();
    assert!(matches!(
        workspace_reply(&mut client).await,
        WorkspaceServerMessage::HelloAck { accepted: true, .. }
    ));
    assert!(matches!(
        workspace_reply(&mut client).await,
        WorkspaceServerMessage::FullSnapshot { .. }
    ));
    client
}

async fn read_host(client: &mut DaemonClient, root: &Path, name: &str) -> String {
    let handle = client.handle();
    let id = handle.allocate_request_id();
    tokio::time::timeout(
        BOUND,
        handle.send_files_with_request_id(
            id,
            FilesClientMessage::ReadFile { path: name.into() },
            Some(root.into()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(BOUND, async {
        loop {
            if let DaemonServerMessage::Files {
                request_id,
                message,
            } = client.recv().await.expect("files channel")
            {
                if request_id != id {
                    continue;
                }
                let FilesServerMessage::FileContent { path, bytes } = message else {
                    panic!("host read failed: {message:?}")
                };
                assert_eq!(path, name);
                assert!(
                    !bytes.is_empty(),
                    "host bytes must not be a blank placeholder"
                );
                return String::from_utf8(bytes).unwrap();
            }
        }
    })
    .await
    .expect("host read within four seconds")
}

async fn send(client: &DaemonClient, message: CrdtClientMessage) -> u64 {
    tokio::time::timeout(BOUND, client.handle().send_crdt(message))
        .await
        .unwrap()
        .unwrap()
}

async fn crdt_reply(
    client: &mut DaemonClient,
    predicate: impl Fn(&CrdtServerMessage) -> bool,
) -> CrdtServerMessage {
    tokio::time::timeout(BOUND, async {
        loop {
            if let DaemonServerMessage::Crdt { message, .. } =
                client.recv().await.expect("CRDT channel")
            {
                assert!(
                    !matches!(message, CrdtServerMessage::Error { .. }),
                    "CRDT error: {message:?}"
                );
                if predicate(&message) {
                    return message;
                }
            }
        }
    })
    .await
    .expect("CRDT reply/broadcast within four seconds")
}

async fn open(client: &mut DaemonClient, buffer: &str, text: &str, pane: &mut Editor) {
    send(
        client,
        CrdtClientMessage::OpenBuffer {
            buffer_id: buffer.into(),
            initial_text: text.into(),
        },
    )
    .await;
    let msg = crdt_reply(client, |m| matches!(m, CrdtServerMessage::Snapshot { buffer_id, .. } if buffer_id == buffer)).await;
    let CrdtServerMessage::Snapshot { update_v1, .. } = msg else {
        unreachable!()
    };
    pane.seed(&update_v1);
}

fn presence(buffer: &str, peer: &str) -> CrdtPeerPresence {
    CrdtPeerPresence {
        buffer_id: buffer.into(),
        peer_id: peer.into(),
        display_name: peer.into(),
        color: CrdtPresenceColor {
            r: 12,
            g: 34,
            b: 56,
        },
        cursor: CrdtCursorPosition {
            line: 1,
            column: 2,
            offset: Some(3),
        },
        selection: None,
        insert: true,
        rainbow: false,
        updated_at_ms: 0,
    }
}

async fn exercise(env: &Environment, opts: DaemonClientOptions) {
    // Explicit host-root override is essential: default workspace contains decoys.
    let root = env.directory.path().join("hostroot");
    std::fs::create_dir_all(&root).unwrap();
    let mut a = connect(opts.clone()).await;
    let mut b = connect(opts).await;
    for (name, source, markdown) in [
        (
            "shared.rs",
            "fn host_only() { /* 🦀 */ }\n// real host code",
            false,
        ),
        (
            "shared.md",
            "# Host-only note\nActual host markdown 🦀",
            true,
        ),
    ] {
        std::fs::write(root.join(name), source).unwrap();
        std::fs::write(env.directory.path().join(name), "WRONG DEFAULT WORKSPACE")
            .unwrap();
        let path = root.join(name);
        let buffer = format!("file://{}", path.display());
        let mut pa = Editor::host(&path, 101, &buffer, markdown);
        let mut pb = Editor::host(&path, 102, &buffer, markdown);
        // Path exists locally in this fixture, so an accidental disk load would
        // return source here. Host construction MUST stay empty and unready.
        assert_eq!(pa.text(), "");
        assert_eq!(pb.text(), "");
        assert!(!pa.ready());
        assert!(!pb.ready());
        let ta = read_host(&mut a, &root, name).await;
        let tb = read_host(&mut b, &root, name).await;
        assert_eq!(ta, source);
        assert_eq!(tb, source);
        pa.hydrate(&ta);
        pb.hydrate(&tb);
        assert!(pa.ready() && pb.ready());
        assert_eq!(pa.text(), source);
        assert_eq!(pb.text(), source);
        open(&mut a, &buffer, &ta, &mut pa).await;
        open(&mut b, &buffer, &tb, &mut pb).await;
        assert_eq!(pa.text(), source);
        assert_eq!(pb.text(), source);

        // Both directions use unsolicited network presence, not hub calls or
        // a snapshot manufactured by the test. Daemon owns timestamps.
        for turn in 0..2 {
            let (sender, receiver, peer) = if turn == 0 {
                (&a, &mut b, "peer-a")
            } else {
                (&b, &mut a, "peer-b")
            };
            let sent = presence(&buffer, peer);
            send(
                sender,
                CrdtClientMessage::PublishPresence {
                    presence: sent.clone(),
                },
            )
            .await;
            let msg = crdt_reply(receiver, |m| matches!(m, CrdtServerMessage::Presence { update: CrdtPresenceUpdate::Upsert(p) } if p.peer_id == peer && p.buffer_id == buffer)).await;
            let CrdtServerMessage::Presence {
                update: CrdtPresenceUpdate::Upsert(mut received),
            } = msg
            else {
                unreachable!()
            };
            assert!(received.updated_at_ms > 0);
            received.updated_at_ms = 0;
            assert_eq!(received, sent);
        }
        // Two sequential real pane edits, through websocket daemon sync in
        // both directions; replay on both panes also checks own-origin echo.
        for turn in 0..2 {
            let update = if turn == 0 {
                pa.edit("A ")
            } else {
                pb.edit("B ")
            };
            let origin = update.origin_client_id;
            let envelope = CrdtSyncEnvelope {
                buffer_id: buffer.clone(),
                origin_client_id: origin,
                update_v1: update.update_v1,
                state_vector_v1: update.state_vector_v1,
            };
            send(
                if turn == 0 { &a } else { &b },
                CrdtClientMessage::ApplySync { envelope },
            )
            .await;
            for (client, pane) in [(&mut a, &mut pa), (&mut b, &mut pb)] {
                let msg = crdt_reply(client, |m| matches!(m, CrdtServerMessage::Sync { envelope } if envelope.buffer_id == buffer && envelope.origin_client_id == origin)).await;
                let CrdtServerMessage::Sync { envelope } = msg else {
                    unreachable!()
                };
                pane.apply(&envelope);
            }
            assert_eq!(pa.text(), pb.text());
            let prefix = if turn == 0 { "A " } else { "B A " };
            let expected = if markdown {
                // Markdown's actual insertion entry point preserves the heading
                // marker and inserts at the first editable content column.
                source.replacen("Host-only", &format!("{prefix}Host-only"), 1)
            } else {
                format!("{prefix}{source}")
            };
            assert_eq!(pa.text(), expected, "exact host text plus both real edits");
        }
        assert_ne!(pa.text(), source);
        // Restored/reopened shared pane starts with stale host disk bytes, but
        // the actual existing daemon document snapshot must win, without dirty.
        let mut restored = Editor::host(&path, 103, &buffer, markdown);
        restored.hydrate(&read_host(&mut a, &root, name).await);
        open(&mut a, &buffer, source, &mut restored).await;
        assert_eq!(restored.text(), pa.text());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            source,
            "no implicit saves"
        );
    }
}

fn run(fast: bool) {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let env = Environment::new();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let daemon = Daemon::start(&env).await;
        if fast {
            std::fs::create_dir_all(env.directory.path().join("hostroot")).unwrap();
            let ingress = fast_servers::transport_contract::LocalIngress::start(
                daemon.endpoint.clone(),
                env.directory.path().join("hostroot"),
                MASTER,
                GUEST,
            )
            .await;
            let mut opts = options(&ingress.endpoint, Some(GUEST));
            opts.upgrade_auth = DaemonUpgradeAuth::FastBearer;
            exercise(&env, opts).await;
        } else {
            let opts = options(&daemon.endpoint, Some(MASTER));
            assert_eq!(opts.upgrade_auth, DaemonUpgradeAuth::HelloOnly);
            exercise(&env, opts).await;
        }
    });
}

#[test]
fn contract_document_sync_ordinary_hello_only() {
    run(false);
}

#[test]
fn contract_document_sync_fast_bearer() {
    run(true);
}
