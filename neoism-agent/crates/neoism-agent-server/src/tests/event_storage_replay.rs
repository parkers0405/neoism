use super::*;

#[tokio::test]
async fn compressed_replay_crosses_byte_and_row_pages_before_live_family_baseline() {
    let path = std::env::temp_dir().join(format!(
        "neoism-compressed-replay-{}.db",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root = neoism_agent_core::new_session_id();
    let child = neoism_agent_core::new_session_id();
    let foreign = neoism_agent_core::new_session_id();
    let mut child_info = store_test_session(&child, now_millis());
    child_info.parent_id = Some(root.clone());
    for info in [
        store_test_session(&root, now_millis()),
        child_info,
        store_test_session(&foreign, now_millis()),
    ] {
        state.inner.store.insert_session(&info).await.unwrap();
    }

    // The first two large decoded records cannot share an 8 MiB replay page.
    // Remaining records also cross the internal 128-row page boundary.
    for marker in 0..300u64 {
        let session = if marker % 5 == 0 {
            &foreign
        } else if marker % 2 == 0 {
            &root
        } else {
            &child
        };
        let bytes = if matches!(marker, 4 | 9) {
            5 * 1024 * 1024
        } else {
            4096
        };
        let mut event = EventPayload::new(
            "fixture.history",
            json!({
                "sessionID": session,
                "journalMarker": marker,
                "snapshot": "x".repeat(bytes),
            }),
        );
        state.allocate_event_sequence(&mut event);
        state.inner.store.append_event(&event).await.unwrap();
    }

    state.publish(EventPayload::new(event_type::MESSAGE_UPDATED, json!({
        "sessionID": root,
        "info": {"id": "live-codec-message", "sessionId": root, "role": "assistant", "time": {"created": 1}}
    })));
    state.publish(EventPayload::new(event_type::MESSAGE_PART_UPDATED, json!({
        "sessionID": root,
        "part": {"id": "live-codec-part", "sessionId": root, "messageId": "live-codec-message", "type": "text", "text": "", "time": {"start": 1}}
    })));
    let delta = |text: &str| EventPayload::new(event_type::MESSAGE_PART_DELTA, json!({
        "sessionID": root, "messageID": "live-codec-message", "partID": "live-codec-part",
        "partType": "text", "field": "text", "delta": text,
    }));
    state.publish_live(delta("before join "));
    let response = app(state.clone()).oneshot(
        Request::builder()
            .uri(format!("/v2/events?sessionId={root}&tail=true&limit=5000"))
            .header("last-event-id", "0")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    state.publish_live(delta("after join"));

    let mut body = response.into_body().into_data_stream();
    let mut frames = String::new();
    let live_tail = b"\"delta\":\"after join\"";
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = body.next().await.expect("stream must remain live").unwrap();
            // Search the new bytes and a short overlap for split chunks. The
            // complete history contains multi-megabyte records; rescanning it
            // on every frame makes this test quadratic in the replay size.
            let search_start = frames.len().saturating_sub(live_tail.len() - 1);
            frames.push_str(std::str::from_utf8(&frame).unwrap());
            if frames.as_bytes()[search_start..]
                .windows(live_tail.len())
                .any(|bytes| bytes == live_tail)
            {
                break;
            }
        }
    }).await.expect("all replay pages, baseline and live tail must arrive");
    let markers = frames.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| value["data"]["journalMarker"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(markers, (0..300).filter(|marker| marker % 5 != 0).collect::<Vec<_>>());
    assert_eq!(frames.matches("before join ").count(), 1);
    assert_eq!(frames.matches("\"delta\":\"after join\"").count(), 1);
    assert!(frames.rfind("journalMarker").unwrap() < frames.find("before join ").unwrap());
    assert!(frames.find("before join ").unwrap() < frames.find("\"delta\":\"after join\"").unwrap());
    drop(body);
    state.shutdown().await.unwrap();
    drop(state);
    cleanup_sqlite_files(&path);
}
