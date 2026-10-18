use super::*;

fn panel(count: usize) -> NeoismAgentSidePanel {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_last_panel_height_rows(4);
    panel.set_session_projects(
        (0..count)
            .map(|n| (format!("/p/{n:02}"), (count - n) as u64))
            .collect(),
    );
    panel
}
fn chat(id: &str, directory: &str) -> NeoismAgentSessionEntry {
    NeoismAgentSessionEntry::new(id, id, "").with_directory(Some(directory.into()))
}
fn select(panel: &mut NeoismAgentSidePanel, id: &str) {
    let index = panel.sessions.iter().position(|e| e.id == id).unwrap();
    panel.set_selected(index);
}
fn settle(
    panel: &mut NeoismAgentSidePanel,
    request: &(String, Option<String>, u64),
    cursor: Option<&str>,
) {
    panel.set_project_session_page(
        &request.0,
        request.1.as_deref(),
        vec![chat(&format!("chat:{}", request.0), &request.0)],
        cursor.map(str::to_owned),
        request.2,
    );
}

#[test]
fn all_project_pins_are_hydrated_before_pages_and_not_duplicated_in_folders() {
    let mut p = panel(2);
    p.set_pinned_sessions(vec![
        chat("old-pin", "/p/00")
            .with_pinned(true)
            .with_updated_ms(1),
        chat("other-pin", "/p/01")
            .with_pinned(true)
            .with_updated_ms(2),
        chat("not-pinned", "/p/00"),
    ]);
    assert_eq!(p.sessions[0].title, "Pinned");
    assert_eq!(p.scroll_top(), 0);
    assert_eq!(p.scroll_px, 0.0);
    assert_eq!(p.sessions[1].id, "other-pin");
    assert_eq!(p.sessions[2].id, "old-pin");
    assert!(p.sessions[3].is_project());
    assert_eq!(
        p.sessions.iter().filter(|row| row.id == "old-pin").count(),
        1
    );
    assert!(p.catalog_session("not-pinned").is_none());
    assert_eq!(
        p.sessions
            .iter()
            .filter(|row| row.is_header && row.title == "Recent")
            .count(),
        0
    );
    select(&mut p, "project:/p/00");
    p.toggle_selected_project();
    assert!(p.sessions.iter().any(|row| row.id == "old-pin"));
    p.upsert_session(chat("other-pin", "/p/01").with_updated_ms(3));
    assert_eq!(
        p.sessions
            .iter()
            .filter(|row| row.id == "other-pin")
            .count(),
        1
    );
    assert_eq!(p.sessions[1].id, "old-pin");
    p.set_pinned_sessions(Vec::new());
    assert!(p.catalog_session("old-pin").is_none());
    assert!(p.catalog_session("other-pin").is_some());
    assert!(!p
        .sessions
        .iter()
        .any(|row| row.is_header && row.title == "Pinned"));
    p.set_catalog_directory(Some("/p/00".into()));
    p.set_pinned_sessions(vec![chat("foreign-pin", "/p/01").with_pinned(true)]);
    assert!(p.catalog_session("foreign-pin").is_none());
    assert!(p.discovered_pinned_versions.is_empty());
}

#[test]
fn delayed_pin_snapshots_preserve_newer_live_pin_and_unpin_metadata() {
    let mut p = panel(1);
    let pin = chat("pin", "/p/00").with_pinned(true).with_updated_ms(1);
    p.set_pinned_sessions(vec![pin.clone()]);
    p.upsert_session(chat("pin", "/p/00").with_pinned(true).with_updated_ms(2));
    p.set_pinned_sessions(Vec::new());
    assert!(p.catalog_session("pin").unwrap().pinned);
    p.set_pinned_sessions(vec![pin.clone()]);
    assert_eq!(p.catalog_session("pin").unwrap().updated_ms, 2);
    p.upsert_session(chat("pin", "/p/00").with_updated_ms(3));
    p.set_pinned_sessions(vec![pin]);
    assert!(!p.catalog_session("pin").unwrap().pinned);
    p.set_pinned_sessions(Vec::new());
    assert_eq!(p.catalog_session("pin").unwrap().updated_ms, 3);
    p.remove_session("pin");
    p.set_pinned_sessions(vec![chat("pin", "/p/00")
        .with_pinned(true)
        .with_updated_ms(4)]);
    assert!(p.catalog_session("pin").is_none());
    assert!(!p
        .sessions
        .iter()
        .any(|row| row.is_header && row.title == "Pinned"));
}

#[test]
fn discovery_headers_are_independent_of_loaded_sessions_and_visible_work_is_bounded() {
    let mut p = panel(40);
    assert_eq!(p.sessions.iter().filter(|e| e.is_project()).count(), 40);
    assert!(p
        .sessions
        .iter()
        .filter(|e| e.is_project())
        .all(|e| e.time_label.is_empty()));
    let requests = p.begin_project_session_requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["/p/00", "/p/01", "/p/02"]
    );
    assert!(requests.iter().all(|r| r.1.is_none()));
    assert!(p.begin_project_session_requests().is_empty());
    // Scroll well beyond the first projects while their replies are pending.
    p.scroll_top = p
        .sessions
        .iter()
        .position(|e| e.id == "project:/p/30")
        .unwrap();
    p.scroll_px = p.scroll_top as f32 * p.row_height();
    settle(&mut p, &requests[0], None);
    let next = p.begin_project_session_requests();
    assert_eq!(next.len(), 1);
    assert!(next[0].0.as_str() >= "/p/29");
    assert!(next[0].0.as_str() <= "/p/32");
    // Finishing the sibling requests doesn't invalidate the visible request.
    settle(&mut p, &requests[1], None);
    settle(&mut p, &requests[2], None);
    settle(&mut p, &next[0], None);
    assert!(p.catalog_session(&format!("chat:{}", next[0].0)).is_some());
}

#[test]
fn pin_sections_preserve_selection_viewport_and_the_trailing_page_action() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    p.set_project_session_page(
        "/p/00",
        None,
        (0..5)
            .map(|n| chat(&format!("chat-{n}"), "/p/00"))
            .collect(),
        Some("next".into()),
        first.2,
    );
    p.set_last_panel_height_rows(2);
    select(&mut p, "chat-3");
    let top = p
        .sessions
        .iter()
        .position(|row| row.id == "chat-2")
        .unwrap();
    p.scroll_top = top;
    p.scroll_px = top as f32 * p.row_height() + 2.0;
    let anchor = p.session_viewport_anchor();
    p.upsert_session(chat("chat-3", "/p/00").with_pinned(true));
    assert_eq!(p.selected_session().unwrap().id, "chat-3");
    assert_eq!(p.session_viewport_anchor(), anchor);
    assert_eq!(
        p.sessions
            .iter()
            .filter(|row| row.is_header && !row.is_selectable())
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Pinned"]
    );
    assert!(p
        .sessions
        .iter()
        .filter(|row| row.is_header && !row.is_selectable())
        .all(|row| row.directory.is_none() && row.depth == 0));
    assert_eq!(p.sessions.iter().filter(|row| !row.is_header).count(), 5);
    assert!(p.sessions.last().unwrap().is_project_page());
    p.upsert_session(chat("chat-3", "/p/00").with_pinned(false));
    assert_eq!(p.selected_session().unwrap().id, "chat-3");
    assert_eq!(p.session_viewport_anchor(), anchor);
    assert!(!p
        .sessions
        .iter()
        .any(|row| row.is_header && row.title == "Pinned"));
    select(&mut p, "project-page:/p/00");
    assert!(p.toggle_selected_project());
    assert_eq!(
        p.begin_project_session_requests()[0].1.as_deref(),
        Some("next")
    );
}

#[test]
fn paging_pointer_only_marks_actionable_show_more_and_retry_rows() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    settle(&mut p, &first, Some("next"));
    let rect = [0., 0., 300., 300.];
    let row_height = p.row_height();
    p.set_last_panel_rect(rect);
    p.set_row_hit_rect(rect, row_height);
    let row = p
        .sessions
        .iter()
        .position(|row| row.is_project_page())
        .unwrap();
    let y = (row as f32 + 0.5) * row_height;
    assert!(p.project_page_action_at(100., y));
    assert!(!p.project_page_action_at(400., y));
    assert!(!p.project_page_action_at(100., row_height * 0.5));
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    assert!(!p.project_page_action_at(100., y));
    let next = p.begin_project_session_requests().remove(0);
    assert!(!p.project_page_action_at(100., y));
    p.settle_project_session_error(&next.0, next.2, "offline".into());
    assert!(p.project_page_action_at(100., y));
    p.invalidate_session_catalog();
    assert!(!p.project_page_action_at(100., y));
}

#[test]
fn header_focus_walks_pins_folders_chats_and_show_more_in_both_directions() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    settle(&mut p, &first, Some("page2"));
    p.set_pinned_sessions(vec![chat("pinned", "/p/00").with_pinned(true)]);
    p.set_navigation_header_enabled(true);
    p.select_new_chat();
    p.select_next();
    assert_eq!(p.selected_header_action(), Some(CatalogHeaderAction::Search));
    p.select_next();
    assert!(p.projects_selected());
    let expected: Vec<_> = p
        .sessions
        .iter()
        .filter(|row| row.is_selectable())
        .map(|row| row.stable_identity().to_owned())
        .collect();
    assert!(expected.iter().any(|id| id == "pinned"));
    assert!(expected.iter().any(|id| id == "project:/p/00"));
    assert!(expected.iter().any(|id| id == "project-page:/p/00"));
    for identity in &expected {
        p.select_next();
        assert!(!p.projects_selected());
        assert!(!p.search_focused());
        assert_eq!(p.selected_row().unwrap().stable_identity(), identity);
    }
    for identity in expected.iter().rev().skip(1) {
        p.select_prev();
        assert_eq!(p.selected_row().unwrap().stable_identity(), identity);
    }
    p.select_prev();
    assert!(p.projects_selected());
    assert!(!p.toggle_selected_project());
    p.select_prev();
    assert_eq!(p.selected_header_action(), Some(CatalogHeaderAction::Search));
    p.select_prev();
    assert!(p.new_chat_selected());
}

#[test]
fn show_more_requires_activation_and_claims_cursor_singleflight() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    settle(&mut p, &first, Some("page2"));
    assert!(p.begin_project_session_requests().is_empty());
    p.session_next_cursor = Some("old-global-tail".into());
    assert!(p.begin_session_page_near_end().is_none());
    select(&mut p, "project:/p/00");
    p.select_next(); // chat
    p.select_next(); // paging control is keyboard-selectable
    assert!(p.sessions[p.selected].is_project_page());
    assert!(p.selected_session().is_none());
    assert!(p.selected_catalog_row().is_none());
    assert!(p.sessions[p.selected].is_selectable());
    assert!(p.toggle_selected_project());
    assert_eq!(p.sessions[p.selected].title, "Loading…");
    let second = p.begin_project_session_requests();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].1.as_deref(), Some("page2"));
    assert!(p.toggle_selected_project()); // repeated keyboard / mouse activation
    assert!(p.begin_project_session_requests().is_empty());
    // A mismatched cursor, even with the right generation, cannot settle a claim.
    p.set_project_session_page(
        "/p/00",
        None,
        vec![chat("wrong", "/p/00")],
        None,
        second[0].2,
    );
    assert!(p.catalog_session("wrong").is_none());
    assert!(p.project_pages["/p/00"].request.is_some());
    p.set_project_session_page(
        "/p/00",
        Some("page2"),
        vec![chat("second", "/p/00")],
        None,
        second[0].2,
    );
    assert!(p.catalog_session("second").is_some());
    assert!(!p.sessions.iter().any(|e| e.is_project_page()));
}

#[test]
fn collapse_prevents_initial_requests_and_does_not_reopen_on_reply() {
    let mut p = panel(2);
    select(&mut p, "project:/p/00");
    assert!(p.toggle_selected_project());
    let first = p.begin_project_session_requests();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "/p/01");
    select(&mut p, "project:/p/01");
    assert!(p.toggle_selected_project());
    settle(&mut p, &first[0], Some("more"));
    assert!(p
        .sessions
        .iter()
        .all(|e| e.is_project() || e.id == "section:global:Recent"));
    assert!(p.begin_project_session_requests().is_empty());
    select(&mut p, "project:/p/00");
    p.toggle_selected_project();
    assert_eq!(p.begin_project_session_requests()[0].0, "/p/00");
}

#[test]
fn errors_require_explicit_retry_for_both_initial_and_continuation_pages() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    p.settle_project_session_error(&first.0, first.2, "offline".into());
    assert!(p.begin_project_session_requests().is_empty());
    assert!(p.sessions.iter().any(|e| e.title == "Retry — offline"));
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    let retry = p.begin_project_session_requests().remove(0);
    assert!(retry.2 > first.2);
    assert!(retry.1.is_none());
    // Late errors from the failed request cannot steal the retry slot.
    p.settle_project_session_error(&first.0, first.2, "stale".into());
    assert_eq!(
        p.project_pages["/p/00"].request.as_ref().unwrap().1,
        retry.2
    );
    settle(&mut p, &retry, Some("next"));
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    let second = p.begin_project_session_requests().remove(0);
    p.settle_project_session_error(&second.0, second.2, "timeout".into());
    assert!(p.catalog_session("chat:/p/00").is_some());
    assert!(p.begin_project_session_requests().is_empty());
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    assert_eq!(
        p.begin_project_session_requests()[0].1.as_deref(),
        Some("next")
    );
}

#[test]
fn refresh_scope_and_catalog_invalidation_reject_old_replies() {
    let mut p = panel(1);
    let old = p.begin_project_session_requests().remove(0);
    p.set_session_projects(Vec::new()); // removal retires its old worker
    p.set_session_projects(vec![("/p/00".into(), 99)]);
    let fresh = p.begin_project_session_requests().remove(0);
    assert_ne!(fresh.2, old.2);
    settle(&mut p, &old, Some("bad"));
    p.settle_project_session_error(&old.0, old.2, "stale".into());
    assert!(p.all_sessions.is_empty());
    assert!(p.project_pages["/p/00"].request.is_some());
    // A global refresh generation must not invalidate a project request.
    p.next_session_request_generation();
    settle(&mut p, &fresh, None);
    assert!(!p.all_sessions.is_empty());
    p.invalidate_session_catalog();
    settle(&mut p, &fresh, None);
    assert!(p.sessions.is_empty());
    p.set_session_projects(vec![("/p/00".into(), 100)]);
    let newer = p.begin_project_session_requests().remove(0);
    p.set_catalog_directory(Some("/p/00".into()));
    assert!(!p.project_pagination_enabled());
    settle(&mut p, &newer, None);
    assert!(p.sessions.is_empty());
    // A project discovery callback must not change scoped mode.
    p.set_session_projects(vec![("/p/00".into(), 100)]);
    assert!(!p.project_pagination_enabled());
    p.set_session_page(
        vec![chat("scoped", "/p/00")],
        None,
        Some("flat-next".into()),
    );
    assert_eq!(
        p.begin_session_page_near_end().as_deref(),
        Some("flat-next")
    );
}

#[test]
fn rediscovery_preserves_inflight_claims_history_cursor_and_worker_budget() {
    let mut p = panel(4);
    let initial = p.begin_project_session_requests();
    assert_eq!(initial.len(), 3);
    p.set_session_projects((0..4).map(|n| (format!("/p/{n:02}"), 100)).collect());
    assert!(p.begin_project_session_requests().is_empty());
    for request in &initial {
        settle(&mut p, request, Some("more"));
    }
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    let continuation = p
        .begin_project_session_requests()
        .into_iter()
        .find(|r| r.0 == "/p/00")
        .unwrap();
    p.set_session_projects((0..4).map(|n| (format!("/p/{n:02}"), 200)).collect());
    assert!(p.catalog_session("chat:/p/00").is_some());
    assert_eq!(
        p.project_pages["/p/00"].next_cursor.as_deref(),
        Some("more")
    );
    assert_eq!(
        p.project_pages["/p/00"].request.as_ref().unwrap().1,
        continuation.2
    );
    assert!(!p
        .begin_project_session_requests()
        .iter()
        .any(|r| r.0 == "/p/00"));
    settle(&mut p, &continuation, None);
    assert!(p.project_pages["/p/00"].next_cursor.is_none());

    // Three removed but still-running workers prevent three replacement
    // projects from creating six simultaneous requests.
    let mut p = panel(3);
    let old = p.begin_project_session_requests();
    p.set_session_projects(vec![
        ("/new/a".into(), 1),
        ("/new/b".into(), 1),
        ("/new/c".into(), 1),
    ]);
    assert!(p.begin_project_session_requests().is_empty());
    p.settle_project_session_error(&old[0].0, old[0].2, "late failure".into());
    assert_eq!(p.begin_project_session_requests().len(), 1);
    settle(&mut p, &old[1], None);
    assert!(p.catalog_session(&format!("chat:{}", old[1].0)).is_none());
    assert_eq!(p.begin_project_session_requests().len(), 1);
}

#[test]
fn search_hydration_does_not_expand_the_browsing_allotment() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    p.set_project_session_page(
        "/p/00",
        None,
        (0..5)
            .map(|n| chat(&format!("page-{n}"), "/p/00"))
            .collect(),
        Some("next".into()),
        first.2,
    );
    p.set_session_query("search".into());
    p.admit_search_sessions((0..50).map(|n| chat(&format!("search-{n}"), "/p/00")));
    assert_eq!(p.sessions.iter().filter(|e| !e.is_header).count(), 50);
    assert!(
        p.catalog_session("search-49").is_some(),
        "hydrated rows remain navigable by id"
    );
    p.upsert_session(chat("live", "/p/00"));
    p.upsert_session(chat("search-0", "/p/00")); // genuine live update promotes this hit
    p.clear_session_query();
    assert_eq!(p.sessions.iter().filter(|e| !e.is_header).count(), 7);
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    let next = p.begin_project_session_requests().remove(0);
    p.set_project_session_page(
        "/p/00",
        next.1.as_deref(),
        vec![chat("search-49", "/p/00")],
        None,
        next.2,
    );
    assert_eq!(p.sessions.iter().filter(|e| !e.is_header).count(), 8);
}

#[test]
fn blank_directory_discovery_is_a_pageable_other_chats_group() {
    let mut p = NeoismAgentSidePanel::default();
    p.set_session_projects(vec![(String::new(), 123)]);
    assert!(p.sessions[0].is_project());
    assert_eq!(p.sessions[0].title, "Other chats");
    let first = p.begin_project_session_requests().remove(0);
    assert_eq!(first.0, "");
    p.set_project_session_page(
        "",
        None,
        vec![chat("unknown-dir", "")],
        Some("next".into()),
        first.2,
    );
    assert!(p.sessions.iter().any(|e| e.id == "unknown-dir"));
    select(&mut p, "project-page:");
    assert!(p.toggle_selected_project());
    assert_eq!(
        p.begin_project_session_requests()[0].1.as_deref(),
        Some("next")
    );
    select(&mut p, "project:");
    assert!(p.toggle_selected_project());
    assert!(p.sessions.iter().all(|row| row.is_project()));
}

#[test]
fn search_does_not_fetch_project_pages_and_live_events_remain_visible() {
    let mut p = panel(1);
    let first = p.begin_project_session_requests().remove(0);
    settle(&mut p, &first, Some("more"));
    p.upsert_session(chat("live", "/p/00"));
    p.set_session_query("live".into());
    assert!(p.begin_project_session_requests().is_empty());
    assert_eq!(
        p.sessions
            .iter()
            .filter(|e| !e.is_header)
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["live"]
    );
    assert!(!p.sessions.iter().any(|e| e.is_project_page()));
    p.clear_session_query();
    p.remove_session("live");
    assert!(p.catalog_session("live").is_none());
    select(&mut p, "project-page:/p/00");
    p.toggle_selected_project();
    let second = p.begin_project_session_requests().remove(0);
    p.set_project_session_page(
        "/p/00",
        second.1.as_deref(),
        vec![chat("live", "/p/00")],
        None,
        second.2,
    );
    assert!(
        p.catalog_session("live").is_none(),
        "stale snapshot must not revive deletion"
    );
}
