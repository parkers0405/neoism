//! Explicitly opted-in, read-only live probe. Never enable tracing or dump wire payloads.
use neoism_protocol::{
    crdt::CrdtClientMessage as C, workspace::WorkspaceClientMessage as W,
};
use neoism_server_transport_contract::{
    daemon_client::{
        DaemonClient, DaemonClientOptions, DaemonEndpoint, DaemonServerMessage,
    },
    server_registry::{normalize_server_address, ServerRegistry},
};
use serde_json::{json, Value};
use std::time::Duration;
const BOUND: Duration = Duration::from_secs(8);

fn summary(v: &Value) -> Value {
    let Some(obj) = v.as_object() else {
        return json!({"unit_reply": v.as_str()});
    };
    let (kind, body) = obj.iter().next().expect("tagged protocol reply");
    let mut out = json!({"type": kind});
    if let Some(message) = body.get("message").and_then(Value::as_str) {
        out["error_class"] =
            json!(if message.to_ascii_lowercase().contains("unknown buffer")
                || message.to_ascii_lowercase().contains("unknown crdt buffer")
            {
                "unknown_buffer"
            } else if message.to_ascii_lowercase().contains("no active workspace")
                || message
                    .to_ascii_lowercase()
                    .contains("no active project root")
            {
                "no_active_workspace"
            } else if message.to_ascii_lowercase().contains("not found") {
                "not_found"
            } else if message.to_ascii_lowercase().contains("auth") {
                "authorization"
            } else {
                "other_redacted"
            });
    }
    // Explicit allowlist: never log text, bytes, errors, preferences, URLs or credentials.
    for key in [
        "accepted",
        "connected_host_id",
        "client_id",
        "workspace_id",
        "host_id",
        "id",
        "buffer_id",
        "path",
        "active",
        "session_id",
    ] {
        if let Some(value) = body.get(key) {
            out[key] = value.clone();
        }
    }
    for key in ["reason", "peer_identity", "layout"] {
        if let Some(value) = body.get(key) {
            out[format!("{key}_present")] = json!(!value.is_null());
        }
    }
    for key in [
        "sessions",
        "hosts",
        "workspaces",
        "tabs",
        "surfaces",
        "project_roots",
        "peers",
        "entries",
    ] {
        if let Some(items) = body.get(key).and_then(Value::as_array) {
            if key == "entries" {
                out["entries_count"] = json!(items.len());
                continue;
            }
            out[key] = json!(items
                .iter()
                .map(|item| {
                    let mut safe = json!({});
                    for field in [
                        "id",
                        "workspace_id",
                        "host_id",
                        "session_id",
                        "surface_id",
                        "peer_id",
                        "buffer_id",
                        "path",
                        "root_dir",
                        "cwd",
                        "online",
                        "active",
                        "active_workspace_id",
                        "main_session_id",
                        "running_on_host_id",
                        "controlled_by_host_id",
                        "kind",
                        "is_dir",
                        "name",
                    ] {
                        // Names are omitted: a directory listing can contain sensitive filenames.
                        if field != "name" {
                            if let Some(value) = item.get(field) {
                                safe[field] = value.clone();
                            }
                        }
                    }
                    safe
                })
                .collect::<Vec<_>>());
        }
    }
    for key in ["bytes", "update_v1", "state_vector_v1"] {
        if let Some(items) = body.get(key).and_then(Value::as_array) {
            out[format!("{key}_len")] = json!(items.len());
        }
    }
    out
}
async fn reply(client: &mut DaemonClient, wanted: Option<u64>) -> Value {
    tokio::time::timeout(BOUND, async {
        loop {
            let (id, value) =
                match client.recv().await.expect("live reply channel closed") {
                    DaemonServerMessage::Workspace {
                        request_id,
                        message,
                    } => (request_id, serde_json::to_value(message).unwrap()),
                    DaemonServerMessage::Crdt {
                        request_id,
                        message,
                    } => (request_id, serde_json::to_value(message).unwrap()),
                    DaemonServerMessage::Files {
                        request_id,
                        message,
                    } => (request_id, serde_json::to_value(message).unwrap()),
                    _ => continue,
                };
            println!("RX request_id={id} {}", summary(&value));
            if wanted.is_none() || wanted == Some(id) {
                return value;
            }
        }
    })
    .await
    .expect("live response exceeded 8s deadline")
}
async fn workspace(client: &mut DaemonClient, message: W) -> Value {
    let kind = serde_json::to_value(&message).unwrap();
    let kind = kind
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| kind.as_object().unwrap().keys().next().unwrap().clone());
    let id = tokio::time::timeout(BOUND, client.send(message))
        .await
        .expect("send deadline")
        .ok()
        .expect("send failed (details suppressed)");
    println!("TX Workspace::{kind} request_id={id}");
    reply(client, Some(id)).await
}
async fn crdt(client: &mut DaemonClient, message: C) -> Value {
    let kind = serde_json::to_value(&message)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let id = tokio::time::timeout(BOUND, client.handle().send_crdt(message))
        .await
        .expect("send deadline")
        .ok()
        .expect("send failed (details suppressed)");
    println!("TX Crdt::{kind} request_id={id}");
    reply(client, Some(id)).await
}

#[tokio::test]
#[ignore = "LIVE read-only network diagnosis; requires explicit NEOISM_LIVE_DIAGNOSTIC_URL"]
async fn live_ordinary_read_only() {
    let requested = std::env::var("NEOISM_LIVE_DIAGNOSTIC_URL")
        .expect("explicit URL opt-in required");
    let parsed = url::Url::parse(&requested)
        .ok()
        .expect("invalid diagnostic URL");
    assert!(
        parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "URL credentials/query forbidden"
    );
    let endpoint = normalize_server_address(&requested)
        .ok()
        .expect("invalid endpoint");
    let directory = neoism_backend::config::config_dir_path();
    // Production load takes an existing registry lock; don't create any home files.
    assert!(
        directory.join("servers.lock").is_file(),
        "existing registry lock required; refusing to create one"
    );
    let registry = ServerRegistry::load(directory)
        .ok()
        .expect("saved registry unavailable (details suppressed)");
    let matches: Vec<_> = registry
        .servers()
        .iter()
        .filter(|s| s.endpoint == endpoint)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "require exactly one configured matching endpoint"
    );
    let saved = matches[0];
    println!(
        "matched configured server id={} credential_present={}",
        saved.id,
        registry.token(&saved.id).is_some()
    );
    for n in 1..=8 {
        let profile = format!("window-{n}");
        let sub = registry.workspace_subscription(&profile, &saved.id);
        if !sub.subscribed_workspace_ids.is_empty()
            || sub.last_active_workspace_id.is_some()
        {
            println!(
                "saved profile={profile} subscribed={:?} last_active={:?}",
                sub.subscribed_workspace_ids, sub.last_active_workspace_id
            );
        }
    }
    let mut options = DaemonClientOptions::new(
        DaemonEndpoint::parse(&endpoint)
            .ok()
            .expect("endpoint parse"),
    );
    options.token = registry.token(&saved.id).map(str::to_owned);
    options.client_name = "neoism-read-only-live-diagnostic".into();
    options.client_id = uuid::Uuid::new_v4();
    options.reconnect.handshake = BOUND;
    let mut client = DaemonClient::connect_with_options(options)
        .await
        .ok()
        .expect("client construction failed");
    println!(
        "TX automatic Workspace::Hello (credential suppressed), RequestFullSnapshot"
    );
    let ack = reply(&mut client, None).await;
    assert_eq!(
        ack.pointer("/HelloAck/accepted"),
        Some(&json!(true)),
        "expected authenticated HelloAck (payload suppressed)"
    );
    let _initial = reply(&mut client, None).await;
    let tree = workspace(&mut client, W::RequestHostWorkspaceTree).await;
    workspace(&mut client, W::ListHostWorkspaces { host_id: None }).await;
    let roots = workspace(&mut client, W::ListProjectRoots).await;
    if let Some(items) = roots
        .pointer("/ProjectRootList/project_roots")
        .and_then(Value::as_array)
    {
        for root in items.iter().take(8) {
            if let Some(id) = root["id"].as_str() {
                workspace(&mut client, W::GetProjectRootInfo { id: id.into() }).await;
            }
        }
    }
    println!("PHASE pre_selection (fresh diagnostic connection)");
    workspace(&mut client, W::ListSessions).await;
    workspace(&mut client, W::ListEditorSurfaces).await;
    crdt(
        &mut client,
        C::RequestPresenceSnapshot {
            buffer_id: "workspace://presence".into(),
            exclude_peer_id: None,
        },
    )
    .await;
    let workspaces = tree
        .pointer("/HostWorkspaceTree/workspaces")
        .and_then(Value::as_array)
        .expect("workspace tree inventory required");
    let selected = workspaces
        .iter()
        .find(|ws| ws["id"] == "4b7d417f-0aab-4a8c-bfad-42da74b1e1b1")
        .or_else(|| {
            workspaces
                .iter()
                .find(|ws| ws["root_dir"].is_string() && ws["main_session_id"].is_null())
        })
        .expect("existing root-only workspace required; will not create one");
    let id = selected["id"].as_str().expect("workspace id");
    println!(
        "PHASE selected_workspace workspace_id={id} (connection-local subscription only)"
    );
    let ack = workspace(
        &mut client,
        W::SubscribeWorkspace {
            workspace_id: id.into(),
        },
    )
    .await;
    let acknowledged = ack
        .pointer("/HostWorkspaceUpserted/workspace/id")
        .and_then(Value::as_str)
        == Some(id);
    println!("SELECTED workspace_id={id} acknowledged={acknowledged} expected_ack=HostWorkspaceUpserted");
    // Keep collecting inventories even when the running daemon rejects selection.
    let sessions = workspace(&mut client, W::ListSessions).await;
    let selected_surfaces = workspace(&mut client, W::ListEditorSurfaces).await;
    let snapshot =
        workspace(&mut client, W::RequestFullSnapshot { since_offset: None }).await;
    let presence = crdt(
        &mut client,
        C::RequestPresenceSnapshot {
            buffer_id: "workspace://presence".into(),
            exclude_peer_id: None,
        },
    )
    .await;
    let tabs = workspace(
        &mut client,
        W::ListWorkspaceTabs {
            workspace_id: id.into(),
        },
    )
    .await;
    println!(
        "SELECTED_COUNTS {}",
        json!({
            "acknowledged": acknowledged,
            "sessions": sessions.pointer("/SessionList/sessions").and_then(Value::as_array).map(Vec::len),
            "surfaces": selected_surfaces.pointer("/EditorSurfaceList/surfaces").and_then(Value::as_array).map(Vec::len),
            "snapshot_sessions": snapshot.pointer("/FullSnapshot/sessions").and_then(Value::as_array).map(Vec::len),
            "snapshot_layout_present": snapshot.pointer("/FullSnapshot/layout").map(|layout| !layout.is_null()),
            "workspace_presence_peers": presence.pointer("/PresenceSnapshot/peers").and_then(Value::as_array).map(Vec::len),
            "tabs": tabs.pointer("/WorkspaceTabList/tabs").and_then(Value::as_array).map(Vec::len),
        })
    );
    println!("PHASE post_selection_host_inventory (check global host active pointers)");
    workspace(&mut client, W::RequestHostWorkspaceTree).await;
    println!("DONE: only connection-local SubscribeWorkspace added; no file/document probes, creation, edits, saves, presence publications, host selection, or GUI inspection");
}
