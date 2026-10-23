//! Offline transport fixtures: any unexpected command panics before spawning.
use super::tests::{fixture, Call};
use super::*;
fn handle_fixture(request: PullRequestRequest) -> PullRequestReply {
    let root = tempfile::tempdir().unwrap();
    git2::Repository::init(root.path()).unwrap();
    handle_at(root.path(), request)
}
pub(super) fn git_call(args: &[&str], output: &str) -> Call {
    Call {
        program: "git",
        args: args.iter().map(|s| s.to_string()).collect(),
        input: None,
        result: Ok(output.into()),
    }
}
pub(super) fn api(
    method: &str,
    endpoint: &str,
    input: Option<Value>,
    output: Value,
) -> Call {
    let mut args = vec![
        "api".into(),
        "--hostname".into(),
        "github.com".into(),
        "--method".into(),
        method.into(),
        endpoint.into(),
    ];
    if input.is_some() {
        args.extend(["--input".into(), "-".into()]);
    }
    Call {
        program: "gh",
        args,
        input,
        result: Ok(output.to_string()),
    }
}
pub(super) fn setup() -> Vec<Call> {
    vec![
        git_call(
            &["remote", "get-url", "origin"],
            "git@github.com:fork/repo.git",
        ),
        git_call(
            &["remote", "get-url", "upstream"],
            "https://github.com/base/repo.git",
        ),
        api("GET", "repos/base/repo", None, json!({"fork":false})),
    ]
}
pub(super) fn pr(head: &str) -> Value {
    json!({"number":7,"title":"Example","state":"open","head":{"sha":head,"ref":"topic"},"base":{"sha":"b".repeat(40),"ref":"main"}})
}
fn thread_call(threads: Value) -> Call {
    api(
        "POST",
        "graphql",
        Some(
            json!({"query":THREAD_QUERY,"variables":{"owner":"base","repo":"repo","number":7}}),
        ),
        json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"pageInfo":{"hasNextPage":false},"nodes":threads}}}}}),
    )
}
#[test]
fn thread_ids_are_repository_and_pr_scoped_before_reply_or_resolve() {
    for request in [
        PullRequestRequest::ReplyThread {
            number: 7,
            thread_id: "foreign".into(),
            body: "Reply".into(),
        },
        PullRequestRequest::ResolveThread {
            number: 7,
            thread_id: "foreign".into(),
            resolved: true,
        },
    ] {
        let mut calls = setup();
        calls.push(thread_call(json!([{ "id":"owned", "path":"file", "diffSide":"RIGHT", "line":1, "comments":{"nodes":[],"pageInfo":{"hasNextPage":false}} }])));
        fixture(calls, || {
            assert!(matches!(
                handle_fixture(request),
                PullRequestReply::Error {
                    kind: PullRequestErrorKind::Invalid,
                    ..
                }
            ))
        });
    }
}
#[test]
fn immutable_detail_uses_pr_comparison_not_worktree_and_reports_omitted_patches() {
    let head = "a".repeat(40);
    let mut calls = setup();
    calls.extend([api("GET","repos/base/repo/pulls/7",None,pr(&head)),api("GET","repos/base/repo/pulls/7/files?per_page=100&page=1",None,json!([{"filename":"binary.png","status":"modified","additions":0,"deletions":0}])),api("GET","repos/base/repo/issues/7/comments?per_page=100",None,json!([])),api("GET","repos/base/repo/pulls/7/reviews?per_page=100",None,json!([])),thread_call(json!([])),api("GET",&format!("repos/base/repo/commits/{head}/check-runs?per_page=100"),None,json!({"total_count":0,"check_runs":[]})),api("GET",&format!("repos/base/repo/commits/{head}/status?per_page=100"),None,json!({"total_count":0,"statuses":[]})),api("GET","repos/base/repo/pulls/7",None,pr(&head)),api("POST","graphql",Some(json!({"query":METADATA_QUERY,"variables":{"owner":"base","repo":"repo","number":7}})),json!({"data":{"repository":{"pullRequest":{"reviewDecision":"APPROVED","commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}}}}}))]);
    calls.push(api("GET", "repos/base/repo/pulls/7", None, pr(&head)));
    fixture(calls, || {
        assert!(
            matches!(handle_fixture(PullRequestRequest::Read { number:7 }),PullRequestReply::Detail { detail } if detail.truncated && detail.files.len()==1 && detail.summary.head_sha==head && detail.summary.review_decision=="APPROVED" && detail.summary.checks_state=="SUCCESS")
        )
    });
}

#[test]
fn detail_rejects_head_or_base_changes_during_mutable_number_keyed_reads() {
    for changed_base in [false, true] {
        let head = "a".repeat(40);
        let mut changed = pr(&head);
        if changed_base {
            changed["base"]["sha"] = json!("c".repeat(40));
        } else {
            changed["head"]["sha"] = json!("c".repeat(40));
        }
        let mut calls = setup();
        calls.extend([
            api("GET", "repos/base/repo/pulls/7", None, pr(&head)),
            api(
                "GET",
                "repos/base/repo/pulls/7/files?per_page=100&page=1",
                None,
                json!([]),
            ),
            api(
                "GET",
                "repos/base/repo/issues/7/comments?per_page=100",
                None,
                json!([]),
            ),
            api(
                "GET",
                "repos/base/repo/pulls/7/reviews?per_page=100",
                None,
                json!([]),
            ),
            thread_call(json!([])),
            api(
                "GET",
                &format!("repos/base/repo/commits/{head}/check-runs?per_page=100"),
                None,
                json!({"total_count":0,"check_runs":[]}),
            ),
            api(
                "GET",
                &format!("repos/base/repo/commits/{head}/status?per_page=100"),
                None,
                json!({"total_count":0,"statuses":[]}),
            ),
            api("GET", "repos/base/repo/pulls/7", None, changed),
        ]);
        fixture(calls, || {
            assert!(matches!(
                handle_fixture(PullRequestRequest::Read { number: 7 }),
                PullRequestReply::Error {
                    kind: PullRequestErrorKind::Stale,
                    ..
                }
            ))
        });
    }
}

#[test]
fn permissions_do_not_change_legacy_git() {
    use neoism_protocol::{git::GitClientMessage, pairing::Permission};
    assert_eq!(
        crate::git::required_permission(&GitClientMessage::Commit {
            message: "legacy".into()
        }),
        Permission::WriteFiles
    );
    assert_eq!(
        crate::git::required_permission(&GitClientMessage::PullRequest {
            request: PullRequestRequest::Read { number: 7 }
        }),
        Permission::ReadFiles
    );
    assert_eq!(
        crate::git::required_permission(&GitClientMessage::PullRequest {
            request: PullRequestRequest::Merge {
                number: 7,
                expected_head: "a".repeat(40),
                method: PullRequestMergeMethod::Merge
            }
        }),
        Permission::GitWrite
    );
}

#[test]
fn discover_no_pr_is_success_not_auth_or_missing_cli() {
    let mut calls = setup();
    calls.extend([
        git_call(&["symbolic-ref", "--quiet", "--short", "HEAD"], "topic"),
        git_call(
            &["rev-parse", "--verify", "refs/heads/topic"],
            &"a".repeat(40),
        ),
        api(
            "GET",
            "repos/base/repo",
            None,
            json!({"default_branch":"main"}),
        ),
        api(
            "GET",
            "repos/base/repo/pulls?state=open&per_page=100&head=fork%3Atopic",
            None,
            json!([]),
        ),
    ]);
    fixture(calls, || {
        assert!(
            matches!(handle_fixture(PullRequestRequest::Discover { branch:None }),PullRequestReply::Discovered { pull_request:None,default_branch,.. } if default_branch == "main")
        )
    });
}
#[test]
fn authentication_and_cli_missing_remain_typed() {
    for kind in [
        PullRequestErrorKind::Authentication,
        PullRequestErrorKind::Unavailable,
    ] {
        let mut calls = setup();
        calls.last_mut().unwrap().result = Err(error(kind, "safe diagnostic"));
        fixture(calls, || {
            assert!(
                matches!(handle_fixture(PullRequestRequest::Read { number:7 }),PullRequestReply::Error { kind:actual,.. } if actual == kind)
            )
        });
    }
}
#[test]
fn stale_merge_never_mutates() {
    let mut calls = setup();
    calls.push(api(
        "GET",
        "repos/base/repo/pulls/7",
        None,
        pr(&"b".repeat(40)),
    ));
    fixture(calls, || {
        assert!(matches!(
            handle_fixture(PullRequestRequest::Merge {
                number: 7,
                expected_head: "a".repeat(40),
                method: PullRequestMergeMethod::Squash
            }),
            PullRequestReply::Error {
                kind: PullRequestErrorKind::Stale,
                ..
            }
        ))
    });
}
#[test]
fn merge_is_expected_sha_and_server_policy_only() {
    let head = "a".repeat(40);
    let mut calls = setup();
    calls.extend([
        api("GET", "repos/base/repo/pulls/7", None, pr(&head)),
        api(
            "PUT",
            "repos/base/repo/pulls/7/merge",
            Some(json!({"sha":head,"merge_method":"squash"})),
            json!({"merged":false}),
        ),
    ]);
    fixture(calls, || {
        assert!(matches!(
            handle_fixture(PullRequestRequest::Merge {
                number: 7,
                expected_head: head.clone(),
                method: PullRequestMergeMethod::Squash
            }),
            PullRequestReply::Error {
                kind: PullRequestErrorKind::Conflict,
                ..
            }
        ))
    });
}
#[test]
fn inline_review_is_one_atomic_submission_and_head_pinned() {
    let head = "a".repeat(40);
    let mut calls = setup();
    calls.extend([api("GET","repos/base/repo/pulls/7",None,pr(&head)),api("GET","repos/base/repo/pulls/7/files?per_page=100&page=1",None,json!([{"filename":"file.rs","patch":"@@ -1 +1 @@\n-old\n+new"}])),api("GET","repos/base/repo/pulls/7",None,pr(&head)),api("POST","repos/base/repo/pulls/7/reviews",Some(json!({"commit_id":head,"body":"Review","event":"COMMENT","comments":[{"path":"file.rs","side":"RIGHT","line":1,"body":"Inline"}]})),json!({"id":1}))]);
    fixture(calls, || {
        assert!(matches!(
            handle_fixture(PullRequestRequest::SubmitReview {
                number: 7,
                expected_head: head.clone(),
                body: "Review".into(),
                event: PullRequestReviewEvent::Comment,
                comments: vec![PullRequestInlineComment {
                    path: "file.rs".into(),
                    side: PullRequestSide::Right,
                    line: 1,
                    start_line: None,
                    body: "Inline".into()
                }]
            }),
            PullRequestReply::ReviewSubmitted { number: 7 }
        ))
    });
}
#[test]
fn invalid_inline_path_cannot_post_review() {
    let head = "a".repeat(40);
    let mut calls = setup();
    calls.extend([
        api("GET", "repos/base/repo/pulls/7", None, pr(&head)),
        api(
            "GET",
            "repos/base/repo/pulls/7/files?per_page=100&page=1",
            None,
            json!([]),
        ),
    ]);
    fixture(calls, || {
        assert!(matches!(
            handle_fixture(PullRequestRequest::SubmitReview {
                number: 7,
                expected_head: head.clone(),
                body: "Review".into(),
                event: PullRequestReviewEvent::Approve,
                comments: vec![PullRequestInlineComment {
                    path: "../other".into(),
                    side: PullRequestSide::Right,
                    line: 1,
                    start_line: None,
                    body: "Inline".into()
                }]
            }),
            PullRequestReply::Error {
                kind: PullRequestErrorKind::Invalid,
                ..
            }
        ))
    });
}
#[test]
fn creation_requires_upstream_proof_and_never_pushes() {
    let head = "a".repeat(40);
    let mut calls = setup();
    calls.extend([
        git_call(&["symbolic-ref", "--quiet", "--short", "HEAD"], "topic"),
        git_call(&["rev-parse", "HEAD"], &head),
        git_call(
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
            "origin/wrong",
        ),
        git_call(&["config", "--get", "branch.topic.remote"], "origin"),
    ]);
    fixture(calls, || {
        assert!(matches!(
            handle_fixture(PullRequestRequest::Create {
                title: "New PR".into(),
                body: String::new(),
                base: "main".into(),
                draft: false,
                expected_branch: "topic".into(),
                expected_head: head.clone()
            }),
            PullRequestReply::Error {
                kind: PullRequestErrorKind::Conflict,
                ..
            }
        ))
    });
}
