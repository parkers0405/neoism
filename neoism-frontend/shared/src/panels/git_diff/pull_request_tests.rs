use super::*;

#[test]
fn inbox_opens_without_a_linked_pr_and_waits_for_the_active_lane() {
    let mut panel = GitDiffPanel::new();
    assert!(!panel.open_pull_request_inbox());
    panel.data.lock().unwrap().repo_root = Some("/repo".into());
    assert!(panel.begin_external_pull_request());
    assert!(panel.open_pull_request_inbox());
    assert!(panel.take_pull_request_opens().is_empty());
    panel.host_pull_request_reply(PullRequestReply::Error {
        kind: PullRequestErrorKind::Unavailable,
        message: "Discovery unavailable".into(),
    });
    assert!(matches!(
        panel.take_pull_request_opens().as_slice(),
        [PullRequestOpen::Inbox { root }] if root == std::path::Path::new("/repo")
    ));
    assert!(!panel.pull_request_row_visible());
    assert!(panel.pull_request_label().is_empty());
    assert!(panel.take_pull_request_notices().is_empty());
}

#[test]
fn inbox_open_survives_first_repository_discovery_identity_setup() {
    let mut panel = GitDiffPanel::new();
    panel.visible = true;
    panel.data.lock().unwrap().repo_root = Some("/repo".into());
    assert!(panel.open_pull_request_inbox());
    panel.discover_pull_request();
    assert!(matches!(
        panel.take_pull_request_opens().as_slice(),
        [PullRequestOpen::Inbox { root }] if root == std::path::Path::new("/repo")
    ));
}

#[test]
fn absent_pr_loading_and_background_failures_have_no_row_or_stale_hit_target() {
    let mut panel = GitDiffPanel::new();
    panel.visible = true;
    panel.data.lock().unwrap().pr.pending = true;
    assert!(!panel.pull_request_row_visible());
    {
        let mut data = panel.data.lock().unwrap();
        data.pr.discovering = true;
        data.pr.rect = Some(Rect {
            x: 10.0,
            y: 40.0,
            w: 200.0,
            h: 26.0,
        });
    }
    panel.host_pull_request_reply(PullRequestReply::Error {
        kind: PullRequestErrorKind::Unavailable,
        message: "The declared workspace is not ready for pull requests.".into(),
    });
    assert!(!panel.pull_request_row_visible());
    assert!(!panel.pull_request_hit(30.0, 45.0));
    assert!(panel.pull_request_label().is_empty());
    assert!(panel.take_pull_request_notices().is_empty());
    assert!(
        panel.data.lock().unwrap().error.is_none(),
        "local Git must remain usable"
    );
}

#[test]
fn only_a_real_pr_or_explicit_lookup_gets_a_row() {
    let mut panel = GitDiffPanel::new();
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "topic".into(),
        head: "sha".into(),
        default_branch: "main".into(),
        pull_request: None,
    });
    assert!(!panel.pull_request_row_visible());
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "topic".into(),
        head: "sha".into(),
        default_branch: "main".into(),
        pull_request: Some(PullRequestSummary {
            number: 42,
            title: "Fix selection".into(),
            url: "https://github.com/example/repo/pull/42".into(),
            author: "ada".into(),
            state: "OPEN".into(),
            draft: false,
            base_branch: "main".into(),
            head_branch: "topic".into(),
            base_sha: "base".into(),
            head_sha: "sha".into(),
            review_decision: String::new(),
            checks_state: String::new(),
        }),
    });
    assert!(panel.pull_request_row_visible());
    assert_eq!(panel.pull_request_label(), "#42  Fix selection");
    let summary = match panel.data.lock().unwrap().pr.discovered.as_ref().unwrap() {
        PullRequestReply::Discovered {
            pull_request: Some(pr),
            ..
        } => pr.clone(),
        _ => unreachable!(),
    };
    panel.data.lock().unwrap().pr.discovered = None;
    panel.start_pull_request_lookup();
    assert!(panel.pull_request_row_visible());
    panel.pull_request_lookup_key("Escape", None);
    assert!(!panel.pull_request_row_visible());
    panel.data.lock().unwrap().branch = Some("topic".into());
    panel.host_pull_request_reply(PullRequestReply::Created {
        pull_request: summary.clone(),
    });
    assert!(
        panel.pull_request_row_visible(),
        "creation installs the actual PR row immediately"
    );
    panel.data.lock().unwrap().pr.discovered = None;
    panel.data.lock().unwrap().branch = Some("other".into());
    panel.host_pull_request_reply(PullRequestReply::Created {
        pull_request: summary,
    });
    assert!(
        !panel.pull_request_row_visible(),
        "a late creation cannot associate another branch"
    );
}

#[test]
fn explicit_create_failures_are_notices_not_permanent_sidebar_labels() {
    let mut panel = GitDiffPanel::new();
    {
        let mut data = panel.data.lock().unwrap();
        data.pr.discovering = true;
        data.pr.open_create = true;
    }
    panel.host_pull_request_reply(PullRequestReply::Error {
        kind: PullRequestErrorKind::Authentication,
        message: "Sign in to GitHub on the repository host.".into(),
    });
    assert!(!panel.pull_request_row_visible());
    assert_eq!(
        panel.take_pull_request_notices(),
        vec!["Sign in to GitHub on the repository host."]
    );
    assert!(panel.take_pull_request_notices().is_empty());
}

#[test]
fn quiet_pr_row_hover_tracks_pointer_without_a_permanent_button_fill() {
    let mut panel = GitDiffPanel::new();
    panel.visible = true;
    panel.data.lock().unwrap().pr.rect = Some(Rect {
        x: 10.0,
        y: 40.0,
        w: 200.0,
        h: 26.0,
    });
    assert!(!panel.pull_request_hovered());
    assert!(panel.update_latest_commit_hover(30.0, 45.0));
    assert!(panel.pull_request_hovered());
    assert!(!panel.update_latest_commit_hover(30.0, 45.0));
    assert!(panel.update_latest_commit_hover(30.0, 70.0));
    assert!(!panel.pull_request_hovered());
    panel.start_pull_request_lookup();
    assert!(!panel.pull_request_label().contains('│'));
}

#[test]
fn checkout_branch_status_preserves_its_inflight_action_and_success_reply() {
    let mut panel = GitDiffPanel::new();
    panel.visible = true;
    {
        let mut data = panel.data.lock().unwrap();
        data.repo_root = Some("/repo".into());
        data.branch = Some("main".into());
        data.pr.generation = 7;
    }
    assert!(panel.begin_external_pull_request());
    panel.data.lock().unwrap().branch = Some("neoism/pr-42".into());
    panel.discover_pull_request();
    {
        let data = panel.data.lock().unwrap();
        assert!(data.pr.pending);
        assert_eq!(data.pr.generation, 7);
    }
    panel.host_pull_request_reply(PullRequestReply::CheckedOut {
        number: 42,
        path: "/repo".into(),
        worktree: false,
    });
    panel.discover_pull_request();
    assert!(matches!(
        panel.take_pull_request_replies().as_slice(),
        [PullRequestReply::CheckedOut { number: 42, .. }]
    ));
    panel.host_pull_request_reply(PullRequestReply::ReviewSubmitted { number: 42 });
    panel.data.lock().unwrap().repo_root = Some("/other".into());
    panel.discover_pull_request();
    assert!(
        panel.take_pull_request_replies().is_empty(),
        "changing repositories must still invalidate replies"
    );
    assert_eq!(panel.data.lock().unwrap().pr.generation, 8);
}

#[test]
fn numeric_lookup_owns_its_input_and_escape_cancels_without_opening_a_tab() {
    let mut panel = GitDiffPanel::new();
    panel.data.lock().unwrap().repo_root = Some("/repo".into());
    panel.start_pull_request_lookup();
    assert!(panel.pull_request_lookup_active());
    assert!(panel.pull_request_lookup_key("Enter", None));
    assert!(
        panel.pull_request_lookup_active(),
        "empty identifiers cannot submit"
    );
    panel.pull_request_lookup_key("", Some("#42"));
    panel.pull_request_lookup_key("", Some("https://github.com/repo2/project/pull/7"));
    assert!(panel.pull_request_label().contains("#42"));
    panel.pull_request_lookup_key("Backspace", None);
    assert!(panel.pull_request_label().contains("#4"));
    panel.pull_request_lookup_key("Escape", None);
    assert!(!panel.pull_request_lookup_active());
    assert!(panel.take_pull_request_opens().is_empty());
    assert!(panel.take_pull_request_replies().is_empty());
}
#[test]
fn hidden_discovery_completion_does_not_own_an_endless_redraw_loop() {
    let mut panel = GitDiffPanel::new();
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "feature".into(),
        head: "a".repeat(40),
        default_branch: "main".into(),
        pull_request: None,
    });
    assert!(!panel.pull_request_needs_redraw());
}

#[test]
fn worker_discovery_keeps_a_final_redraw_until_label_is_painted() {
    let mut panel = GitDiffPanel::new();
    panel.visible = true;
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "topic".into(),
        head: "h".into(),
        default_branch: "main".into(),
        pull_request: None,
    });
    assert!(panel.pull_request_needs_redraw());
    assert!(!panel.pull_request_row_visible());
    assert!(panel.pull_request_label().is_empty());
    assert!(!panel.pull_request_needs_redraw());
}

#[test]
fn native_completion_rejects_root_branch_and_generation_changes() {
    let mut d = super::super::types::PanelData::default();
    d.repo_root = Some("/repo".into());
    d.branch = Some("topic".into());
    d.pr.generation = 4;
    let root = std::path::Path::new("/repo");
    let branch = Some("topic".into());
    assert!(completion_is_current(&d, root, &branch, 4));
    assert!(!completion_is_current(&d, root, &branch, 3));
    d.branch = Some("other".into());
    assert!(!completion_is_current(&d, root, &branch, 4));
    d.branch = branch.clone();
    d.repo_root = Some("/other".into());
    assert!(!completion_is_current(&d, root, &branch, 4));
}

#[test]
fn external_busy_gate_and_completion() {
    let mut panel = GitDiffPanel::new();
    assert!(panel.begin_external_pull_request());
    assert!(!panel.begin_external_pull_request());
    panel.host_pull_request_reply(PullRequestReply::Error {
        kind: PullRequestErrorKind::Conflict,
        message: "busy".into(),
    });
    assert_eq!(panel.take_pull_request_replies().len(), 1);
    assert!(panel.begin_external_pull_request());
}
#[test]
fn discovery_never_occupies_action_reply_route() {
    let mut panel = GitDiffPanel::new();
    {
        let mut d = panel.data.lock().unwrap();
        d.pr.discovering = true;
        d.pr.pending = true;
    }
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "topic".into(),
        head: "sha".into(),
        default_branch: "main".into(),
        pull_request: None,
    });
    assert!(panel.take_pull_request_replies().is_empty());
    assert!(panel.take_pull_request_opens().is_empty());
    {
        let mut d = panel.data.lock().unwrap();
        d.pr.discovering = true;
        d.pr.pending = true;
    }
    panel.host_pull_request_reply(PullRequestReply::Error {
        kind: PullRequestErrorKind::Authentication,
        message: "Sign in".into(),
    });
    assert!(panel.take_pull_request_replies().is_empty());
    assert!(panel.take_pull_request_notices().is_empty());
    assert!(!panel.pull_request_row_visible());
    assert!(panel.pull_request_label().is_empty());
}
#[test]
fn discovery_only_opens_creation_on_explicit_activation() {
    let mut panel = GitDiffPanel::new();
    panel.data.lock().unwrap().repo_root = Some("/repo".into());
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "topic".into(),
        head: "sha".into(),
        default_branch: "main".into(),
        pull_request: None,
    });
    assert!(panel.take_pull_request_opens().is_empty());
    assert!(
        !panel.activate_pull_request(),
        "a missing PR is not a persistent create control"
    );
    panel.data.lock().unwrap().pr.open_create = true;
    panel.host_pull_request_reply(PullRequestReply::Discovered {
        branch: "topic".into(),
        head: "fresh-sha".into(),
        default_branch: "main".into(),
        pull_request: None,
    });
    assert!(
        matches!(panel.take_pull_request_opens().as_slice(), [PullRequestOpen::Create { branch, head, .. }] if branch == "topic" && head == "fresh-sha")
    );
}
#[test]
fn refresh_detail_is_reply_not_new_tab() {
    let mut panel = GitDiffPanel::new();
    panel.data.lock().unwrap().repo_root = Some("/repo".into());
    let detail:PullRequestDetail=serde_json::from_value(serde_json::json!({"summary":{"number":1,"title":"T","url":"https://example.test","author":"a","state":"open","draft":false,"base_branch":"main","head_branch":"topic","base_sha":"a","head_sha":"b","review_decision":"","checks_state":""},"body":"","files":[],"diffs":[],"conversation":[],"threads":[],"checks":[],"truncated":false})).unwrap();
    panel.host_pull_request_reply(PullRequestReply::Detail {
        detail: detail.clone(),
    });
    assert!(panel.take_pull_request_opens().is_empty());
    assert_eq!(panel.take_pull_request_replies().len(), 1);
    panel.data.lock().unwrap().pr.open_read = true;
    panel.host_pull_request_reply(PullRequestReply::Detail { detail });
    assert_eq!(panel.take_pull_request_opens().len(), 1);
    assert!(panel.take_pull_request_replies().is_empty());
}
