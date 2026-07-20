//! Wave 7E: concurrent-edit soak — the correctness gate for calling
//! multiplayer done.
//!
//! A real daemon (axum router on an ephemeral port, same harness as
//! `presence_ws.rs`) hosts ONE shared document. Four simulated clients
//! hammer it concurrently over real websockets:
//!
//! * two **markdown-pane drivers** — the exact desktop client stack
//!   (`MarkdownPane` + `MarkdownDocBinding`, Wave 7B) shipping
//!   `flush_local` ops as `ApplySync` frames and applying broadcast
//!   envelopes through `apply_remote` (caret transform included),
//! * two **raw Yrs clients** — bare `CrdtTextBuffer` replicas applying
//!   randomized UTF-16 edits, the shape a web peer produces.
//!
//! Each client performs ~220 randomized interleaved edits (insert /
//! delete / replace at random char boundaries, biased toward offset 0
//! so same-position races actually happen; snippets include newlines
//! and multibyte scalars) with randomized sub-millisecond delays.
//!
//! Assertions:
//! 1. **Convergence**: after quiescence every replica is byte-identical
//!    to the daemon's authoritative text (raw replica text, pane lines,
//!    AND each binding's internal doc).
//! 2. **Echo/storm bound**: a hub-level subscriber counts every `Sync`
//!    broadcast. The count must EQUAL the number of accepted client
//!    edits (one broadcast per edit — linear, not quadratic) per
//!    origin, and must stop growing once the clients go quiet.
//! 3. **Caret sanity under fire**: the markdown drivers keep a live
//!    caret and assert in-bounds + char-boundary after EVERY remote
//!    apply — the 7B transform must never corrupt it.
//! 4. **Determinism**: the RNG seeds from `NEOISM_SOAK_SEED` (fixed
//!    default) and the seed is printed so failures reproduce.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage, CrdtSyncEnvelope};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit, CrdtTextUpdate};
use neoism_ui::editor::markdown::doc_sync::{lines_to_text, MarkdownDocBinding};
use neoism_ui::editor::markdown::MarkdownPane;
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::crdt::sync::CrdtSyncHub;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::workspace::WorkspaceManager;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Barrier};
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

const DOC: &str = "file:///soak/notes/shared.md";
const BASE_TEXT: &str = "# Soak Doc\n\nalpha bravo charlie\ndelta echo foxtrot\ngolf hotel india\njuliet kilo lima\nmike november oscar\npapa québec romeo 🦀\nsierra tango uniform";
const MD_CLIENTS: usize = 2;
const RAW_CLIENTS: usize = 2;
const EDITS_PER_CLIENT: usize = 220;
const DEFAULT_SEED: u64 = 0x7E_5EED;

fn soak_seed() -> u64 {
    std::env::var("NEOISM_SOAK_SEED")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

// ---------------------------------------------------------------------
// Daemon harness (presence_ws.rs shape, plus a kept hub handle so the
// test can read the authoritative text and count broadcasts).
// ---------------------------------------------------------------------

struct Daemon {
    addr: SocketAddr,
    hub: CrdtSyncHub,
    task: Option<JoinHandle<()>>,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl Daemon {
    async fn spawn() -> Self {
        let config_dir = TempDir::new().expect("pairing config tempdir");
        let data_dir = TempDir::new().expect("auth data tempdir");
        let registry_dir = TempDir::new().expect("registry tempdir");
        let registry_file = registry_dir.path().join("workspaces.json");

        for key in [
            "NEOISM_REQUIRE_AUTH",
            "NEOISM_DAEMON_TOKEN",
            "NEOISM_PRESENCE_TTL_MS",
        ] {
            std::env::remove_var(key);
        }
        std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
        std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
        std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

        let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
        let workspaces = WorkspaceManager::bootstrap();
        let pairing_tokens =
            PairingTokenStore::load(config_dir.path()).expect("pairing store load");
        let hub = CrdtSyncHub::default();

        let app = server::router(AppState {
            auth,
            sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
            workspaces,
            pairing_tokens,
            crdt: hub.clone(),
            paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
        });

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        });

        Daemon {
            addr,
            hub,
            task: Some(task),
            _config_dir: config_dir,
            _data_dir: data_dir,
            _registry_dir: registry_dir,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------------
// Wire helpers
// ---------------------------------------------------------------------

type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = SplitSink<WsClient, Message>;

/// Subset of the daemon's (crate-private) `ServiceClientMessage`.
#[derive(Serialize)]
enum ClientEnvelope<'a> {
    Crdt {
        request_id: u64,
        message: &'a CrdtClientMessage,
    },
}

async fn send_crdt(sink: &mut WsSink, request_id: u64, message: &CrdtClientMessage) {
    let env = ClientEnvelope::Crdt {
        request_id,
        message,
    };
    let payload = serde_json::to_string(&env).expect("serialize envelope");
    sink.send(Message::Text(payload))
        .await
        .expect("send websocket frame");
}

async fn send_apply_sync(
    sink: &mut WsSink,
    request_id: &mut u64,
    update: &CrdtTextUpdate,
) {
    *request_id += 1;
    let message = CrdtClientMessage::ApplySync {
        envelope: CrdtSyncEnvelope {
            buffer_id: DOC.to_string(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1.clone(),
            state_vector_v1: update.state_vector_v1.clone(),
        },
    };
    send_crdt(sink, *request_id, &message).await;
}

/// Pump every `CrdtReply` frame on the socket into an mpsc so the
/// client's edit loop can interleave non-blocking drains with sends.
fn spawn_reader(
    mut stream: SplitStream<WsClient>,
    tx: mpsc::UnboundedSender<CrdtServerMessage>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(Ok(frame)) = stream.next().await {
            let text = match frame {
                Message::Text(t) => t,
                Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                Message::Ping(_) | Message::Pong(_) => continue,
                Message::Close(_) | Message::Frame(_) => break,
            };
            let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(payload) = raw.get("CrdtReply") else {
                continue;
            };
            let Some(message) = payload.get("message") else {
                continue;
            };
            if let Ok(parsed) =
                serde_json::from_value::<CrdtServerMessage>(message.clone())
            {
                if tx.send(parsed).is_err() {
                    break;
                }
            }
        }
    })
}

async fn recv_deadline(
    rx: &mut mpsc::UnboundedReceiver<CrdtServerMessage>,
    timeout: Duration,
    what: &str,
    seed: u64,
) -> CrdtServerMessage {
    tokio::time::timeout(timeout, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("seed={seed}: timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("seed={seed}: socket closed waiting for {what}"))
}

// ---------------------------------------------------------------------
// Randomized edit generation (deterministic per seed)
// ---------------------------------------------------------------------

/// Char-boundary byte offsets of `text`, including the end offset.
fn char_starts(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect()
}

fn random_snippet(rng: &mut StdRng) -> String {
    // Mixed-width palette: ASCII, 2-byte, 3-byte and 4-byte scalars plus
    // newlines, so UTF-8/UTF-16 offset math and line splicing both get
    // exercised.
    const PALETTE: &[char] = &[
        'a', 'b', 'c', 'd', 'm', 'n', 'x', 'y', 'z', ' ', ' ', '.', 'é', 'ü', '∑', '🦀',
        '\n',
    ];
    let len = rng.gen_range(1..=5);
    (0..len)
        .map(|_| PALETTE[rng.gen_range(0..PALETTE.len())])
        .collect()
}

/// Random insert/delete/replace on whole-document text at a random char
/// boundary, biased toward offset 0 so concurrent same-position races
/// actually happen. Always changes the text (a replace can randomly
/// draw a snippet identical to the span it removes; retry those).
fn random_doc_edit(rng: &mut StdRng, text: &str) -> String {
    loop {
        let candidate = random_doc_edit_once(rng, text);
        if candidate != text {
            return candidate;
        }
    }
}

fn random_doc_edit_once(rng: &mut StdRng, text: &str) -> String {
    let bounds = char_starts(text);
    let start_slot = if rng.gen_bool(0.2) {
        0
    } else {
        rng.gen_range(0..bounds.len())
    };
    let start = bounds[start_slot];
    // Insert-weighted op mix (the delete/replace spans average larger
    // than a snippet, so 50/25/25 keeps the doc size a random walk),
    // with a floor that forces inserts so it stays multi-line.
    let op = if bounds.len() < 100 {
        0
    } else {
        rng.gen_range(0..4_usize).saturating_sub(1)
    };
    if op == 0 {
        return format!(
            "{}{}{}",
            &text[..start],
            random_snippet(rng),
            &text[start..]
        );
    }
    let max_span = bounds.len() - 1 - start_slot;
    if max_span == 0 {
        return format!("{}{}", text, random_snippet(rng));
    }
    let span = rng.gen_range(1..=max_span.min(8));
    let end = bounds[start_slot + span];
    if op == 1 {
        format!("{}{}", &text[..start], &text[end..])
    } else {
        format!("{}{}{}", &text[..start], random_snippet(rng), &text[end..])
    }
}

/// Random UTF-16 edit for the raw Yrs clients (same op mix and offset-0
/// bias as the markdown drivers, expressed in `OffsetKind::Utf16`).
fn random_raw_edit(rng: &mut StdRng, text: &str) -> CrdtTextEdit {
    let mut offsets: Vec<u32> = Vec::with_capacity(text.chars().count() + 1);
    offsets.push(0);
    let mut acc = 0u32;
    for ch in text.chars() {
        acc += ch.len_utf16() as u32;
        offsets.push(acc);
    }
    let slot = if rng.gen_bool(0.2) {
        0
    } else {
        rng.gen_range(0..offsets.len())
    };
    let index = offsets[slot];
    let op = if offsets.len() < 100 {
        0
    } else {
        rng.gen_range(0..4_usize).saturating_sub(1)
    };
    let max_span = offsets.len() - 1 - slot;
    if op == 0 || max_span == 0 {
        return CrdtTextEdit::Insert {
            index,
            content: random_snippet(rng),
        };
    }
    if op == 1 {
        let span = rng.gen_range(1..=max_span.min(8));
        CrdtTextEdit::Delete {
            index,
            len: offsets[slot + span] - index,
        }
    } else {
        let span = rng.gen_range(1..=max_span.min(6));
        CrdtTextEdit::Replace {
            index,
            len: offsets[slot + span] - index,
            content: random_snippet(rng),
        }
    }
}

fn set_random_caret(rng: &mut StdRng, pane: &mut MarkdownPane) {
    let line = rng.gen_range(0..pane.lines.len());
    let cols = char_starts(&pane.lines[line]);
    pane.cursor_line = line;
    pane.cursor_col = cols[rng.gen_range(0..cols.len())];
}

fn assert_caret_sane(pane: &MarkdownPane, context: &str, seed: u64) {
    assert!(
        pane.cursor_line < pane.lines.len(),
        "seed={seed}: caret line {} out of bounds ({} lines) {context}",
        pane.cursor_line,
        pane.lines.len(),
    );
    let line = &pane.lines[pane.cursor_line];
    assert!(
        pane.cursor_col <= line.len(),
        "seed={seed}: caret col {} past line end {} {context}",
        pane.cursor_col,
        line.len(),
    );
    assert!(
        line.is_char_boundary(pane.cursor_col),
        "seed={seed}: caret col {} off a char boundary in {line:?} {context}",
        pane.cursor_col,
    );
}

async fn jitter(rng: &mut StdRng) {
    if rng.gen_bool(0.6) {
        tokio::time::sleep(Duration::from_micros(rng.gen_range(50..2_500))).await;
    } else {
        tokio::task::yield_now().await;
    }
}

// ---------------------------------------------------------------------
// Simulated clients
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
struct Report {
    client_id: u64,
    edits_sent: u64,
    syncs_seen: u64,
    caret_checks: u64,
    errors: Vec<String>,
    final_text: String,
}

/// Markdown-pane driver: the desktop's exact client stack over a real
/// websocket. Local edits mutate `pane.lines` (the binding's documented
/// choke-point contract) and ship through `flush_local`; remote
/// envelopes land through `apply_remote` with a caret-sanity assert
/// after every apply.
async fn run_md_client(
    addr: SocketAddr,
    client_id: u64,
    start: Arc<Barrier>,
    done: Arc<Barrier>,
    seed: u64,
) -> Report {
    let mut rng = StdRng::seed_from_u64(seed ^ client_id);
    let (ws, _) = connect_async(format!("ws://{addr}/session"))
        .await
        .expect("websocket upgrade");
    let (mut sink, stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let reader = spawn_reader(stream, tx);

    let mut pane =
        MarkdownPane::from_source(PathBuf::from("/soak/notes/shared.md"), BASE_TEXT);
    let mut binding = MarkdownDocBinding::new(client_id, DOC);
    let mut report = Report {
        client_id,
        ..Report::default()
    };
    let mut request_id = 1u64;

    // Join the shared doc exactly the way the desktop does.
    send_crdt(
        &mut sink,
        request_id,
        &CrdtClientMessage::OpenBuffer {
            buffer_id: DOC.to_string(),
            initial_text: BASE_TEXT.to_string(),
        },
    )
    .await;
    loop {
        match recv_deadline(&mut rx, Duration::from_secs(10), "open snapshot", seed).await
        {
            CrdtServerMessage::Snapshot { update_v1, .. } => {
                binding
                    .seed_from_snapshot(&update_v1, &mut pane)
                    .unwrap_or_else(|err| {
                        panic!("seed={seed}: snapshot seed failed: {err}")
                    });
                break;
            }
            _ => continue,
        }
    }

    start.wait().await;

    for _ in 0..EDITS_PER_CLIENT {
        // Drain whatever broadcasts have piled up since the last turn.
        while let Ok(message) = rx.try_recv() {
            md_apply_message(
                message,
                &mut binding,
                &mut pane,
                &mut sink,
                &mut request_id,
                &mut report,
                seed,
            )
            .await;
        }

        // One randomized local edit through the flush_local choke point.
        let new_text = random_doc_edit(&mut rng, &lines_to_text(&pane.lines));
        pane.lines = new_text.split('\n').map(str::to_string).collect();
        set_random_caret(&mut rng, &mut pane);
        if let Some(update) = binding.flush_local(&pane) {
            send_apply_sync(&mut sink, &mut request_id, &update).await;
            report.edits_sent += 1;
        }

        jitter(&mut rng).await;
    }

    // Fence: a CompactionStatus reply proves the daemon processed every
    // ApplySync this socket sent (frames are handled in order).
    request_id += 1;
    send_crdt(
        &mut sink,
        request_id,
        &CrdtClientMessage::RequestCompactionStatus {
            buffer_id: DOC.to_string(),
        },
    )
    .await;
    loop {
        let message =
            recv_deadline(&mut rx, Duration::from_secs(10), "compaction fence", seed)
                .await;
        if matches!(message, CrdtServerMessage::CompactionStatus(_)) {
            break;
        }
        md_apply_message(
            message,
            &mut binding,
            &mut pane,
            &mut sink,
            &mut request_id,
            &mut report,
            seed,
        )
        .await;
    }

    // Everyone's edits are now in the daemon.
    done.wait().await;

    // Apply in-flight broadcasts until the line goes quiet.
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
    {
        md_apply_message(
            message,
            &mut binding,
            &mut pane,
            &mut sink,
            &mut request_id,
            &mut report,
            seed,
        )
        .await;
    }

    // Lag-recovery catch-up (the protocol's answer to a lagged
    // broadcast channel): fetch the diff against our state vector.
    request_id += 1;
    send_crdt(
        &mut sink,
        request_id,
        &CrdtClientMessage::RequestSnapshot {
            buffer_id: DOC.to_string(),
            state_vector_v1: binding.state_vector_v1(),
        },
    )
    .await;
    loop {
        match recv_deadline(&mut rx, Duration::from_secs(10), "catch-up snapshot", seed)
            .await
        {
            CrdtServerMessage::Snapshot { update_v1, .. }
            | CrdtServerMessage::SnapshotFallback { update_v1, .. } => {
                // Sentinel origin: never equals a real client id, so the
                // echo guard doesn't skip the merge (idempotent in Yrs).
                binding
                    .apply_remote(u64::MAX, &update_v1, &mut pane)
                    .unwrap_or_else(|err| panic!("seed={seed}: catch-up failed: {err}"));
                break;
            }
            message => {
                md_apply_message(
                    message,
                    &mut binding,
                    &mut pane,
                    &mut sink,
                    &mut request_id,
                    &mut report,
                    seed,
                )
                .await;
            }
        }
    }

    assert_caret_sane(&pane, "(after final catch-up)", seed);
    assert!(
        binding.flush_local(&pane).is_none(),
        "seed={seed}: phantom pending op after quiescence (echo-guard breach)"
    );
    report.final_text = lines_to_text(&pane.lines);
    assert_eq!(
        binding.doc_text(),
        report.final_text,
        "seed={seed}: binding replica and pane lines diverged"
    );
    reader.abort();
    report
}

async fn md_apply_message(
    message: CrdtServerMessage,
    binding: &mut MarkdownDocBinding,
    pane: &mut MarkdownPane,
    sink: &mut WsSink,
    request_id: &mut u64,
    report: &mut Report,
    seed: u64,
) {
    match message {
        CrdtServerMessage::Sync { envelope } if envelope.buffer_id == DOC => {
            report.syncs_seen += 1;
            let result = binding
                .apply_remote(envelope.origin_client_id, &envelope.update_v1, pane)
                .unwrap_or_else(|err| {
                    panic!(
                        "seed={seed}: remote apply failed (origin {}): {err}",
                        envelope.origin_client_id
                    )
                });
            assert_caret_sane(pane, "(after remote apply)", seed);
            report.caret_checks += 1;
            if let Some(update) = result.flushed_local {
                send_apply_sync(sink, request_id, &update).await;
                report.edits_sent += 1;
            }
        }
        CrdtServerMessage::Error { message, .. } => report.errors.push(message),
        _ => {}
    }
}

/// Raw Yrs websocket client: a bare replica applying randomized UTF-16
/// edits and folding in every non-own-origin Sync envelope.
async fn run_raw_client(
    addr: SocketAddr,
    client_id: u64,
    start: Arc<Barrier>,
    done: Arc<Barrier>,
    seed: u64,
) -> Report {
    let mut rng = StdRng::seed_from_u64(seed ^ client_id);
    let (ws, _) = connect_async(format!("ws://{addr}/session"))
        .await
        .expect("websocket upgrade");
    let (mut sink, stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let reader = spawn_reader(stream, tx);

    let replica = CrdtTextBuffer::new(client_id);
    let mut report = Report {
        client_id,
        ..Report::default()
    };
    let mut request_id = 1u64;

    send_crdt(
        &mut sink,
        request_id,
        &CrdtClientMessage::OpenBuffer {
            buffer_id: DOC.to_string(),
            initial_text: BASE_TEXT.to_string(),
        },
    )
    .await;
    loop {
        match recv_deadline(&mut rx, Duration::from_secs(10), "open snapshot", seed).await
        {
            CrdtServerMessage::Snapshot { update_v1, .. } => {
                replica.apply_update_v1(&update_v1).unwrap_or_else(|err| {
                    panic!("seed={seed}: snapshot apply failed: {err}")
                });
                break;
            }
            _ => continue,
        }
    }

    start.wait().await;

    for _ in 0..EDITS_PER_CLIENT {
        while let Ok(message) = rx.try_recv() {
            raw_apply_message(message, &replica, &mut report, seed);
        }

        let edit = random_raw_edit(&mut rng, &replica.text());
        let update = replica
            .apply_local_edit(edit.clone())
            .unwrap_or_else(|err| {
                panic!("seed={seed}: local edit {edit:?} failed: {err}")
            });
        send_apply_sync(&mut sink, &mut request_id, &update).await;
        report.edits_sent += 1;

        jitter(&mut rng).await;
    }

    request_id += 1;
    send_crdt(
        &mut sink,
        request_id,
        &CrdtClientMessage::RequestCompactionStatus {
            buffer_id: DOC.to_string(),
        },
    )
    .await;
    loop {
        let message =
            recv_deadline(&mut rx, Duration::from_secs(10), "compaction fence", seed)
                .await;
        if matches!(message, CrdtServerMessage::CompactionStatus(_)) {
            break;
        }
        raw_apply_message(message, &replica, &mut report, seed);
    }

    done.wait().await;

    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
    {
        raw_apply_message(message, &replica, &mut report, seed);
    }

    request_id += 1;
    send_crdt(
        &mut sink,
        request_id,
        &CrdtClientMessage::RequestSnapshot {
            buffer_id: DOC.to_string(),
            state_vector_v1: replica.state_vector_v1(),
        },
    )
    .await;
    loop {
        match recv_deadline(&mut rx, Duration::from_secs(10), "catch-up snapshot", seed)
            .await
        {
            CrdtServerMessage::Snapshot { update_v1, .. }
            | CrdtServerMessage::SnapshotFallback { update_v1, .. } => {
                replica
                    .apply_update_v1(&update_v1)
                    .unwrap_or_else(|err| panic!("seed={seed}: catch-up failed: {err}"));
                break;
            }
            message => raw_apply_message(message, &replica, &mut report, seed),
        }
    }

    report.final_text = replica.text();
    reader.abort();
    report
}

fn raw_apply_message(
    message: CrdtServerMessage,
    replica: &CrdtTextBuffer,
    report: &mut Report,
    seed: u64,
) {
    match message {
        CrdtServerMessage::Sync { envelope } if envelope.buffer_id == DOC => {
            report.syncs_seen += 1;
            // Origin echo guard, same policy as the daemon's nvim applier.
            if envelope.origin_client_id != replica.client_id() {
                replica
                    .apply_update_v1(&envelope.update_v1)
                    .unwrap_or_else(|err| {
                        panic!(
                            "seed={seed}: raw remote apply failed (origin {}): {err}",
                            envelope.origin_client_id
                        )
                    });
            }
        }
        CrdtServerMessage::Error { message, .. } => report.errors.push(message),
        _ => {}
    }
}

// ---------------------------------------------------------------------
// Hub broadcast counter (echo/storm gauge)
// ---------------------------------------------------------------------

#[derive(Default)]
struct BroadcastCounter {
    per_origin: StdMutex<HashMap<u64, u64>>,
    total: AtomicU64,
    lagged: AtomicBool,
}

fn spawn_counter(hub: &CrdtSyncHub, counter: Arc<BroadcastCounter>) -> JoinHandle<()> {
    let mut rx = hub.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(CrdtServerMessage::Sync { envelope }) => {
                    counter.total.fetch_add(1, Ordering::SeqCst);
                    *counter
                        .per_origin
                        .lock()
                        .unwrap()
                        .entry(envelope.origin_client_id)
                        .or_default() += 1;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    counter.lagged.store(true, Ordering::SeqCst);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

// ---------------------------------------------------------------------
// The soak
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_edit_soak_converges_with_bounded_traffic() {
    let seed = soak_seed();
    eprintln!(
        "crdt_soak: seed={seed} — export NEOISM_SOAK_SEED={seed} to reproduce a failure"
    );

    let daemon = Daemon::spawn().await;
    let counter = Arc::new(BroadcastCounter::default());
    let counter_task = spawn_counter(&daemon.hub, counter.clone());

    let clients = MD_CLIENTS + RAW_CLIENTS;
    let start = Arc::new(Barrier::new(clients));
    let done = Arc::new(Barrier::new(clients));

    let mut handles: Vec<(&str, JoinHandle<Report>)> = Vec::new();
    for i in 0..MD_CLIENTS {
        handles.push((
            "md-pane",
            tokio::spawn(run_md_client(
                daemon.addr,
                7_101 + i as u64,
                start.clone(),
                done.clone(),
                seed,
            )),
        ));
    }
    for i in 0..RAW_CLIENTS {
        handles.push((
            "raw-yrs",
            tokio::spawn(run_raw_client(
                daemon.addr,
                7_201 + i as u64,
                start.clone(),
                done.clone(),
                seed,
            )),
        ));
    }

    let mut reports: Vec<(&str, Report)> = Vec::new();
    for (kind, handle) in handles {
        match handle.await {
            Ok(report) => reports.push((kind, report)),
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => panic!("seed={seed}: client task failed: {err}"),
        }
    }

    // ---- 1. convergence: every replica byte-identical to the daemon ----
    let authoritative = daemon.hub.buffers().text(DOC).expect("doc tracked");
    assert_ne!(
        authoritative, BASE_TEXT,
        "seed={seed}: soak produced no net change — the edit loops never ran"
    );
    for (kind, report) in &reports {
        assert!(
            report.errors.is_empty(),
            "seed={seed}: {kind} client {} received daemon errors: {:?}",
            report.client_id,
            report.errors
        );
        assert_eq!(
            report.final_text, authoritative,
            "seed={seed}: {kind} client {} replica diverged from the daemon",
            report.client_id
        );
    }

    // ---- 3. caret sanity ran under real fire ----
    let caret_checks: u64 = reports
        .iter()
        .filter(|(kind, _)| *kind == "md-pane")
        .map(|(_, r)| r.caret_checks)
        .sum();
    assert!(
        caret_checks >= 200,
        "seed={seed}: only {caret_checks} caret checks — the md drivers weren't under fire"
    );

    // ---- 2. echo/storm bound ----
    // Give any straggling broadcast a moment, then demand quiescence.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !counter.lagged.load(Ordering::SeqCst),
        "seed={seed}: the counting subscriber lagged; broadcast volume exploded"
    );
    let total_edits: u64 = reports.iter().map(|(_, r)| r.edits_sent).sum();
    let total_syncs = counter.total.load(Ordering::SeqCst);
    assert!(
        total_edits >= (clients * EDITS_PER_CLIENT) as u64,
        "seed={seed}: only {total_edits} edits were sent"
    );
    // Exactly ONE Sync broadcast per accepted edit: linear in edit
    // count. An echo loop re-emits applied updates (origin id flipped),
    // which shows up here as total_syncs > total_edits.
    assert_eq!(
        total_syncs, total_edits,
        "seed={seed}: Sync broadcasts != edits sent — echo or dropped updates"
    );
    {
        let per_origin = counter.per_origin.lock().unwrap();
        for (kind, report) in &reports {
            assert_eq!(
                per_origin.get(&report.client_id).copied().unwrap_or(0),
                report.edits_sent,
                "seed={seed}: {kind} client {} broadcast count != its edit count",
                report.client_id
            );
        }
        assert!(
            !per_origin.contains_key(&daemon.hub.daemon_client_id()),
            "seed={seed}: daemon-origin Syncs appeared with no nvim in play — echo loop: {per_origin:?}"
        );
    }
    // Storm detector: the counter must be flat once everyone is quiet.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        counter.total.load(Ordering::SeqCst),
        total_syncs,
        "seed={seed}: Sync broadcasts kept flowing after quiescence (echo storm)"
    );

    eprintln!(
        "crdt_soak: seed={seed} converged — {} clients, {} edits, {} Sync broadcasts, doc {} bytes, {} caret checks",
        clients,
        total_edits,
        total_syncs,
        authoritative.len(),
        caret_checks
    );

    counter_task.abort();
}
