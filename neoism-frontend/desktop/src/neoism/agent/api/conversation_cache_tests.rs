use super::*;
use crate::neoism::agent::pane::{
    CachedAgentSession, NeoismAgentBackgroundUpdate, NeoismAgentMessage, NeoismAgentPane,
};
use std::sync::Arc;

fn key(session: &str) -> Identity {
    Identity {
        server: "http://history.test/agent/workspaces/a".into(),
        credential: Some("secret".into()),
        session: session.into(),
        generation: 0,
    }
}

fn page() -> SessionMessagePage {
    SessionMessagePage {
        blocks: vec![
            NeoismAgentMessage::assistant("a much longer stale reply").with_id("a"),
            NeoismAgentMessage::assistant("deleted reply").with_id("b"),
        ],
        raw_count: 100,
        oldest_cursor: Some("cursor-oldest-stored-message".into()),
    }
}

fn wire_page(text: &str) -> String {
    serde_json::json!({"items": [{
        "info": {"id": "message", "role": "assistant"},
        "parts": [{"id": "part", "type": "text", "text": text}]
    }]})
    .to_string()
}

fn insert_wire(
    cache: &mut Cache,
    identity: Identity,
    json: String,
    limit: usize,
    now: Instant,
) {
    if let Some((page, bytes)) = prepare_snapshot(&identity, &json) {
        cache.insert(identity, page, limit, bytes, now);
    } else {
        cache.forget(&identity);
    }
}

#[test]
fn conversation_cache_bounds_ttl_and_replacement_are_strict() {
    let now = Instant::now();
    let mut cache = Cache::default();
    for i in 0..MAX_ENTRIES + 3 {
        insert_wire(
            &mut cache,
            key(&i.to_string()),
            wire_page("answer"),
            100,
            now,
        );
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert!(cache.snapshot(&key("0"), now).is_none());
    assert!(cache.snapshot(&key("3"), now).is_some());
    insert_wire(&mut cache, key("3"), wire_page("replacement"), 80, now);
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    let (page, limit) = cache.snapshot(&key("3"), now).unwrap();
    assert_eq!(limit, 80);
    assert!(page.blocks.iter().any(|block| block.text == "replacement"));
    // Reads do not extend the display TTL.
    assert!(cache
        .snapshot(&key("3"), now + TTL - Duration::from_millis(1))
        .is_some());
    assert!(cache.snapshot(&key("3"), now + TTL).is_none());
    assert_eq!(cache.bytes, 0);

    for i in 0..MAX_ENTRIES {
        insert_wire(
            &mut cache,
            key(&i.to_string()),
            wire_page(&"x".repeat(700 * 1024)),
            100,
            now,
        );
        assert!(cache.bytes <= MAX_BYTES);
        assert!(cache.entries.len() <= MAX_ENTRIES);
        assert_eq!(
            cache.bytes,
            cache.entries.iter().map(|entry| entry.bytes).sum::<usize>()
        );
    }
    assert!(!cache.entries.is_empty());
    assert!(cache.entries.len() < MAX_ENTRIES);
    insert_wire(
        &mut cache,
        key("huge"),
        wire_page(&"x".repeat(MAX_ENTRY_BYTES + 1)),
        100,
        now,
    );
    assert!(cache.snapshot(&key("huge"), now).is_none());
}

#[test]
fn conversation_cache_scopes_auth_and_rejects_inflight_aba() {
    let mut cache = Cache::default();
    let now = Instant::now();
    let old = key("session");
    insert_wire(&mut cache, old.clone(), wire_page("answer"), 100, now);
    for other in [
        Identity {
            server: "http://other.test".into(),
            ..old.clone()
        },
        Identity {
            credential: Some("other-token".into()),
            ..old.clone()
        },
        Identity {
            credential: None,
            ..old.clone()
        },
        Identity {
            session: "other-session".into(),
            ..old.clone()
        },
    ] {
        assert!(cache.snapshot(&other, now).is_none());
    }
    cache.credentials_changed();
    cache.credentials_changed(); // A -> B -> A still changes the generation.
    insert_wire(
        &mut cache,
        old.clone(),
        wire_page("stale inflight response"),
        100,
        now,
    );
    assert!(cache.entries.is_empty());
    let current = Identity {
        generation: cache.generation,
        ..old
    };
    insert_wire(&mut cache, current.clone(), wire_page("current"), 80, now);
    assert!(cache.snapshot(&current, now).is_some());
    cache.forget(&current); // Failed revalidation/auth must evict previews.
    assert!(cache.snapshot(&current, now).is_none());
}

#[test]
fn conversation_cache_snapshots_share_the_predecoded_page() {
    let now = Instant::now();
    let mut cache = Cache::default();
    let identity = key("shared");
    let (decoded, bytes) =
        prepare_snapshot(&identity, &wire_page("warm answer")).unwrap();
    cache.insert(identity.clone(), Arc::clone(&decoded), 73, bytes, now);
    for _ in 0..10 {
        let (preview, limit) = cache.snapshot(&identity, now).unwrap();
        assert!(
            Arc::ptr_eq(&decoded, &preview),
            "peek must not decode or deep-clone blocks"
        );
        assert_eq!(limit, 73);
    }
    cache.forget(&identity);
    assert!(cache.snapshot(&identity, now).is_none());
    assert_eq!(
        decoded.blocks[0].text, "warm answer",
        "existing display owners remain valid"
    );
}

#[test]
fn conversation_cache_bounds_decoded_image_payloads_not_only_wire_bytes() {
    let identity = key("images");
    let json = serde_json::json!({"items": [{
        "info": {"id": "image-message", "role": "assistant"},
        "parts": [{"id": "image", "type": "file", "mime": "image/png",
            "filename": "generated.png", "url": format!("data:image/png;base64,{}", "A".repeat(600 * 1024))}]
    }]}).to_string();
    assert!(json.len() < MAX_ENTRY_BYTES);
    let page = decode_snapshot(&json).unwrap();
    assert_eq!(page.blocks[0].images.len(), 1);
    assert!(resident_bytes(&identity, &page) > MAX_ENTRY_BYTES);
    assert!(prepare_snapshot(&identity, &json).is_none());

    let mut cache = Cache::default();
    let now = Instant::now();
    insert_wire(
        &mut cache,
        identity.clone(),
        wire_page("previous"),
        100,
        now,
    );
    insert_wire(&mut cache, identity.clone(), json, 100, now);
    assert!(
        cache.snapshot(&identity, now).is_none(),
        "an oversized replacement evicts stale display data"
    );
    assert_eq!(cache.bytes, 0);
}

#[test]
fn conversation_cache_accounts_composition_children_and_reserved_capacity() {
    let identity = key("composition");
    let json = serde_json::json!({"items": [{
        "info": {"id": "message", "role": "assistant"},
        "parts": [{"id": "compose", "type": "tool", "tool": "compose",
            "state": {"status": "completed", "metadata": {"composition": {
                "toolCalls": [{"id": "call", "path": "neoism.read", "status": "completed",
                    "input": {"filePath": "example.rs"},
                    "presentation": {"tool": "read", "output": "retained child output"}}]
            }}}}]
    }]})
    .to_string();
    let mut page = decode_snapshot(&json).unwrap();
    assert_eq!(page.blocks[0].composition_children.len(), 1);
    let baseline = resident_bytes(&identity, &page);
    let child = &mut page.blocks[0].composition_children[0];
    child
        .images
        .push(neoism_ui::panels::agent_pane::state::NeoismAgentImage {
            filename: "child.png".into(),
            mime: "image/png".into(),
            url: format!("data:image/png;base64,{}", "B".repeat(600 * 1024)).into(),
        });
    assert!(resident_bytes(&identity, &page) > baseline + 600 * 1024);
    assert!(resident_bytes(&identity, &page) > MAX_ENTRY_BYTES);
    page.blocks[0].composition_children.clear();
    let before_reserve = resident_bytes(&identity, &page);
    page.blocks[0].text.reserve(MAX_ENTRY_BYTES);
    assert!(resident_bytes(&identity, &page) >= before_reserve + MAX_ENTRY_BYTES);
}

#[test]
fn conversation_cache_wire_artifact_reference_is_not_hydrated() {
    let json = serde_json::json!({"items": [{
        "info": {"id": "message", "role": "assistant"},
        "parts": [{"id": "image", "type": "file", "mime": "image/png",
            "filename": "generated.png", "url": "/v2/artifacts/art_1/content"}]
    }]})
    .to_string();
    let (page, _) = prepare_snapshot(&key("artifact"), &json).unwrap();
    assert_eq!(
        page.blocks[0].images[0].url.as_str(),
        "/v2/artifacts/art_1/content"
    );
}

#[test]
fn conversation_cache_decodes_pagination_without_inventing_block_count() {
    let json = serde_json::json!({"items": [
        {"info": {"id": "new", "role": "assistant"}, "parts": [
            {"id": "part-new", "type": "text", "text": "answer"}
        ]},
        {"info": {"id": "old", "role": "assistant"}, "parts": []}
    ]})
    .to_string();
    let page = decode_snapshot(&json).unwrap();
    assert_eq!(page.raw_count, 2);
    assert_eq!(page.oldest_cursor.as_deref(), Some("old"));
    assert!(page.blocks.iter().any(|message| message.text == "answer"));
    assert!(decode_snapshot("{}").is_none());
}

#[test]
fn conversation_cache_seed_is_non_authoritative_and_does_not_replace_live_panes() {
    let mut pane = NeoismAgentPane::default();
    pane.session_cache
        .insert("session".into(), CachedAgentSession::live_only());
    pane.apply_initial_history_preview("session", Arc::new(page()), 100);
    let cached = &pane.session_cache["session"];
    assert!(!cached.hydrated);
    assert_eq!(cached.messages.len(), 2);
    assert!(cached.timeline_history.has_older);
    assert_eq!(
        cached.timeline_history.oldest_loaded_cursor.as_deref(),
        Some("cursor-oldest-stored-message")
    );
    assert_eq!(
        cached.initial_history_preview.as_ref().unwrap().raw_count,
        100
    );
    pane.runtime_hydrated_sessions.insert("session".into());
    pane.activate_cached_session("session");
    assert_eq!(pane.messages.len(), 2);
    pane.cache_current_session(false);
    assert!(
        !pane.session_cache["session"].hydrated,
        "parking a preview cannot promote it to authority"
    );

    let mut live = CachedAgentSession::live_only();
    live.messages.push(NeoismAgentMessage::assistant("live"));
    pane.session_cache.insert("live".into(), live);
    pane.apply_initial_history_preview("live", Arc::new(page()), 100);
    assert_eq!(pane.session_cache["live"].messages[0].text, "live");
    assert!(pane.session_cache["live"].initial_history_preview.is_none());

    let mut hydrated = CachedAgentSession::live_only();
    hydrated.hydrated = true;
    pane.session_cache.insert("hydrated".into(), hydrated);
    pane.apply_initial_history_preview("hydrated", Arc::new(page()), 100);
    assert!(pane.session_cache["hydrated"].messages.is_empty());

    let mut pending = CachedAgentSession::live_only();
    pending.pending_user_prompts.push("unsent echo".into());
    pane.session_cache.insert("pending".into(), pending);
    pane.apply_initial_history_preview("pending", Arc::new(page()), 100);
    assert!(pane.session_cache["pending"].messages.is_empty());
}

#[test]
fn conversation_cache_authoritative_preload_removes_preview_but_keeps_new_live_parts() {
    for active in [false, true] {
        let mut pane = NeoismAgentPane::default();
        pane.session_cache
            .insert("session".into(), CachedAgentSession::live_only());
        pane.apply_initial_history_preview("session", Arc::new(page()), 100);
        if active {
            pane.runtime_hydrated_sessions.insert("session".into());
            pane.activate_cached_session("session");
            pane.model_context_limit = Some(100_000);
            pane.messages
                .push(NeoismAgentMessage::assistant("new live part").with_id("live"));
        } else {
            pane.session_cache
                .get_mut("session")
                .unwrap()
                .messages
                .push(NeoismAgentMessage::assistant("new live part").with_id("live"));
        }
        pane.background_sender()
            .send(NeoismAgentBackgroundUpdate::SessionPreloaded {
                session_id: "session".into(),
                raw_count: 1,
                requested_limit: 32,
                messages: vec![NeoismAgentMessage::assistant("short").with_id("a")],
                oldest_cursor: Some("fresh-cursor".into()),
            })
            .unwrap();
        pane.drain_background_updates();
        let messages = if active {
            &pane.messages
        } else {
            &pane.session_cache["session"].messages
        };
        assert!(messages
            .iter()
            .any(|message| message.id == "a" && message.text == "short"));
        assert!(!messages.iter().any(|message| message.id == "b"));
        assert!(messages.iter().any(|message| message.id == "live"));
        if active {
            assert!(pane.initial_history_preview.is_none());
        } else {
            assert!(pane.session_cache["session"]
                .initial_history_preview
                .is_none());
            assert!(pane.session_cache["session"].hydrated);
        }
    }
}

#[test]
fn conversation_cache_preview_diff_preserves_sse_edits_and_duplicate_ids() {
    let preview = Arc::new(page());
    let mut messages = preview.blocks.clone();
    messages[0].text = "new SSE edit".into();
    messages.push(NeoismAgentMessage::assistant("another live part").with_id("b"));
    discard_unchanged_preview(&mut messages, Some(preview));
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "new SSE edit");
    assert_eq!(messages[1].text, "another live part");
}
