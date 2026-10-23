use super::*;
use std::collections::HashSet;

fn session(id: &str, parent: Option<&str>, hint: Option<&str>) -> SessionInfo {
    let mut value = json!({
        "id": id, "slug": id, "projectId": "global", "directory": "/tmp",
        "title": id, "version": "test", "time": {"created": 1, "updated": 1},
    });
    if let Some(parent) = parent {
        value["parentId"] = json!(parent);
    }
    if let Some(hint) = hint {
        value[crate::execution_activity::EXECUTION_ROOT_KEY] = json!(hint);
    }
    serde_json::from_value(value).unwrap()
}

// The former SSE helper's fixed-point parent closure, not catalogue membership.
fn reference_closure(sessions: &[SessionInfo], root: &str) -> HashSet<String> {
    let mut family = HashSet::from([root.to_string()]);
    loop {
        let before = family.len();
        for session in sessions {
            if session.parent_id.as_ref().is_some_and(|p| family.contains(p.as_str())) {
                family.insert(session.id.to_string());
            }
        }
        if family.len() == before {
            return family;
        }
    }
}

#[tokio::test]
async fn indexed_sse_descendants_match_parent_closure_without_catalogue_hydration() {
    let state = AppState::open_database(std::env::temp_dir().join(format!(
        "sse-descendants-{}.db", neoism_agent_core::new_session_id()
    ))).await.unwrap();
    let mut sessions = vec![
        session("deep", Some("child"), None),
        session("child", Some("root"), Some("elsewhere")),
        session("root", None, None),
        session("sibling", Some("root"), None),
        session("hint-only", None, Some("root")),
        session("orphan", Some("missing"), None),
        session("orphan-child", Some("orphan"), None),
        session("cycle-a", Some("cycle-b"), None),
        session("cycle-b", Some("cycle-a"), None),
        session("cycle-tail", Some("cycle-a"), None),
        session("self-cycle", Some("self-cycle"), None),
    ];
    // Force multiple batches and a following generation beyond those batches.
    for i in 0..140 {
        sessions.push(session(&format!("wide-{i}"), Some("root"), None));
        sessions.push(session(&format!("leaf-{i}"), Some(&format!("wide-{i}")), None));
    }
    for info in &sessions {
        state.inner.store.insert_session(info).await.unwrap();
    }
    // The canonical expression index must serve the actual frontier predicate.
    let plan = state.inner.store.db.fetch_all(
        "EXPLAIN QUERY PLAN SELECT id FROM sessions WHERE json_extract(info_json, '$.parentId') = ? UNION ALL SELECT id FROM sessions WHERE json_extract(info_json, '$.parentId') = ?",
        vec![text("root"), text("child")],
    ).await.unwrap();
    assert_eq!(plan.iter().filter(|row| row.get_str("detail").unwrap().contains("idx_sessions_catalog_parent")).count(), 2);
    for root in sessions.iter().map(|s| s.id.as_str()).chain(["missing", "absent", "elsewhere"]) {
        assert_eq!(state.inner.store.session_descendant_ids(root).await.unwrap(), reference_closure(&sessions, root), "root {root}");
    }
    // Simulate a legacy store whose sidecar hasn't backfilled. A valid JSON
    // row with invalid SessionInfo must not poison an unrelated family either.
    let bad = session("bad", None, None);
    state.inner.store.insert_session(&bad).await.unwrap();
    state.inner.store.db.execute("UPDATE sessions SET info_json = '{}' WHERE id = 'bad'", vec![]).await.unwrap();
    state.inner.store.db.execute("DELETE FROM session_list_index", vec![]).await.unwrap();
    assert!(state.inner.store.list_sessions().await.is_err());
    assert_eq!(crate::v2_routes::session_family_ids(&state, "root").await, reference_closure(&sessions, "root"));
    assert_eq!(crate::v2_routes::session_family_ids(&state, "missing").await, reference_closure(&sessions, "missing"));
    state.shutdown().await.unwrap();
}

#[tokio::test]
async fn scoped_sse_headers_and_child_replay_do_not_require_unrelated_session_hydration() {
    use axum::{extract::{Query, State}, http::HeaderMap, response::IntoResponse};
    use futures::StreamExt;
    use neoism_agent_core::{event_type, EventPayload};
    use std::time::Duration;

    let state = AppState::open_database(std::env::temp_dir().join(format!(
        "sse-headers-{}.db", neoism_agent_core::new_session_id()
    ))).await.unwrap();
    for info in [session("root", None, None), session("child", Some("root"), None), session("bad", None, None)] {
        state.inner.store.insert_session(&info).await.unwrap();
    }
    state.inner.store.db.execute("UPDATE sessions SET info_json = '{}' WHERE id = 'bad'", vec![]).await.unwrap();
    assert!(state.inner.store.list_sessions().await.is_err());
    let event = EventPayload::new(event_type::SESSION_UPDATED, json!({"sessionID": "child", "marker": "child-replay"}));
    state.inner.store.append_event(&event).await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), crate::v2_routes::v2_events(
        State(state.clone()), None, HeaderMap::new(),
        Query(crate::v2_routes::V2EventQuery { session_id: Some("root".into()), since: Some(0), ..Default::default() }),
    )).await.expect("headers must not require unrelated catalogue hydration").into_response();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(2), body.next()).await
        .expect("child replay must survive unrelated invalid SessionInfo").unwrap().unwrap();
    assert!(std::str::from_utf8(&chunk).unwrap().contains("child-replay"));
    // The pre-header subscription still owns live events, even before polling.
    state.publish_live(EventPayload::new(event_type::MESSAGE_PART_DELTA, json!({
        "sessionID": "child", "messageID": "m", "partID": "p", "field": "text", "delta": "child-live"
    })));
    let chunk = tokio::time::timeout(Duration::from_secs(2), body.next()).await.unwrap().unwrap().unwrap();
    assert!(std::str::from_utf8(&chunk).unwrap().contains("child-live"));
    drop(body);
    state.shutdown().await.unwrap();
}
