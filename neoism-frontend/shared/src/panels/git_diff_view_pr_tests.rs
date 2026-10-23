use super::*;

#[test]
fn hosted_review_mutations_refresh_the_same_pr_without_replaying_the_mutation() {
    for reply in [
        PullRequestReply::ThreadUpdated { number: 7 },
        PullRequestReply::ReviewSubmitted { number: 7 },
        PullRequestReply::Merged { number: 7 },
    ] {
        let mut v = view();
        v.apply_pull_request_reply(&reply);
        assert!(matches!(
            v.take_actions().as_slice(),
            [GitDiffViewAction::PullRequest {
                request: PullRequestRequest::Read { number: 7 },
                ..
            }]
        ));
        v.apply_pull_request_reply(&PullRequestReply::Detail { detail: detail() });
        assert!(v.take_actions().is_empty());
    }
    let mut v = view();
    v.apply_pull_request_reply(&PullRequestReply::ThreadUpdated { number: 8 });
    assert!(v.take_actions().is_empty());
}
#[test]
fn checkout_and_worktree_are_disabled_with_unsent_review_or_inline_drafts() {
    let mut v = view();
    v.pr_intent(Intent::Review);
    v.pull_request_text_input("unsent review");
    for worktree in [false, true] {
        v.pr_intent(Intent::Checkout(worktree));
        assert!(v.take_actions().is_empty());
    }
    v.pr_intent(Intent::Discard);
    let p = v.pull_request.as_mut().unwrap();
    p.comments.push(PullRequestInlineComment {
        path: "a".into(),
        side: PullRequestSide::Right,
        line: 1,
        start_line: None,
        body: "queued".into(),
    });
    for worktree in [false, true] {
        v.pr_intent(Intent::Checkout(worktree));
        assert!(v.take_actions().is_empty());
    }
    v.pr_intent(Intent::Discard);
    v.pr_intent(Intent::Checkout(true));
    assert!(matches!(
        v.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::Checkout {
                new_worktree: true,
                ..
            },
            ..
        }]
    ));
    v.pr_intent(Intent::Checkout(true));
    assert!(v.take_actions().is_empty());
}
#[test]
fn truncated_coverage_notice_is_independent_of_selected_tab() {
    let mut d = detail();
    d.truncated = true;
    let mut v = GitDiffView::from_pull_request("/repo".into(), d);
    for tab in [Tab::Changes, Tab::Conversation, Tab::Checks] {
        v.pr_intent(Intent::Tab(tab));
        assert!(v
            .pull_request
            .as_ref()
            .unwrap()
            .coverage_warning()
            .unwrap()
            .contains("Partial review"));
    }
}

#[test]
fn parsed_patch_wrap_anchors_are_source_lines_not_visual_rows() {
    let mut section = FileSection::new(FileChange {
        path: "a.rs".into(),
        status: FileStatus::Modified,
        additions: 1,
        deletions: 1,
        staged: false,
    });
    let mut lines = Vec::new();
    crate::panels::git_diff::parse_diff_into_with_headers(b"@@ -41,2 +81,2 @@\n unchanged\n-abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz\n+replacementabcdefghijklmnopqrstuvwxyzabcdefghijklmnop\n",&mut lines);
    section.set_lines(lines);
    section.measure_body(120.0, 1.0, false, 30.0);
    let lines = section.lines.as_ref().unwrap();
    for (i, line) in lines.iter().enumerate() {
        let first = section.wrapped_offsets[i];
        let end = section.wrapped_offsets[i + 1];
        if line.kind == DiffLineKind::Remove || line.kind == DiffLineKind::Add {
            assert!(end - first > 1, "test must exercise wrapping");
            for visual in first..end {
                let a = section.pr_anchor_for_visual_row(visual, false, 0).unwrap();
                assert_eq!(
                    a.line,
                    if line.kind == DiffLineKind::Remove {
                        42
                    } else {
                        82
                    }
                );
                assert_eq!(
                    a.side,
                    if line.kind == DiffLineKind::Remove {
                        PullRequestSide::Left
                    } else {
                        PullRequestSide::Right
                    }
                );
            }
        }
    }
    assert!(section
        .pr_anchor_for_visual_row(*section.wrapped_offsets.last().unwrap(), false, 0)
        .is_none());
    for column in 0..2 {
        let rows = if column == 0 {
            &section.left
        } else {
            &section.right
        };
        for (i, line) in rows.iter().enumerate() {
            let a = section.pr_anchor_for_visual_row(i, true, column);
            let expected = if column == 0 {
                line.old_line_number
            } else {
                line.new_line_number
            };
            assert_eq!(a.map(|a| a.line), expected);
        }
    }
}

fn detail() -> PullRequestDetail {
    PullRequestDetail {
        summary: PullRequestSummary {
            number: 7,
            title: "Review".into(),
            url: "https://example.test/pr/7".into(),
            author: "a".into(),
            state: "open".into(),
            draft: false,
            base_branch: "main".into(),
            head_branch: "topic".into(),
            base_sha: "base".into(),
            head_sha: "head".into(),
            review_decision: String::new(),
            checks_state: String::new(),
        },
        body: "Description".into(),
        files: Vec::new(),
        diffs: Vec::new(),
        conversation: Vec::new(),
        threads: Vec::new(),
        checks: Vec::new(),
        truncated: false,
    }
}
fn view() -> GitDiffView {
    GitDiffView::from_pull_request("/repo".into(), detail())
}
#[test]
fn immutable_without_fake_commit() {
    let mut v = view();
    assert!(v.is_read_only());
    assert!(v.commit_id.is_none());
    v.set_edit_focus(true);
    assert!(v.document_path().is_none());
    assert!(v.take_edit_request().is_none());
    v.pending_actions.push(GitDiffViewAction::StageAll {
        root: v.root.clone(),
    });
    v.pending_actions.push(GitDiffViewAction::OpenSource {
        root: v.root.clone(),
        path: "a".into(),
        line: 0,
        column: 0,
    });
    v.pending_actions.push(GitDiffViewAction::OpenUrl {
        url: "https://example.test".into(),
    });
    assert!(matches!(
        v.take_actions().as_slice(),
        [GitDiffViewAction::OpenUrl { .. }]
    ));
}
#[test]
fn creation_editing_multiline_cursor() {
    let mut v = GitDiffView::create_pull_request(
        "/repo".into(),
        "topic".into(),
        "head".into(),
        "main".into(),
    );
    assert!(v.is_read_only());
    assert!(v.pull_request_text_input("éTitle"));
    v.pull_request_key("Home", false, false);
    v.pull_request_key("Delete", false, false);
    assert_eq!(v.pull_request.as_ref().unwrap().fields[0], "Title");
    v.pull_request_key("Tab", false, false);
    v.pull_request_text_input("one");
    v.pull_request_key("Enter", false, false);
    v.pull_request_text_input("two");
    assert_eq!(v.pull_request.as_ref().unwrap().fields[1], "one\ntwo");
    v.pull_request_key("Tab", true, false);
    v.pull_request_key("End", false, false);
    v.pull_request_key("Backspace", false, false);
    assert_eq!(v.pull_request.as_ref().unwrap().fields[0], "Titl");
    assert!(v.has_pull_request_drafts());
}
#[test]
fn submit_once_and_error_retains_creation() {
    let mut v = GitDiffView::create_pull_request(
        "/repo".into(),
        "topic".into(),
        "head".into(),
        "main".into(),
    );
    v.pull_request_text_input("Title");
    v.pull_request_key("Enter", false, true);
    v.pull_request_key("Enter", false, true);
    let actions = v.take_actions();
    assert_eq!(actions.len(), 1);
    assert!(
        matches!(&actions[0],GitDiffViewAction::PullRequest{request:PullRequestRequest::Create{expected_head,..},..} if expected_head=="head")
    );
    v.apply_pull_request_reply(&PullRequestReply::Error {
        kind: PullRequestErrorKind::Conflict,
        message: "busy".into(),
    });
    assert!(v.has_pull_request_drafts());
    v.pull_request_key("Enter", false, true);
    assert_eq!(v.take_actions().len(), 1);
}
#[test]
fn created_transitions_and_reads() {
    let mut v = GitDiffView::create_pull_request(
        "/repo".into(),
        "topic".into(),
        "head".into(),
        "main".into(),
    );
    v.pull_request_text_input("Title");
    v.apply_pull_request_reply(&PullRequestReply::Created {
        pull_request: detail().summary,
    });
    assert_eq!(v.pull_request_number(), Some(7));
    assert!(!v.has_pull_request_drafts());
    assert!(matches!(
        v.take_actions().as_slice(),
        [GitDiffViewAction::PullRequest {
            request: PullRequestRequest::Read { number: 7 },
            ..
        }]
    ));
}
#[test]
fn refresh_preserves_drafts_head_change_stales() {
    let mut v = view();
    v.pr_intent(Intent::Review);
    v.pull_request_text_input("keep this");
    let mut next = detail();
    next.summary.head_sha = "new-head".into();
    v.apply_pull_request_reply(&PullRequestReply::Detail { detail: next });
    let p = v.pull_request.as_ref().unwrap();
    assert_eq!(p.fields[3], "keep this");
    assert!(p.stale);
    v.pr_intent(Intent::Submit);
    assert!(v.take_actions().is_empty());
    v.pr_intent(Intent::Discard);
    assert!(!v.has_pull_request_drafts());
}
#[test]
fn queued_review_outcome_preserves_on_error() {
    let mut v = view();
    let anchor = PullRequestInlineComment {
        path: "a.rs".into(),
        side: PullRequestSide::Left,
        line: 12,
        start_line: None,
        body: String::new(),
    };
    let p = v.pull_request.as_mut().unwrap();
    p.anchor = Some(anchor);
    p.focus_field(3);
    v.pull_request_text_input("fix this");
    v.pr_intent(Intent::Queue);
    v.pr_intent(Intent::Review);
    v.pr_intent(Intent::Event(PullRequestReviewEvent::RequestChanges));
    v.pr_intent(Intent::Submit);
    v.pr_intent(Intent::Submit);
    let a = v.take_actions();
    assert_eq!(a.len(), 1);
    assert!(
        matches!(&a[0],GitDiffViewAction::PullRequest{request:PullRequestRequest::SubmitReview{comments,event:PullRequestReviewEvent::RequestChanges,..},..} if comments[0].line==12)
    );
    v.apply_pull_request_reply(&PullRequestReply::Error {
        kind: PullRequestErrorKind::Permission,
        message: "denied".into(),
    });
    assert_eq!(v.pull_request.as_ref().unwrap().comments.len(), 1);
    v.pr_intent(Intent::Submit);
    v.take_actions();
    v.apply_pull_request_reply(&PullRequestReply::ReviewSubmitted { number: 7 });
    assert!(!v.has_pull_request_drafts());
}
#[test]
fn sheet_and_tabs_own_pointer_over_old_diff_hits() {
    let mut v = view();
    v.header_actions
        .push((0, false, Rect::new(0.0, 0.0, 200.0, 200.0), true));
    v.pr_intent(Intent::Tab(Tab::Conversation));
    assert!(v.pointer_down(30.0, 30.0));
    assert!(v.take_actions().is_empty());
    v.pr_intent(Intent::Review);
    assert!(v.pointer_down(30.0, 30.0));
    assert!(v.take_actions().is_empty());
}
#[test]
fn source_hit_retains_old_new_coordinates_across_wrap_fragments() {
    let mut v = view();
    for (x, side, line) in [
        (0.0, PullRequestSide::Left, 91),
        (40.0, PullRequestSide::Right, 105),
    ] {
        v.pr_line_hits.push((
            Rect::new(x, 10.0, 20.0, 54.0),
            PullRequestInlineComment {
                path: "a.rs".into(),
                side,
                line,
                start_line: None,
                body: String::new(),
            },
        ));
        for y in [11.0, 29.0, 47.0] {
            let a = v.pr_anchor_at(x + 1.0, y).unwrap();
            assert_eq!(a.side, side);
            assert_eq!(a.line, line);
        }
    }
    assert!(v.pr_anchor_at(100.0, 20.0).is_none());
}
#[test]
fn inline_draft_survives_cancel_and_discard_clears() {
    let mut v = view();
    v.pr_line_hits.push((
        Rect::new(0.0, 10.0, 20.0, 18.0),
        PullRequestInlineComment {
            path: "a.rs".into(),
            side: PullRequestSide::Right,
            line: 2,
            start_line: None,
            body: String::new(),
        },
    ));
    assert!(v.pointer_down(1.0, 11.0));
    v.pull_request_text_input("draft");
    v.pr_intent(Intent::Cancel);
    assert!(v.has_pull_request_drafts());
    v.pr_intent(Intent::Discard);
    assert!(!v.has_pull_request_drafts());
}
