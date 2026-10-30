//! MANUAL live-daemon probe (always #[ignore]; run explicitly):
//!
//! ```
//! bash scripts/blaze.sh test -p neoism-workspace-daemon --test live_probe -- --ignored --nocapture
//! ```
//!
//! Connects to the locally running daemon as a real CRDT client,
//! decodes the authoritative doc for a buffer, compares it to disk,
//! and round-trips a probe edit so live screens visibly receive it.

use futures::{SinkExt, StreamExt};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

fn buffer_path() -> String {
    std::env::var("NEOISM_PROBE_PATH")
        .unwrap_or_else(|_| "/home/parkersettle/projects/neoism/flake.nix".into())
}

async fn recv_crdt_reply(
    ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
              + Unpin),
    want_request_id: u64,
) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .ok()??
            .ok()?;
        let Message::Text(text) = msg else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(reply) = value.get("CrdtReply") {
            if reply.get("request_id").and_then(Value::as_u64) == Some(want_request_id) {
                return Some(reply.get("message")?.clone());
            }
        }
    }
}

fn bytes_of(value: &Value) -> Vec<u8> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_u64)
                .map(|byte| byte as u8)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "manual probe against a live local daemon"]
async fn probe_live_hub_doc_and_inject_edit() {
    let buffer_id = format!("file://{}", buffer_path());
    let (mut ws, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:7878/session")
        .await
        .expect("daemon reachable");

    // 1) Snapshot the authoritative doc.
    ws.send(Message::Text(
        json!({"Crdt": {"request_id": 1, "message": {"RequestSnapshot": {
            "buffer_id": buffer_id, "state_vector_v1": []}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    let reply = recv_crdt_reply(&mut ws, 1).await.expect("snapshot reply");
    if let Some(error) = reply.get("Error") {
        println!("HUB: doc does not exist → {error}");
        return;
    }
    let snapshot = reply
        .get("Snapshot")
        .or_else(|| reply.get("SnapshotFallback"))
        .expect("snapshot variant");
    let update = bytes_of(snapshot.get("update_v1").unwrap());

    let replica = CrdtTextBuffer::new(424242);
    replica.apply_update_v1(&update).expect("decode update");
    let hub_text = replica.text();
    let disk_text = std::fs::read_to_string(buffer_path()).unwrap_or_default();
    let disk_text = disk_text.strip_suffix('\n').unwrap_or(&disk_text);

    println!("HUB text bytes: {}", hub_text.len());
    println!("DISK text bytes: {}", disk_text.len());
    println!(
        "HUB == DISK: {}",
        if hub_text == disk_text {
            "YES (no typed edits ever reached the hub)"
        } else {
            "NO (the hub HAS edits beyond disk)"
        }
    );
    if hub_text != disk_text {
        // Show where they diverge.
        let common = hub_text
            .bytes()
            .zip(disk_text.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        println!(
            "first divergence at byte {common}; hub context: {:?}",
            &hub_text[common.saturating_sub(20)..(common + 40).min(hub_text.len())]
        );
    }

    // 2) Inject a visible probe edit as a third peer: any live nvim
    //    session displaying this file should show it appear, then
    //    disappear. This tests hub→nvim→screen end to end.
    let edit = replica
        .apply_local_edit(CrdtTextEdit::Insert {
            index: 0,
            content: "PROBE-LIVE-SYNC ".into(),
        })
        .unwrap();
    ws.send(Message::Text(
        json!({"Crdt": {"request_id": 2, "message": {"ApplySync": {"envelope": {
            "buffer_id": buffer_id,
            "origin_client_id": edit.origin_client_id,
            "update_v1": edit.update_v1,
            "state_vector_v1": edit.state_vector_v1,
        }}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    println!("injected PROBE-LIVE-SYNC at doc start; leaving it for 8s…");
    tokio::time::sleep(Duration::from_secs(8)).await;

    let revert = replica
        .apply_local_edit(CrdtTextEdit::Delete {
            index: 0,
            len: "PROBE-LIVE-SYNC ".encode_utf16().count() as u32,
        })
        .unwrap();
    ws.send(Message::Text(
        json!({"Crdt": {"request_id": 3, "message": {"ApplySync": {"envelope": {
            "buffer_id": buffer_id,
            "origin_client_id": revert.origin_client_id,
            "update_v1": revert.update_v1,
            "state_vector_v1": revert.state_vector_v1,
        }}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    println!("probe edit reverted; hub text restored");
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// Reproduce the USER FLOW over the real websocket Editor arm: open a
/// file (spawns a probe-scoped nvim session + seeds the CRDT), type
/// via SendKeys, then snapshot the hub — did the typing fold in?
#[tokio::test]
#[ignore = "manual probe against a live local daemon"]
async fn probe_live_typing_reaches_hub() {
    let buffer_id = format!("file://{}", buffer_path());
    let (mut ws, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:7878/session")
        .await
        .expect("daemon reachable");

    ws.send(Message::Text(
        json!({"Editor": {"request_id": 10, "workspace_root": "/home/parkersettle/projects/neoism", "message": {"OpenBuffer": {
            "path": buffer_path(), "surface_id": "live-probe:1"}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    // Give nvim time to spawn + open + seed.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Baseline hub text.
    ws.send(Message::Text(
        json!({"Crdt": {"request_id": 11, "message": {"RequestSnapshot": {
            "buffer_id": buffer_id, "state_vector_v1": []}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    let reply = recv_crdt_reply(&mut ws, 11).await.expect("snapshot reply");
    let snapshot = reply
        .get("Snapshot")
        .or_else(|| reply.get("SnapshotFallback"))
        .unwrap_or_else(|| panic!("no snapshot: {reply}"));
    let before = CrdtTextBuffer::new(424243);
    before
        .apply_update_v1(&bytes_of(snapshot.get("update_v1").unwrap()))
        .unwrap();
    println!("hub before typing: {} bytes", before.text().len());

    // Type like the user: insert text at the top, leave insert mode.
    ws.send(Message::Text(
        json!({"Editor": {"request_id": 12, "workspace_root": "/home/parkersettle/projects/neoism", "message": {"SendKeys": {
            "bytes": b"ggiPROBETYPE <Esc>".to_vec(), "surface_id": "live-probe:1"}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Hub after typing.
    ws.send(Message::Text(
        json!({"Crdt": {"request_id": 13, "message": {"RequestSnapshot": {
            "buffer_id": buffer_id, "state_vector_v1": []}}}})
        .to_string(),
    ))
    .await
    .unwrap();
    let reply = recv_crdt_reply(&mut ws, 13).await.expect("snapshot reply");
    let snapshot = reply
        .get("Snapshot")
        .or_else(|| reply.get("SnapshotFallback"))
        .unwrap();
    let after = CrdtTextBuffer::new(424244);
    after
        .apply_update_v1(&bytes_of(snapshot.get("update_v1").unwrap()))
        .unwrap();
    let text = after.text();
    println!("hub after typing: {} bytes", text.len());
    println!(
        "TYPING REACHED HUB: {}",
        if text.contains("PROBETYPE") {
            "YES"
        } else {
            "NO — nvim→CRDT fold is broken over the ws path"
        }
    );

    // Clean the probe text out of the doc if it landed.
    if text.contains("PROBETYPE") {
        let edit = after
            .apply_local_edit(CrdtTextEdit::Delete {
                index: 0,
                len: "PROBETYPE ".encode_utf16().count() as u32,
            })
            .unwrap();
        ws.send(Message::Text(
            json!({"Crdt": {"request_id": 14, "message": {"ApplySync": {"envelope": {
                "buffer_id": buffer_id,
                "origin_client_id": edit.origin_client_id,
                "update_v1": edit.update_v1,
                "state_vector_v1": edit.state_vector_v1,
            }}}}})
            .to_string(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        println!("probe text cleaned");
    }
}
