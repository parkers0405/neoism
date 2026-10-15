use super::*;

fn chat(id: &str, directory: &str, updated: u64) -> NeoismAgentSessionEntry {
    NeoismAgentSessionEntry::new(id, id, "")
        .with_directory(Some(directory.into()))
        .with_updated_ms(updated)
}

fn projects(panel: &NeoismAgentSidePanel) -> Vec<&str> {
    panel
        .sessions()
        .iter()
        .filter(|row| row.is_project())
        .filter_map(|row| row.directory.as_deref())
        .collect()
}

#[test]
fn dashboard_root_precedes_newer_projects_but_not_global_pins() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("old", "/host/home", 1),
        chat("new", "/new", 10),
        chat("middle", "/middle", 5),
        chat("pin", "/new", 20).with_pinned(true),
    ]);
    panel.set_dashboard_directory(Some("/host/home".into()));
    assert_eq!(projects(&panel), ["/host/home", "/new", "/middle"]);
    assert_eq!(panel.sessions()[0].id, "section:global:Pinned");
    assert_eq!(panel.sessions()[1].id, "pin");
    let root = panel
        .sessions()
        .iter()
        .find(|row| row.is_project())
        .unwrap();
    assert_eq!(root.title, "~");
    assert_eq!(root.directory.as_deref(), Some("/host/home"));
    assert!(!panel.sessions().iter().any(|row| row.title == "Recent"));
}

#[test]
fn scoped_catalog_is_unchanged_and_identity_survives_scope_changes() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/host/home".into()));
    panel.set_sessions(vec![
        chat("old", "/host/home", 1),
        chat("new", "/host/home", 10),
    ]);
    let before: Vec<_> = panel.sessions().iter().map(|row| row.id.clone()).collect();
    panel.set_dashboard_directory(Some("/host/home".into()));
    assert_eq!(
        before,
        panel
            .sessions()
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>()
    );
    assert!(panel.sessions().iter().any(|row| row.title == "Recent"));
    panel.set_catalog_directory(None);
    panel.set_sessions(vec![chat("old", "/host/home", 1), chat("new", "/new", 10)]);
    assert_eq!(projects(&panel), ["/host/home", "/new"]);
}

#[test]
fn unknown_empty_and_replaced_roots_do_not_create_groups_or_guess() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("old", "/host/home", 1), chat("new", "/new", 10)]);
    for root in [
        Some("/missing"),
        Some(""),
        Some(" "),
        None,
        Some("/"),
        Some("C:\\"),
    ] {
        panel.set_dashboard_directory(root.map(str::to_owned));
        assert_eq!(projects(&panel), ["/new", "/host/home"]);
        assert!(!panel.sessions().iter().any(|row| row.title == "~"));
    }
    panel.set_dashboard_directory(Some("/host/home".into()));
    panel.set_dashboard_directory(Some("/new".into()));
    assert_eq!(projects(&panel), ["/new", "/host/home"]);
    panel.set_dashboard_directory(None);
    assert_eq!(projects(&panel), ["/new", "/host/home"]);
}

#[test]
fn exact_saved_directory_identity_including_windows_and_root_paths() {
    for root in [
        "C:\\Users\\alice",
        "C:\\",
        "/",
        "/home/alice/./",
        "/project/~",
    ] {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_sessions(vec![
            chat("old", root, 1),
            chat("new", "/new", 10),
            chat("similar", "/home/alice", 5),
        ]);
        panel.set_dashboard_directory(Some(root.into()));
        assert_eq!(projects(&panel)[0], root);
        assert_eq!(
            panel
                .sessions()
                .iter()
                .find(|r| r.is_project())
                .unwrap()
                .title,
            "~"
        );
    }
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("old", "/project/~", 1), chat("new", "/new", 10)]);
    panel.set_dashboard_directory(Some("/different/~".into()));
    assert_eq!(projects(&panel), ["/new", "/project/~"]);
}

#[test]
fn late_metadata_keeps_scrolled_row_anchor_and_detail_view_state() {
    let mut panel = NeoismAgentSidePanel::default();
    let mut rows = vec![chat("old", "/host/home", 1)];
    for i in 0..30 {
        rows.push(chat(&format!("new-{i}"), "/new", 100 + i));
    }
    panel.set_sessions(rows);
    panel.last_panel_height_rows = 5;
    panel.scroll_px = panel.row_height() * 4.0 + 3.0;
    panel.scroll_top = 4;
    let before = panel.session_viewport_anchor().unwrap();
    panel.set_dashboard_directory(Some("/host/home".into()));
    assert_eq!(panel.session_viewport_anchor().unwrap(), before);
    panel.set_mode(SidePanelMode::Subagents);
    panel.scroll_px = 27.5;
    panel.scroll_top = 2;
    panel.selected = 1;
    panel.set_dashboard_directory(Some("/new".into()));
    assert_eq!(panel.scroll_px, 27.5);
    assert_eq!(panel.scroll_top, 2);
    assert_eq!(panel.selected, 1);
}

#[test]
fn paginated_discovery_and_requests_are_not_restarted_by_root_metadata() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_projects(vec![("/host/home".into(), 1), ("/new".into(), 10)]);
    let page = panel.project_pages.get_mut("/host/home").unwrap();
    page.loaded = true;
    page.next_cursor = Some("page-next".into());
    page.request = Some((Some("page-request".into()), 42));
    page.queued = true;
    panel.set_dashboard_directory(Some("/host/home".into()));
    assert_eq!(projects(&panel), ["/host/home", "/new"]);
    let page = panel.project_pages.get("/host/home").unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("page-next"));
    assert_eq!(page.request, Some((Some("page-request".into()), 42)));
    assert!(page.loaded && page.queued);
    // A verified root absent from discovery must not manufacture a folder.
    panel.set_dashboard_directory(Some("/missing".into()));
    assert_eq!(projects(&panel), ["/new", "/host/home"]);
}

#[test]
fn metadata_preserves_selection_top_cursor_and_singleflight_and_is_idempotent() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("old", "/host/home", 1), chat("new", "/new", 10)]);
    panel.selected = panel
        .sessions()
        .iter()
        .position(|row| row.id == "new")
        .unwrap();
    panel.session_next_cursor = Some("next".into());
    panel.session_requested_cursor = Some("in-flight".into());
    panel.session_page_loading = true;
    let generation = panel.next_session_request_generation();
    panel.collapsed_projects.insert("/host/home".into());
    panel.set_dashboard_directory(Some("/host/home".into()));
    assert_eq!(panel.sessions()[panel.selected].id, "new");
    assert_eq!(panel.scroll_px, 0.0);
    assert_eq!(panel.scroll_top, 0);
    assert_eq!(panel.session_next_cursor.as_deref(), Some("next"));
    assert_eq!(panel.session_requested_cursor.as_deref(), Some("in-flight"));
    assert!(panel.session_page_loading);
    assert!(panel.session_request_is_current(generation));
    assert!(panel.collapsed_projects.contains("/host/home"));
    // Deliberately fractional viewport and an impossible-for-rebuild title prove
    // the per-frame replay does not rebuild or normalize anything.
    panel.scroll_px = 17.5;
    panel.scroll_top = 3;
    panel.sessions[0].title = "sentinel".into();
    let selection = panel.selected;
    panel.set_dashboard_directory(Some("/host/home".into()));
    assert_eq!(panel.sessions[0].title, "sentinel");
    assert_eq!(panel.selected, selection);
    assert_eq!(panel.scroll_px, 17.5);
    assert_eq!(panel.scroll_top, 3);
    assert_eq!(panel.session_next_cursor.as_deref(), Some("next"));
    assert_eq!(panel.session_requested_cursor.as_deref(), Some("in-flight"));
    assert!(panel.session_request_is_current(generation));
}
