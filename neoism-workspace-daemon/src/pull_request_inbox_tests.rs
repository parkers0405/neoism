use super::*;
use crate::pull_request::{
    fixture_tests::{api, pr},
    tests::fixture,
};
fn host() -> Host<'static> {
    Host {
        root: Path::new("/unused"),
        repo: Identity {
            owner: "base".into(),
            repo: "repo".into(),
        },
        origin: Identity {
            owner: "base".into(),
            repo: "repo".into(),
        },
    }
}
fn node() -> Value {
    json!({"number":7,"title":"Fix","url":"https://github.com/base/repo/pull/7","author":{"login":"viewer"},"state":"OPEN","headRefOid":"a".repeat(40),"baseRefOid":"b".repeat(40),"repository":{"nameWithOwner":"base/repo"},"additions":42,"deletions":3,"labels":{"nodes":[{"name":"bug"}]},"reviewRequests":{"nodes":[{"requestedReviewer":{"login":"viewer"}}]},"assignees":{"nodes":[{"login":"viewer"}]},"reviewDecision":"APPROVED","commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}})
}
#[test]
fn enriched_read_rechecks_both_revisions_after_all_metadata_reads() {
    for race in [
        "none",
        "metadata-head",
        "metadata-base",
        "final-head",
        "final-base",
    ] {
        let head = "a".repeat(40);
        let mut calls = vec![
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
            api(
                "POST",
                "graphql",
                Some(
                    json!({"query":THREAD_QUERY,"variables":{"owner":"base","repo":"repo","number":7}}),
                ),
                json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}}}),
            ),
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
            api("GET", "repos/base/repo/pulls/7", None, pr(&head)),
            api(
                "POST",
                "graphql",
                Some(
                    json!({"query":METADATA_QUERY,"variables":{"owner":"base","repo":"repo","number":7}}),
                ),
                json!({"data":{"repository":{"pullRequest":{"reviewDecision":"APPROVED","commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}}}}}),
            ),
            api("GET", "repos/base/repo/pulls/7", None, pr(&head)),
        ];
        let mut metadata = node();
        if race == "metadata-head" {
            metadata["headRefOid"] = json!("c".repeat(40));
        }
        if race == "metadata-base" {
            metadata["baseRefOid"] = json!("c".repeat(40));
        }
        let query = format!("query($owner:String!,$repo:String!,$number:Int!){{viewer{{login}} repository(owner:$owner,name:$repo){{pullRequest(number:$number){{{FIELDS}}}}}}}");
        calls.push(api("POST", "graphql", Some(json!({"query":query,"variables":{"owner":"base","repo":"repo","number":7}})), json!({"data":{"viewer":{"login":"viewer"},"repository":{"pullRequest":metadata}}})));
        let mut final_pr = pr(&head);
        if race == "final-head" {
            final_pr["head"]["sha"] = json!("c".repeat(40));
        }
        if race == "final-base" {
            final_pr["base"]["sha"] = json!("c".repeat(40));
        }
        calls.push(api("GET", "repos/base/repo/pulls/7", None, final_pr));
        fixture(calls, || {
            let result = read(&host(), 7);
            if race == "none" {
                let reply = result.unwrap();
                let encoded = serde_json::to_string(&reply).unwrap();
                assert!(
                    matches!(serde_json::from_str::<PullRequestReply>(&encoded).unwrap(), PullRequestReply::InboxDetail { detail, item } if detail.summary.head_sha == item.summary.head_sha && item.additions == 42)
                );
            } else {
                assert!(
                    matches!(result, Err((PullRequestErrorKind::Stale, _))),
                    "{race}"
                );
            }
        });
    }
}

#[test]
fn scope_syntax_and_input_bounds_fail_without_network() {
    for q in [
        "repo:other/repo",
        "org:other",
        "user:other",
        "is:issue",
        "type:issue",
        "sort:created",
        "in:body",
        "fix OR bug",
        "fix or bug",
        "-repo:x/y",
        "foo\" repo:x/y",
        "(foo)",
        "foo\\",
        "label:bug:repo:x/y",
        "https://evil.test",
        "a\0b",
    ] {
        assert!(query_text(q).is_err(), "{q}");
    }
    assert!(query_text(&"a".repeat(257)).is_err());
    assert_eq!(
        query_text("日本語 label:不具合").unwrap(),
        "\"日本語\" label:\"不具合\""
    );
    assert_eq!(
        query_text("fix label:bug").unwrap(),
        "\"fix\" label:\"bug\""
    );
    fixture(vec![], || {
        for cursor in ["", "bad cursor", "https://evil.test", &"a".repeat(257)] {
            assert!(page(
                &host(),
                "",
                PullRequestSort::Updated,
                PullRequestFilter::Open,
                Some(cursor.into()),
                0
            )
            .is_err());
        }
    });
}
#[test]
fn list_sort_filter_cursor_and_metadata_are_real_and_repository_scoped() {
    for (sort, order) in [
        (PullRequestSort::Updated, "updated-desc"),
        (PullRequestSort::Newest, "created-desc"),
        (PullRequestSort::Oldest, "created-asc"),
    ] {
        for (filter, state) in [
            (PullRequestFilter::Open, "is:open"),
            (PullRequestFilter::Closed, "is:closed is:unmerged"),
            (PullRequestFilter::Merged, "is:merged"),
            (PullRequestFilter::All, ""),
        ] {
            let query = format!("query($q:String!,$cursor:String){{viewer{{login}} search(query:$q,type:ISSUE,first:30,after:$cursor){{issueCount pageInfo{{hasNextPage endCursor}} nodes{{... on PullRequest{{{FIELDS}}}}}}}}}");
            let q = format!(
                "repo:base/repo is:pr {state} sort:{order} \"fix\" label:\"bug\""
            );
            fixture(
                vec![api(
                    "POST",
                    "graphql",
                    Some(json!({"query":query,"variables":{"q":q,"cursor":"abc="}})),
                    json!({"data":{"viewer":{"login":"viewer"},"search":{"issueCount":99,"nodes":[node()],"pageInfo":{"hasNextPage":true,"endCursor":"next="}}}}),
                )],
                || {
                    let PullRequestReply::Listed { page } = page(
                        &host(),
                        "fix label:bug",
                        sort,
                        filter,
                        Some("abc=".into()),
                        77,
                    )
                    .unwrap() else {
                        panic!()
                    };
                    assert_eq!(page.repository, "base/repo");
                    assert_eq!(page.viewer, "viewer");
                    assert_eq!(page.generation, 77);
                    assert_eq!(page.total_count, 99);
                    assert_eq!(page.next_cursor.as_deref(), Some("next="));
                    let item = &page.items[0];
                    assert!(item.authored && item.review_requested && item.assigned);
                    assert_eq!(item.additions, 42);
                    assert_eq!(item.deletions, 3);
                    assert_eq!(item.labels, ["bug"]);
                    assert_eq!(item.reviewers, ["viewer"]);
                    assert_eq!(item.summary.checks_state, "SUCCESS");
                    assert_eq!(item.summary.review_decision, "APPROVED");
                    let encoded = serde_json::to_string(&page).unwrap();
                    assert_eq!(
                        serde_json::from_str::<PullRequestPage>(&encoded).unwrap(),
                        page
                    );
                },
            );
        }
    }
}
#[test]
fn page_bounds_and_invalid_server_cursors_are_rejected() {
    let query = format!("query($q:String!,$cursor:String){{viewer{{login}} search(query:$q,type:ISSUE,first:30,after:$cursor){{issueCount pageInfo{{hasNextPage endCursor}} nodes{{... on PullRequest{{{FIELDS}}}}}}}}}");
    for (nodes, cursor, has_next) in [
        (vec![node(); 31], json!(null), false),
        (vec![], json!("bad cursor"), true),
        (vec![], json!(null), true),
    ] {
        fixture(
            vec![api(
                "POST",
                "graphql",
                Some(
                    json!({"query":query,"variables":{"q":"repo:base/repo is:pr is:open sort:updated-desc ","cursor":null}}),
                ),
                json!({"data":{"viewer":{"login":"viewer"},"search":{"nodes":nodes,"pageInfo":{"hasNextPage":has_next,"endCursor":cursor}}}}),
            )],
            || {
                assert!(page(
                    &host(),
                    "",
                    PullRequestSort::Updated,
                    PullRequestFilter::Open,
                    None,
                    0
                )
                .is_err());
            },
        );
    }
}

#[test]
fn foreign_or_invalid_metadata_is_not_returned() {
    let mut n = node();
    n["repository"]["nameWithOwner"] = json!("foreign/repo");
    assert!(item(&host(), &n, "viewer").is_err());
    n = node();
    n["headRefOid"] = json!("HEAD");
    assert!(item(&host(), &n, "viewer").is_err());
    let n = node();
    let other = item(&host(), &n, "other").unwrap();
    assert!(!other.authored && !other.review_requested && !other.assigned);
}
#[test]
fn inbox_requests_roundtrip_and_require_only_read_permission() {
    use neoism_protocol::{git::GitClientMessage, pairing::Permission};
    for request in [
        PullRequestRequest::ReadInbox { number: 7 },
        PullRequestRequest::List {
            query: "fix".into(),
            sort: PullRequestSort::Newest,
            filter: PullRequestFilter::All,
            cursor: None,
            generation: 9,
        },
    ] {
        assert!(!request.is_mutation());
        let msg = GitClientMessage::PullRequest { request };
        assert!(!msg.is_mutation());
        assert_eq!(crate::git::required_permission(&msg), Permission::ReadFiles);
        let decoded: GitClientMessage =
            serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(msg).unwrap()
        );
    }
}
