use super::*;
use neoism_protocol::pull_request::PullRequestSummary;
use neoism_ui::panels::git_diff_view::GitDiffViewAction;

#[test]
fn inbox_initial_load_keeps_its_route_and_cannot_be_mistaken_for_creation() {
    let mut inbox = GitDiffView::pull_request_inbox("/repo".into());
    let creation = GitDiffView::create_pull_request(
        "/repo".into(),
        "topic".into(),
        "sha".into(),
        "main".into(),
    );
    assert!(inbox.is_pull_request());
    assert!(inbox.is_read_only());
    assert!(inbox.is_pull_request_inbox());
    assert!(!creation.is_pull_request_inbox());
    assert_eq!(inbox.pull_request_number(), None);
    assert_eq!(creation.pull_request_number(), None);
    let actions = route_pull_request_actions(7, inbox.take_actions());
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].0, 7);
    assert!(matches!(
        &actions[0].1,
        GitDiffViewAction::PullRequest { root, request: PullRequestRequest::List { .. } }
            if root == Path::new("/repo")
    ));
    assert!(inbox.take_edit_request().is_none());
}

#[test]
fn created_refresh_keeps_original_owner_after_focus_changes() {
    let mut view = GitDiffView::create_pull_request(
        "/repo".into(),
        "topic".into(),
        "sha".into(),
        "main".into(),
    );
    let original_route = 7;
    let now_focused_route = 99;
    view.apply_pull_request_reply(&PullRequestReply::Created {
        pull_request: PullRequestSummary {
            number: 42,
            title: "Review".into(),
            url: "https://github.com/a/b/pull/42".into(),
            author: "a".into(),
            state: "OPEN".into(),
            draft: false,
            base_branch: "main".into(),
            head_branch: "topic".into(),
            base_sha: "base".into(),
            head_sha: "sha".into(),
            review_decision: String::new(),
            checks_state: String::new(),
        },
    });
    let actions = route_pull_request_actions(original_route, view.take_actions());
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].0, original_route);
    assert_ne!(actions[0].0, now_focused_route);
    assert!(
        matches!(&actions[0].1, GitDiffViewAction::PullRequest { root, request: PullRequestRequest::Read { number: 42 } } if root == Path::new("/repo"))
    );
    assert!(view.is_read_only());
    assert_eq!(view.pull_request_number(), Some(42));
}

#[test]
fn typed_cancellation_unblocks_original_page_without_discarding_form() {
    let mut view = GitDiffView::create_pull_request(
        "/repo".into(),
        "topic".into(),
        "sha".into(),
        "main".into(),
    );
    view.pull_request_text_input("Draft title");
    view.pull_request_key("Enter", false, true);
    assert_eq!(view.take_actions().len(), 1);
    view.apply_pull_request_reply(&PullRequestReply::Error {
        kind: PullRequestErrorKind::Unavailable,
        message: "Host changed; verify before retry".into(),
    });
    assert!(view.has_pull_request_drafts());
    view.pull_request_key("Enter", false, true);
    assert_eq!(view.take_actions().len(), 1);
}
