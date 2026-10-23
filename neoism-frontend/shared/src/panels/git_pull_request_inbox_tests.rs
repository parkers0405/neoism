use super::*;
#[test]
fn failed_cached_read_refresh_retries_by_row_without_dirtying_the_inbox() {
    let mut i = loaded();
    load(&mut i, 1);
    i.children.get_mut(&1).unwrap().pr_intent(Intent::Refresh);
    i.take_actions();
    i.apply(&error());
    assert!(i.detail_errors.contains_key(&1));
    assert!(!i.has_drafts());
    i.select(1);
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::Read { number: 1 },
            ..
        }]
    ));
    assert!(!i.has_drafts());
    assert!(!i.detail_errors.contains_key(&1));
}
#[test]
fn human_facing_labels_never_leak_protocol_spellings() {
    assert_eq!(
        format::sort_label(PullRequestSort::Updated),
        "Recently updated"
    );
    assert_eq!(format::sort_label(PullRequestSort::Newest), "Newest first");
    assert_eq!(format::sort_label(PullRequestSort::Oldest), "Oldest first");
    assert_eq!(
        format::filter_label(PullRequestFilter::All),
        "All pull requests"
    );
    assert_eq!(format::review("REVIEW_REQUIRED").label, "Review requested");
    assert_eq!(format::review("CHANGES_REQUESTED").tone, format::Tone::Bad);
    assert_eq!(format::checks("SUCCESS").label, "Checks passed");
    assert_eq!(format::checks("IN_PROGRESS").tone, format::Tone::Pending);
    assert_eq!(
        format::checks("unknown host value").label,
        "No checks reported"
    );
    assert_eq!(format::state_label("OPEN", true), "Draft");
}
#[test]
fn update_dates_are_relative_or_compact_and_validate_calendar_input() {
    let epoch = 1_735_689_600; // 2025-01-01 00:00 UTC
    assert_eq!(
        format::updated("2025-01-01T00:00:00Z", epoch + 20),
        "just now"
    );
    assert_eq!(
        format::updated("2025-01-01T00:00:00Z", epoch + 180),
        "3m ago"
    );
    assert_eq!(
        format::updated("2025-01-01T02:00:00+02:00", epoch + 7200),
        "2h ago"
    );
    assert_eq!(
        format::updated("2025-01-01T00:00:00.123Z", epoch + 86400 * 2),
        "2d ago"
    );
    assert_eq!(
        format::updated("2024-01-01T00:00:00Z", epoch),
        "Jan 1, 2024"
    );
    assert_eq!(
        format::updated("2025-02-30T00:00:00Z", epoch),
        "Update time unavailable"
    );
    assert_eq!(format::updated("", epoch), "Update time unavailable");
    assert_eq!(
        format::updated("2026-08-01T00:00:00Z", epoch),
        "Aug 1, 2026"
    );
}
#[test]
fn empty_page_retains_repository_scope_and_authenticated_viewer() {
    let mut i = PullRequestInbox::new("/repo".into());
    i.take_actions();
    i.apply(&page(1, &[], None));
    assert!(i.items.is_empty());
    assert_eq!(i.repository, "o/r");
    assert_eq!(i.viewer, "alice");
    i.refresh_list();
    i.take_actions();
    i.apply(&error());
    assert_eq!(i.repository, "o/r");
    assert_eq!(i.viewer, "alice");
}
#[test]
fn list_retry_and_detail_retry_have_distinct_request_ownership() {
    let mut i = PullRequestInbox::new("/repo".into());
    i.take_actions();
    i.apply(&error());
    let r = Rect::new(0.0, 0.0, 100.0, 30.0);
    assert!(i.list_error.is_some());
    i.hits = vec![(r, Hit::RefreshList)];
    i.pointer_down(5.0, 5.0);
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::List { generation: 2, .. },
            ..
        }]
    ));
    i.apply(&page(2, &[1], None));
    i.select(1);
    i.take_actions();
    i.apply(&error());
    assert!(i.list_error.is_none());
    assert!(i.detail_errors.contains_key(&1));
    assert!(!i.detail_ready());
    for hit in [Hit::Checkout, Hit::Review] {
        i.hits = vec![(r, hit)];
        i.pointer_down(5.0, 5.0);
        assert!(i.take_actions().is_empty());
    }
    i.hits = vec![(r, Hit::Refresh)];
    i.pointer_down(5.0, 5.0);
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::ReadInbox { number: 1 },
            ..
        }]
    ));
    i.apply(&error());
    i.select(1); // clicking the same failed row retries too
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::ReadInbox { number: 1 },
            ..
        }]
    ));
    i.apply(&PullRequestReply::InboxDetail {
        detail: detail(1),
        item: item(1),
    });
    assert!(i.detail_ready());
    assert!(!i.detail_errors.contains_key(&1));
    i.hits = vec![(r, Hit::RefreshList)];
    i.pointer_down(5.0, 5.0);
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::List { .. },
            ..
        }]
    ));
    assert_eq!(i.selected, Some(1));
    assert!(i.children.contains_key(&1));
}
#[test]
fn loading_and_read_only_refresh_are_clean_but_mutations_are_protected() {
    let mut i = loaded();
    assert!(!i.has_drafts());
    i.select(1);
    assert!(!i.has_drafts());
    i.take_actions();
    assert!(!i.has_drafts());
    i.apply(&PullRequestReply::InboxDetail {
        detail: detail(1),
        item: item(1),
    });
    i.children.get_mut(&1).unwrap().pr_intent(Intent::Refresh);
    assert!(!i.has_drafts());
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::Read { number: 1 },
            ..
        }]
    ));
    assert!(!i.has_drafts());
    i.apply(&PullRequestReply::Detail { detail: detail(1) });
    i.refresh_list();
    i.take_actions();
    i.children
        .get_mut(&1)
        .unwrap()
        .pr_intent(Intent::Resolve("thread".into(), true));
    assert!(i.has_drafts());
    assert!(i.take_actions().is_empty()); // queued behind List
    assert!(i.has_drafts());
    i.apply(&page(2, &[1], None));
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::ResolveThread { number: 1, .. },
            ..
        }]
    ));
    assert!(i.has_drafts());
    i.apply(&error());
    assert!(!i.has_drafts());
}
#[test]
fn unicode_search_edits_graphemes_and_keeps_the_measured_caret_visible() {
    let mut i = loaded();
    i.search_focus = true;
    i.text_input("a👩‍💻e\u{301}");
    i.key("Backspace", false, false);
    assert_eq!(i.query, "a👩‍💻");
    i.key("Backspace", false, false);
    assert_eq!(i.query, "a");
    i.text_input("βγ");
    i.key("ArrowLeft", false, false);
    i.text_input("δ");
    assert_eq!(i.query, "aβδγ");
    i.key("Home", false, false);
    i.key("Delete", false, false);
    assert_eq!(i.query, "βδγ");
    let (offset, caret) = format::search_geometry(200.0, 200.0, 80.0, 0.0, 1.0);
    assert_eq!(offset, 121.0);
    assert_eq!(caret, 79.0);
    let (offset, caret) = format::search_geometry(0.0, 200.0, 80.0, offset, 1.0);
    assert_eq!((offset, caret), (0.0, 0.0));
    let (_, caret) = format::search_geometry(200.0, 200.0, 0.0, 0.0, 1.0);
    assert_eq!(caret, 0.0);
    assert_eq!(
        format::ellipsis("a👩‍💻bc", 3.0, |s| s.graphemes(true).count() as f32),
        "a👩‍💻…"
    );
}
#[test]
fn dropdown_current_selection_escape_and_outside_click_do_not_change_query() {
    let mut i = loaded();
    i.sort = PullRequestSort::Oldest;
    i.open_menu(Menu::Sort);
    assert_eq!(i.menu_index, 2);
    i.key("Escape", false, false);
    assert!(i.menu.is_none());
    assert!(i.take_actions().is_empty());
    i.open_menu(Menu::Filter);
    i.menu_hits = vec![(Rect::new(0.0, 0.0, 100.0, 30.0), 3)];
    assert!(i.pointer_down(5.0, 5.0));
    assert_eq!(i.filter, PullRequestFilter::All);
    assert!(i.menu.is_none());
    i.take_actions();
    let generation = i.generation;
    i.open_menu(Menu::Sort);
    i.menu_hits.clear();
    i.pointer_down(500.0, 500.0);
    assert!(i.menu.is_none());
    assert_eq!(i.generation, generation);
}
#[test]
fn menus_choose_sort_filter_and_search_without_parallel_flights() {
    let mut i = loaded();
    let r = Rect::new(0.0, 0.0, 100.0, 30.0);
    i.hits = vec![(r, Hit::Sort)];
    i.pointer_down(5.0, 5.0);
    assert_eq!(i.menu, Some(Menu::Sort));
    assert_eq!(i.sort, PullRequestSort::Updated);
    assert!(i.take_actions().is_empty());
    i.key("ArrowDown", false, false);
    i.key("Enter", false, false);
    assert_eq!(i.sort, PullRequestSort::Newest);
    assert_eq!(i.take_actions().len(), 1);
    i.hits = vec![(r, Hit::Filter)];
    i.pointer_down(5.0, 5.0);
    assert_eq!(i.filter, PullRequestFilter::Open);
    i.key("ArrowDown", false, false);
    i.key("Enter", false, false);
    assert_eq!(i.filter, PullRequestFilter::Closed);
    i.hits = vec![(r, Hit::Search)];
    i.pointer_down(5.0, 5.0);
    assert!(i.text_input("label:bug"));
    i.key("Backspace", false, false);
    assert_eq!(i.query, "label:bu");
    assert!(i.take_actions().is_empty());
    i.apply(&page(2, &[99], None));
    assert!(i.items.is_empty());
    assert!(
        matches!(i.take_actions().as_slice(),[GitDiffViewAction::PullRequest {request:PullRequestRequest::List {generation:5,filter:PullRequestFilter::Closed,sort:PullRequestSort::Newest,query,..},..}] if query=="label:bu")
    );
}
#[test]
fn wrong_number_does_not_release_pending_mutation() {
    let mut i = loaded();
    load(&mut i, 1);
    i.children
        .get_mut(&1)
        .unwrap()
        .pr_intent(Intent::Checkout(false));
    i.take_actions();
    i.apply(&PullRequestReply::ReviewSubmitted { number: 2 });
    assert!(matches!(
        i.pending,
        Some(Pending::Child {
            number: 1,
            mutation: true
        })
    ));
    assert!(i.children[&1].pull_request.as_ref().unwrap().pending);
    i.apply(&error());
    assert!(i.pending.is_none());
    assert!(!i.children[&1].pull_request.as_ref().unwrap().pending);
}
#[test]
fn checkout_queued_behind_list_rechecks_other_cached_drafts() {
    let mut i = loaded();
    load(&mut i, 1);
    load(&mut i, 2);
    i.refresh_list();
    i.take_actions();
    i.children
        .get_mut(&1)
        .unwrap()
        .pr_intent(Intent::Checkout(false));
    assert!(i.take_actions().is_empty());
    i.children
        .get_mut(&2)
        .unwrap()
        .pull_request
        .as_mut()
        .unwrap()
        .fields[3] = "late draft".into();
    i.apply(&page(2, &[1, 2], None));
    assert!(i.take_actions().is_empty());
    assert!(i.error.is_some());
    assert!(!i.children[&1].pull_request.as_ref().unwrap().pending);
}
#[test]
fn code_scroll_is_delegated_and_escape_closes_sheet_before_back_navigation() {
    let mut i = loaded();
    load(&mut i, 1);
    i.tabs.insert(1, DetailTab::Code);
    i.detail_rect = Rect::new(400.0, 0.0, 600.0, 600.0);
    i.children.get_mut(&1).unwrap().max_scroll = 100.0;
    i.pointer_move(500.0, 200.0);
    i.scroll(40.0);
    assert_eq!(i.children[&1].scroll_top, 40.0);
    i.prelude_height = 200.0;
    i.children.get_mut(&1).unwrap().max_scroll = 1000.0;
    i.scroll(120.0);
    assert_eq!(i.detail_scroll, 120.0);
    assert_eq!(i.children[&1].scroll_top, 40.0);
    i.scroll(120.0);
    assert_eq!(i.detail_scroll, 200.0);
    assert_eq!(i.children[&1].scroll_top, 80.0);
    assert_eq!(i.children[&1].max_scroll, 800.0);
    i.scroll(-100.0);
    assert_eq!(i.children[&1].scroll_top, 0.0);
    assert_eq!(i.detail_scroll, 180.0);
    assert_eq!(i.children[&1].max_scroll, 820.0);
    i.children.get_mut(&1).unwrap().pr_intent(Intent::Review);
    {
        let p = i
            .children
            .get_mut(&1)
            .unwrap()
            .pull_request
            .as_mut()
            .unwrap();
        p.max_scroll = 500.0;
    }
    i.scroll(70.0);
    assert_eq!(i.detail_scroll, 180.0);
    assert_eq!(i.children[&1].pull_request.as_ref().unwrap().scroll, 70.0);
    i.scroll(-100.0);
    assert_eq!(i.detail_scroll, 180.0);
    assert!(i.key("Escape", false, false));
    assert!(!i.children[&1].pull_request.as_ref().unwrap().sheet);
    assert!(i.narrow_detail);
    i.key("Escape", false, false);
    assert!(!i.narrow_detail);
}
#[test]
fn timeline_scroll_chains_through_conversation_range() {
    let mut i = loaded();
    load(&mut i, 1);
    i.tabs.insert(1, DetailTab::Timeline);
    i.detail_rect = Rect::new(0.0, 0.0, 600.0, 600.0);
    i.prelude_height = 150.0;
    let p = i
        .children
        .get_mut(&1)
        .unwrap()
        .pull_request
        .as_mut()
        .unwrap();
    p.tab = Tab::Conversation;
    p.max_scroll = 700.0;
    i.scroll_detail(250.0);
    assert_eq!(i.detail_scroll, 150.0);
    let p = i.children[&1].pull_request.as_ref().unwrap();
    assert_eq!(p.scroll, 100.0);
    assert_eq!(p.max_scroll, 550.0);
    i.scroll_detail(-180.0);
    assert_eq!(i.detail_scroll, 70.0);
    let p = i.children[&1].pull_request.as_ref().unwrap();
    assert_eq!(p.scroll, 0.0);
    assert_eq!(p.max_scroll, 630.0);
}

#[test]
fn translated_header_controls_are_clipped_before_registering_hits() {
    let clip = [400.0, 50.0, 230.0, 100.0];
    let hidden = render::clipped_control(Rect::new(420.0, 0.0, 100.0, 28.0), clip);
    assert_eq!(hidden.h, 0.0);
    let partial = render::clipped_control(Rect::new(420.0, 40.0, 100.0, 28.0), clip);
    assert_eq!(partial.y, 50.0);
    assert_eq!(partial.h, 18.0);
    let narrow = render::clipped_control(Rect::new(620.0, 140.0, 100.0, 28.0), clip);
    assert_eq!(narrow.w, 10.0);
    assert_eq!(narrow.h, 10.0);
}

#[test]
fn placeholders_distinguish_pending_from_empty_and_errors() {
    let mut i = PullRequestInbox::new(PathBuf::from("/repo"));
    assert!(i.list_loading());
    i.take_actions();
    i.apply(&page(1, &[], None));
    assert!(!i.list_loading());
    assert!(i.items.is_empty());
    i.refresh_list();
    assert!(i.list_loading());
    i.list_error = Some("Network unavailable".into());
    assert!(!i.list_loading());

    let mut i = loaded();
    i.select(1);
    assert!(i.detail_loading(1));
    i.detail_errors.insert(1, "Permission denied".into());
    assert!(!i.detail_loading(1));
    i.detail_errors.remove(&1);
    assert!(i.detail_loading(1));
    i.apply(&PullRequestReply::InboxDetail {
        detail: detail(1),
        item: item(1),
    });
    assert!(!i.detail_loading(1));
}

#[test]
fn detail_viewport_is_visible_only_and_grows_to_full_height() {
    for width in [230.0, 650.0, 1200.0] {
        for height in [90.0, 700.0] {
            let (_, r) =
                render::panes(Rect::new(20.0, 30.0, width, height), true, true, 1.0);
            let mut last = 0.0;
            for offset in [0.0, 100.0, 300.0, 500.0] {
                let v = render::content_viewport(r, r.x, r.w, r.y + 300.0 - offset);
                assert!(v.y >= r.y);
                assert!(v.y + v.h <= r.y + r.h);
                assert!(v.h >= last);
                last = v.h;
            }
            assert_eq!(last, r.h);
        }
    }
}
#[test]
fn list_scroll_and_summary_document_scroll_are_independent() {
    let mut i = loaded();
    load(&mut i, 1);
    i.list_rect = Rect::new(0.0, 0.0, 400.0, 600.0);
    i.detail_rect = Rect::new(400.0, 0.0, 600.0, 600.0);
    i.list_max = 500.0;
    i.detail_max = 900.0;
    i.pointer_move(100.0, 200.0);
    i.scroll(80.0);
    assert_eq!(i.list_scroll, 80.0);
    assert_eq!(i.detail_scroll, 0.0);
    i.pointer_move(500.0, 200.0);
    i.scroll(300.0);
    assert_eq!(i.detail_scroll, 300.0);
    assert_eq!(i.list_scroll, 80.0);
    assert!(i.key("Home", false, false));
    assert_eq!(i.detail_scroll, 0.0);
    i.tabs.insert(1, DetailTab::Code);
    i.prelude_height = 200.0;
    assert!(i.key("PageDown", false, false));
    assert_eq!(i.detail_scroll, 200.0);
    assert!(i.key("Tab", false, false));
    assert_eq!(i.detail_scroll, 0.0);
}
#[test]
fn hidden_child_does_not_own_header_pointer_or_horizontal_scroll() {
    let mut i = loaded();
    load(&mut i, 1);
    i.tabs.insert(1, DetailTab::Code);
    i.child_rect = Rect::new(400.0, 300.0, 600.0, 300.0);
    assert!(!i.child_visible_at(500.0, 100.0));
    assert!(i.child_visible_at(500.0, 400.0));
    i.children.get_mut(&1).unwrap().max_horizontal = [500.0; 2];
    i.horizontal(500.0, 100.0, 100.0);
    assert_eq!(i.children[&1].horizontal, [0.0; 2]);
    i.clear_child_pointer();
    assert!(!i.child_visible_at(500.0, 400.0));
}

fn item(n: u64) -> PullRequestInboxItem {
    PullRequestInboxItem {
        summary: PullRequestSummary {
            number: n,
            title: format!("PR {n}"),
            url: format!("https://github.com/o/r/pull/{n}"),
            author: "alice".into(),
            state: "OPEN".into(),
            draft: false,
            base_branch: "main".into(),
            head_branch: format!("topic-{n}"),
            base_sha: "a".repeat(40),
            head_sha: "b".repeat(40),
            review_decision: "REVIEW_REQUIRED".into(),
            checks_state: "SUCCESS".into(),
        },
        repository: "o/r".into(),
        updated_at: "2026-08-01".into(),
        additions: 12,
        deletions: 3,
        labels: vec!["bug".into()],
        reviewers: vec!["bob".into()],
        authored: true,
        review_requested: false,
        assigned: false,
    }
}
fn detail(n: u64) -> PullRequestDetail {
    PullRequestDetail {
        summary: item(n).summary,
        body: "# Description\n\n**Real** markdown".into(),
        files: vec![],
        diffs: vec![],
        conversation: vec![],
        threads: vec![],
        checks: vec![],
        truncated: false,
    }
}
fn page(generation: u64, ns: &[u64], cursor: Option<&str>) -> PullRequestReply {
    PullRequestReply::Listed {
        page: PullRequestPage {
            repository: "o/r".into(),
            viewer: "alice".into(),
            generation,
            items: ns.iter().map(|&n| item(n)).collect(),
            next_cursor: cursor.map(str::to_string),
            total_count: 20,
        },
    }
}
fn loaded() -> PullRequestInbox {
    let mut i = PullRequestInbox::new("/repo".into());
    i.take_actions();
    i.apply(&page(1, &[1, 2], None));
    i
}
fn load(i: &mut PullRequestInbox, n: u64) {
    i.select(n);
    i.take_actions();
    i.apply(&PullRequestReply::InboxDetail {
        detail: detail(n),
        item: item(n),
    });
}
fn error() -> PullRequestReply {
    PullRequestReply::Error {
        kind: PullRequestErrorKind::Failed,
        message: "network unavailable".into(),
    }
}
#[test]
fn constructor_is_immutable_and_queues_initial_list_without_pointer_input() {
    let mut v = GitDiffView::pull_request_inbox("/repo".into());
    assert!(v.is_read_only() && v.is_pull_request() && v.is_pull_request_inbox());
    assert_eq!(v.pull_request_number(), None);
    assert_eq!(v.pull_request_title(), Some("Pull Requests"));
    v.set_edit_focus(true);
    assert!(v.document_path().is_none());
    assert!(v.editing_path().is_none());
    assert!(matches!(
        v.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::List {
                generation: 1,
                cursor: None,
                ..
            },
            ..
        }]
    ));
    assert!(v.take_actions().is_empty());
}
#[test]
fn search_defers_latest_generation_and_never_displays_old_page() {
    let mut i = PullRequestInbox::new("/repo".into());
    i.take_actions();
    i.search_focus = true;
    i.text_input("label:bug");
    i.text_input(" author:alice");
    assert!(i.take_actions().is_empty());
    i.apply(&page(1, &[99], None));
    assert!(i.items.is_empty());
    assert!(
        matches!(i.take_actions().as_slice(),[GitDiffViewAction::PullRequest {request:PullRequestRequest::List {query,generation:3,..},..}] if query=="label:bug author:alice")
    );
    // Duplicate old page doesn't release the latest flight.
    i.apply(&page(1, &[99], None));
    assert!(i.pending.is_some());
    i.apply(&page(3, &[1], None));
    assert_eq!(i.items[0].summary.number, 1);
}
#[test]
fn pending_error_belongs_to_older_query_and_latest_still_runs() {
    let mut i = PullRequestInbox::new("/repo".into());
    i.take_actions();
    i.query = "new".into();
    i.refresh_list();
    i.apply(&error());
    assert!(i.list_error.is_none());
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::List { generation: 2, .. },
            ..
        }]
    ));
    i.apply(&error());
    assert_eq!(i.list_error.as_deref(), Some("network unavailable"));
    assert!(i.pending.is_none());
}
#[test]
fn read_errors_release_flight_and_latest_selection_runs() {
    let mut i = loaded();
    i.select(1);
    i.take_actions();
    i.select(2);
    assert!(i.take_actions().is_empty());
    i.apply(&error());
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::ReadInbox { number: 2 },
            ..
        }]
    ));
    i.apply(&PullRequestReply::InboxDetail {
        detail: detail(2),
        item: item(2),
    });
    assert_eq!(i.selected, Some(2));
    assert!(i.children.contains_key(&2));
}
#[test]
fn selection_cache_and_drafts_survive_rows_and_list_refresh() {
    let mut i = loaded();
    load(&mut i, 1);
    i.children
        .get_mut(&1)
        .unwrap()
        .pull_request
        .as_mut()
        .unwrap()
        .fields[3] = "unsent review".into();
    load(&mut i, 2);
    i.select(1);
    assert!(i.take_actions().is_empty());
    assert_eq!(
        i.children[&1].pull_request.as_ref().unwrap().fields[3],
        "unsent review"
    );
    i.refresh_list();
    i.take_actions();
    i.apply(&page(2, &[2], None));
    assert_eq!(i.selected, Some(1));
    assert!(i.has_drafts());
    assert_eq!(i.children.len(), 2);
    assert_eq!(i.metadata[&1].summary.number, 1);
}
#[test]
fn mutation_and_refresh_route_to_original_cached_child() {
    let mut i = loaded();
    load(&mut i, 1);
    load(&mut i, 2);
    i.select(1);
    i.children.get_mut(&1).unwrap().pr_intent(Intent::Review);
    i.children
        .get_mut(&1)
        .unwrap()
        .pull_request_text_input("review");
    i.children.get_mut(&1).unwrap().pr_intent(Intent::Submit);
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::SubmitReview { number: 1, .. },
            ..
        }]
    ));
    i.select(2);
    i.children
        .get_mut(&2)
        .unwrap()
        .pull_request
        .as_mut()
        .unwrap()
        .fields[3] = "keep".into();
    i.apply(&PullRequestReply::ReviewSubmitted { number: 1 });
    assert!(matches!(
        i.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::Read { number: 1 },
            ..
        }]
    ));
    assert!(i.children[&1].pull_request.as_ref().unwrap().fields[3].is_empty());
    assert_eq!(
        i.children[&2].pull_request.as_ref().unwrap().fields[3],
        "keep"
    );
    i.apply(&PullRequestReply::Detail { detail: detail(1) });
    assert!(i.pending.is_none());
    assert_eq!(i.selected, Some(2));
}
#[test]
fn background_child_error_is_not_displayed_as_the_selected_pr_error() {
    let mut i = loaded();
    load(&mut i, 1);
    load(&mut i, 2);
    i.children.get_mut(&1).unwrap().pr_intent(Intent::Refresh);
    i.take_actions();
    assert_eq!(i.selected, Some(2));
    i.apply(&error());
    assert_eq!(
        i.detail_errors.get(&1).map(String::as_str),
        Some("network unavailable")
    );
    assert!(!i.detail_errors.contains_key(&2));
    assert!(i.error.is_none());
}

#[test]
fn mutation_error_does_not_clear_other_child_pending_or_drafts() {
    let mut i = loaded();
    load(&mut i, 1);
    load(&mut i, 2);
    for n in [1, 2] {
        let c = i.children.get_mut(&n).unwrap();
        c.pr_intent(Intent::Review);
        c.pull_request_text_input("keep");
        c.pr_intent(Intent::Submit);
    }
    assert_eq!(i.take_actions().len(), 1);
    i.apply(&error());
    assert_eq!(i.take_actions().len(), 1);
    assert!(!i.children[&1].pull_request.as_ref().unwrap().pending);
    assert!(i.children[&2].pull_request.as_ref().unwrap().pending);
    assert_eq!(
        i.children[&1].pull_request.as_ref().unwrap().fields[3],
        "keep"
    );
}
#[test]
fn sort_filter_and_pagination_are_server_requests_and_deduplicate() {
    let mut i = loaded();
    i.sort = PullRequestSort::Oldest;
    i.filter = PullRequestFilter::Merged;
    i.query = "label:bug".into();
    i.refresh_list();
    assert!(
        matches!(i.take_actions().as_slice(),[GitDiffViewAction::PullRequest {request:PullRequestRequest::List {sort:PullRequestSort::Oldest,filter:PullRequestFilter::Merged,query,cursor:None,..},..}] if query=="label:bug")
    );
    i.apply(&page(2, &[1, 2], Some("next")));
    i.list = Some(i.next_cursor.clone());
    i.pump();
    assert!(
        matches!(i.take_actions().as_slice(),[GitDiffViewAction::PullRequest {request:PullRequestRequest::List {cursor:Some(cursor),generation:2,..},..}] if cursor=="next")
    );
    i.apply(&page(2, &[2, 3], None));
    assert_eq!(
        i.items.iter().map(|i| i.summary.number).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(i.total_count, 20);
    assert!(i.next_cursor.is_none());
}
#[test]
fn all_cached_drafts_guard_embedded_checkout_and_inbox_identity() {
    let mut v = GitDiffView::pull_request_inbox("/repo".into());
    v.take_actions();
    v.apply_pull_request_reply(&page(1, &[1, 2], None));
    let i = v.inbox.as_mut().unwrap();
    load(i, 1);
    load(i, 2);
    i.children
        .get_mut(&1)
        .unwrap()
        .pull_request
        .as_mut()
        .unwrap()
        .fields[3] = "draft".into();
    i.children
        .get_mut(&2)
        .unwrap()
        .pr_intent(Intent::Checkout(false));
    assert!(v.take_actions().is_empty());
    assert!(v.has_pull_request_drafts());
    assert_eq!(v.pull_request_number(), None);
    assert_eq!(v.pull_request_title(), Some("Pull Requests"));
}
#[test]
fn narrow_geometry_keeps_back_navigation_and_wide_geometry_persists_list() {
    let rect = Rect::new(10.0, 20.0, 1200.0, 700.0);
    let (l, d) = render::panes(rect, true, true, 1.0);
    assert!(l.w >= 320.0 && d.w > 500.0);
    assert_eq!(l.x + l.w, d.x);
    let small = Rect::new(10.0, 20.0, 500.0, 700.0);
    let (l, d) = render::panes(small, true, true, 1.0);
    assert_eq!(l.w, 0.0);
    assert_eq!(d.w, 500.0);
    let (l, d) = render::panes(small, true, false, 1.0);
    assert_eq!(l.w, 500.0);
    assert_eq!(d.w, 0.0);
}
#[test]
fn resized_prelude_does_not_reverse_scroll_into_forward_child_motion() {
    let mut i = loaded();
    load(&mut i, 1);
    i.tabs.insert(1, DetailTab::Code);
    i.detail_rect = Rect::new(400.0, 0.0, 600.0, 500.0);
    i.prelude_height = 100.0;
    i.detail_scroll = 200.0;
    i.children.get_mut(&1).unwrap().max_scroll = 500.0;
    i.scroll_detail(-10.0);
    assert_eq!(i.detail_scroll, 90.0);
    assert_eq!(i.children[&1].scroll_top, 0.0);

    i.detail_scroll = 200.0;
    i.scroll_detail(10.0);
    assert_eq!(i.detail_scroll, 100.0);
    assert_eq!(i.children[&1].scroll_top, 10.0);
}

#[test]
fn scroll_ownership_uses_last_pointer_region() {
    let mut i = loaded();
    i.list_rect = Rect::new(0.0, 0.0, 400.0, 600.0);
    i.detail_rect = Rect::new(400.0, 0.0, 600.0, 600.0);
    i.list_max = 200.0;
    i.detail_max = 300.0;
    i.pointer_move(10.0, 200.0);
    i.scroll(30.0);
    assert_eq!(i.list_scroll, 30.0);
    assert_eq!(i.detail_scroll, 0.0);
    i.pointer_move(500.0, 200.0);
    i.scroll(50.0);
    assert_eq!(i.detail_scroll, 50.0);
    assert_eq!(i.list_scroll, 30.0);
}
