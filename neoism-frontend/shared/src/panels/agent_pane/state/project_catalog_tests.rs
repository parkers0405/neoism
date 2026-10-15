use super::*;

fn chat(
    id: &str,
    directory: &str,
    updated: u64,
    pinned: bool,
) -> NeoismAgentSessionEntry {
    NeoismAgentSessionEntry::new(id, id, "")
        .with_directory(Some(directory.into()))
        .with_updated_ms(updated)
        .with_pinned(pinned)
}

#[test]
fn project_scope_shows_only_its_chats_without_project_folder_indentation() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/a".into()));
    panel.set_sessions(vec![
        chat("pin", "/work/a", 1, true),
        chat("recent", "/work/a", 3, false),
        chat("foreign", "/work/b", 4, false),
        chat("nested", "/work/a/nested", 5, false),
    ]);
    assert!(!panel.sessions().iter().any(|row| row.is_project()));
    assert_eq!(
        panel
            .sessions()
            .iter()
            .filter(|row| !row.is_header)
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["pin", "recent"]
    );
    assert!(panel
        .sessions()
        .iter()
        .filter(|row| !row.is_header)
        .all(|row| row.depth == 0));
    assert_eq!(
        panel
            .sessions()
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Pinned", "pin", "Recent", "recent"]
    );
    assert!(panel
        .sessions()
        .iter()
        .filter(|row| row.is_header)
        .all(|row| !row.is_selectable() && row.directory.is_none()));
}

#[test]
fn workspace_scope_change_invalidates_pages_and_switches_grouping() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    let generation = panel.next_session_request_generation();
    assert!(panel.set_catalog_directory(Some("/work/a".into())));
    assert!(!panel.session_request_is_current(generation));
    assert!(panel.sessions().is_empty());
    let scoped_generation = panel.next_session_request_generation();
    assert!(!panel.set_catalog_directory(Some("/work/a".into())));
    assert!(panel.session_request_is_current(scoped_generation));
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    assert!(!panel.sessions().iter().any(|row| row.is_project()));
    panel.set_catalog_directory(None);
    panel.set_sessions(vec![
        chat("a", "/work/a", 1, false),
        chat("b", "/work/b", 2, false),
    ]);
    assert_eq!(
        panel
            .sessions()
            .iter()
            .filter(|row| row.is_project())
            .count(),
        2
    );
}

#[test]
fn scoped_catalog_rejects_foreign_search_results_and_live_metadata() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/a".into()));
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    panel.upsert_session(chat("foreign", "/work/b", 2, false));
    panel.admit_search_sessions(vec![chat("foreign-search", "/work/b", 3, false)]);
    panel.reconcile_session_head(vec![chat("foreign-page", "/work/b", 4, false)], None);
    assert_eq!(panel.all_sessions.len(), 1);
    assert!(panel.catalog_session("foreign").is_none());
    assert!(panel.catalog_session("a").is_some());
}

#[test]
fn moving_a_chat_out_of_the_scoped_project_removes_its_old_row() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/a".into()));
    panel.set_sessions(vec![chat("moved", "/work/a", 1, false)]);
    panel.upsert_session(chat("moved", "/work/b", 2, false));
    panel.admit_search_sessions(vec![chat("moved", "/work/a", 1, false)]);
    assert!(panel.sessions().is_empty());
    panel.upsert_session(chat("moved", "/work/a", 3, false));
    assert!(panel.catalog_session("moved").is_some());
}

#[test]
fn project_search_does_not_match_every_chat_through_its_directory_name() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/neoism".into()));
    panel.set_sessions(vec![chat("fix", "/work/neoism", 1, false)]);
    panel.set_session_query("neoism".into());
    assert!(panel.sessions().is_empty());
    panel.set_session_query("fix".into());
    assert_eq!(
        panel.sessions().iter().filter(|row| !row.is_header).count(),
        1
    );
}

#[test]
fn endpoint_invalidation_preserves_the_workspace_catalog_scope() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/a".into()));
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    panel.invalidate_session_catalog();
    assert_eq!(panel.catalog_directory(), Some("/work/a"));
    assert!(panel.sessions().is_empty());
}

#[test]
fn enabling_navigation_each_frame_keeps_hit_targets_and_keyboard_focus() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_navigation_header_enabled(true);
    panel.set_header_action_rect(CatalogHeaderAction::Projects, [0.0, 0.0, 80.0, 30.0]);
    panel.select_projects();
    panel.set_navigation_header_enabled(true);
    assert_eq!(
        panel.header_action_at(10.0, 10.0),
        Some(CatalogHeaderAction::Projects)
    );
    assert!(panel.projects_selected());
}

#[test]
fn project_subsections_only_show_nonempty_sections_and_do_not_duplicate_pins() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_catalog_directory(Some("/work/a".into()));
    panel.set_sessions(vec![chat("pin", "/work/a", 1, true)]);
    assert_eq!(
        panel
            .sessions()
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Pinned", "pin"]
    );
    panel.upsert_session(chat("pin", "/work/a", 1, false));
    assert_eq!(
        panel
            .sessions()
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Recent", "pin"]
    );
    panel.set_session_query("pin".into());
    assert_eq!(
        panel
            .sessions()
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["pin"]
    );
    panel.clear_session_query();
    assert_eq!(
        panel
            .sessions()
            .iter()
            .filter(|row| row.id == "pin")
            .count(),
        1
    );
    panel.remove_session("pin");
    assert!(panel.sessions().is_empty());
}

#[test]
fn root_catalog_keeps_pins_global_and_orders_project_folders_by_activity() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("old-pin", "/work/a", 1, true),
        chat("new-a", "/work/a", 8, false),
        chat("new-b", "/work/b", 9, false),
    ]);
    let rows = panel.sessions();
    assert_eq!(
        rows.iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["Pinned", "old-pin", "b", "new-b", "a", "new-a"]
    );
    assert!(!rows[0].is_selectable());
    assert_eq!(
        rows.iter()
            .find(|r| r.id == "project:/work/b")
            .unwrap()
            .time_label,
        "1"
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.id == "project:/work/a")
            .unwrap()
            .time_label,
        "2"
    );
    assert_eq!(rows.iter().find(|r| r.id == "old-pin").unwrap().depth, 0);
    assert_eq!(rows.iter().find(|r| r.id == "new-b").unwrap().depth, 1);
}

#[test]
fn project_activation_collapses_without_becoming_a_session() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    panel.set_selected(0);
    assert!(panel.selected_session().is_none());
    assert!(panel.toggle_selected_project());
    assert!(panel.sessions().iter().all(|row| row.is_project()));
    assert_eq!(panel.selected_row().unwrap().id, "project:/work/a");
    assert!(!panel.project_expanded("/work/a"));
    panel.upsert_session(chat("new", "/work/a", 2, false));
    assert!(panel.sessions().iter().all(|row| row.is_project()));
    assert_eq!(panel.selected_row().unwrap().time_label, "2");
    assert!(panel.toggle_selected_project());
    assert_eq!(panel.sessions().len(), 3);
}

#[test]
fn search_reveals_collapsed_projects_and_matches_saved_directory() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("fix", "/work/neoism", 2, false),
        chat("other", "/work/site", 1, false),
    ]);
    panel.set_selected(0);
    panel.toggle_selected_project();
    panel.set_session_query("neoism".into());
    assert!(panel.project_expanded("/work/neoism"));
    assert_eq!(
        panel
            .sessions()
            .iter()
            .filter(|row| !row.is_header)
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["fix"]
    );
    panel.clear_session_query();
    assert!(!panel.project_expanded("/work/neoism"));
    assert!(!panel.sessions().iter().any(|row| row.id == "fix"));
}

#[test]
fn duplicate_names_and_windows_paths_keep_distinct_project_identity() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("one", "/a/site", 3, false),
        chat("two", "/b/site", 2, false),
        chat("win", "C:\\work\\neoism\\", 1, false),
    ]);
    let projects = panel
        .sessions()
        .iter()
        .filter(|row| row.is_project())
        .collect::<Vec<_>>();
    assert_eq!(
        projects
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["/a/site", "/b/site", "neoism"]
    );
    panel.set_selected(0);
    panel.toggle_selected_project();
    assert!(panel.sessions().iter().any(|row| row.id == "two"));
    assert!(!panel.sessions().iter().any(|row| row.id == "one"));
}

#[test]
fn project_keyboard_navigation_mirrors_folder_tree() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    panel.set_selected(0);
    assert!(panel.navigate_project(true));
    assert_eq!(panel.selected_session().unwrap().id, "a");
    assert!(panel.navigate_project(false));
    assert!(panel.selected_session().is_none());
    assert!(panel.navigate_project(false));
    assert!(panel.sessions().iter().all(|row| row.is_project()));
    assert!(panel.navigate_project(true));
    assert!(panel.sessions().iter().any(|row| row.id == "a"));
}

#[test]
fn continuation_pages_merge_into_projects_and_preserve_selected_chat() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_session_page(
        vec![chat("a", "/work/a", 3, false)],
        None,
        Some("next".into()),
    );
    let selected = panel
        .sessions()
        .iter()
        .position(|row| row.id == "a")
        .unwrap();
    panel.set_selected(selected);
    panel.set_session_page(
        vec![
            chat("b", "/work/b", 2, false),
            chat("older-a", "/work/a", 1, false),
        ],
        Some("next"),
        None,
    );
    assert_eq!(
        panel
            .sessions()
            .iter()
            .filter(|row| row.is_project())
            .count(),
        2
    );
    assert_eq!(panel.selected_session().unwrap().id, "a");
    assert_eq!(
        panel
            .sessions()
            .iter()
            .find(|row| row.is_project())
            .unwrap()
            .time_label,
        "2"
    );
}

#[test]
fn proportional_excerpts_use_shaped_width_and_cache_until_content_changes() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![chat("a", "/work/a", 1, false)]);
    panel.set_session_query("letters".into());
    panel.set_semantic_results(
        "letters".into(),
        vec![NeoismAgentSemanticMatch {
            session_id: "a".into(),
            excerpt: "iiii WWWW iiii WWWW iiii".into(),
            distance: 0.1,
        }],
    );
    let width = |text: &str| {
        text.chars()
            .map(|ch| if ch == 'W' { 3.0 } else { 1.0 })
            .sum::<f32>()
    };
    panel.set_result_wrap_metrics(12.0, 12.0, Some(1), 1.0, width);
    let excerpts = panel
        .sessions()
        .iter()
        .filter(|row| row.is_excerpt)
        .collect::<Vec<_>>();
    assert_eq!(excerpts.len(), 3);
    assert!(excerpts.iter().all(|row| width(&row.title) <= 12.0));
    assert_eq!(excerpts[0].title, "iiii");
    panel.set_result_wrap_metrics(12.0, 12.0, Some(1), 1.0, |_| {
        panic!("unchanged layout must stay cached")
    });
}

#[test]
fn excerpt_reflow_preserves_selected_chat() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("a", "/work/a", 2, false),
        chat("b", "/work/a", 1, false),
    ]);
    panel.set_session_query("text".into());
    panel.set_semantic_results(
        "text".into(),
        vec![
            NeoismAgentSemanticMatch {
                session_id: "a".into(),
                excerpt: "one two three four five six".into(),
                distance: 0.1,
            },
            NeoismAgentSemanticMatch {
                session_id: "b".into(),
                excerpt: "text".into(),
                distance: 0.2,
            },
        ],
    );
    let index = panel
        .sessions()
        .iter()
        .position(|row| row.id == "b" && !row.is_excerpt)
        .unwrap();
    panel.set_selected(index);
    panel.set_result_wrap_metrics(8.0, 12.0, Some(1), 1.0, |text| text.len() as f32);
    assert_eq!(panel.selected_session().unwrap().id, "b");
}

#[test]
fn live_project_reordering_keeps_the_selected_folder_identity() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("a", "/work/a", 2, false),
        chat("b", "/work/b", 1, false),
    ]);
    panel.set_selected(0);
    panel.upsert_session(chat("new-b", "/work/b", 3, false));
    assert_eq!(
        panel.selected_row().unwrap().directory.as_deref(),
        Some("/work/a")
    );
    assert!(panel.selected_row().unwrap().is_project());
}

#[test]
fn delayed_search_preserves_live_metadata_and_never_resurrects_deleted_chats() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![
        chat("live", "/work/a", 3, true),
        chat("deleted", "/work/b", 2, false),
    ]);
    panel.remove_session("deleted");
    panel.admit_search_sessions(vec![
        chat("live", "/old/path", 1, false),
        chat("deleted", "/work/b", 2, false),
        chat("discovered", "/work/c", 1, false),
    ]);
    let live = panel
        .sessions()
        .iter()
        .find(|row| row.id == "live")
        .unwrap();
    assert!(live.pinned);
    assert_eq!(live.directory.as_deref(), Some("/work/a"));
    assert!(!panel.sessions().iter().any(|row| row.id == "deleted"));
    assert!(panel.sessions().iter().any(|row| row.id == "discovered"));
}

#[test]
fn semantic_results_preserve_selection_when_a_newer_project_appears() {
    let mut panel = NeoismAgentSidePanel::default();
    let mut title = chat("title", "/work/b", 1, false);
    title.title = "matching title".into();
    panel.set_sessions(vec![title]);
    panel.set_session_query("matching".into());
    panel.set_selected(1);
    panel.admit_search_sessions(vec![chat("transcript", "/work/a", 2, false)]);
    panel.set_semantic_results(
        "matching".into(),
        vec![NeoismAgentSemanticMatch {
            session_id: "transcript".into(),
            excerpt: "matching excerpt".into(),
            distance: 0.1,
        }],
    );
    assert_eq!(panel.sessions()[0].directory.as_deref(), Some("/work/a"));
    assert_eq!(panel.selected_session().unwrap().id, "title");
}

#[test]
fn endpoint_invalidation_drops_external_previews_and_search_metadata() {
    let mut panel = NeoismAgentSidePanel::default();
    panel
        .external_sessions
        .push(chat("private", "/old/private", 1, false));
    panel.semantic_results.push(NeoismAgentSemanticMatch {
        session_id: "private".into(),
        excerpt: "private excerpt".into(),
        distance: 0.1,
    });
    panel.collapsed_projects.insert("/old/private".into());
    panel.invalidate_session_catalog();
    panel.set_sessions(vec![chat("new", "/new/project", 2, false)]);
    assert!(!panel.sessions().iter().any(|row| row.id == "private"));
    assert!(panel.semantic_results.is_empty());
    assert!(panel.collapsed_projects.is_empty());
}

#[test]
fn empty_search_batch_does_not_settle_an_unloaded_catalog() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.upsert_sessions(Vec::new());
    assert_eq!(panel.session_catalog_state(), &SessionCatalogState::Initial);
}

#[test]
fn missing_directory_chats_have_a_non_navigable_other_group() {
    let mut panel = NeoismAgentSidePanel::default();
    panel.set_sessions(vec![NeoismAgentSessionEntry::new("legacy", "Legacy", "")]);
    assert_eq!(panel.sessions()[0].title, "Other chats");
    assert!(!panel.sessions()[0].is_selectable());
    panel.set_selected(0);
    assert_eq!(panel.selected_session().unwrap().id, "legacy");
}
