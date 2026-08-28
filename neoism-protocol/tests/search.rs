//! Wire-contract tests for daemon-backed file, grep, and git search.

use neoism_protocol::search::{
    SearchClientMessage, SearchDirectoryHit, SearchFileHit, SearchFileMode, SearchGitHit,
    SearchGitStatus, SearchGrepHit, SearchGrepMode, SearchServerMessage,
};

fn roundtrip_client(message: &SearchClientMessage) {
    let json = serde_json::to_string(message).expect("serialize client message");
    let decoded: SearchClientMessage =
        serde_json::from_str(&json).expect("deserialize client message");
    let decoded_json =
        serde_json::to_string(&decoded).expect("re-serialize client message");
    assert_eq!(json, decoded_json, "client wire shape changed: {json}");
}

fn roundtrip_server(message: &SearchServerMessage) {
    let json = serde_json::to_string(message).expect("serialize server message");
    let decoded: SearchServerMessage =
        serde_json::from_str(&json).expect("deserialize server message");
    let decoded_json =
        serde_json::to_string(&decoded).expect("re-serialize server message");
    assert_eq!(json, decoded_json, "server wire shape changed: {json}");
}

#[test]
fn client_requests_roundtrip() {
    roundtrip_client(&SearchClientMessage::CollectFiles {
        req_id: 1,
        cwd: "src".into(),
    });
    roundtrip_client(&SearchClientMessage::SearchFiles {
        req_id: 2,
        query: "protocol".into(),
        cwd: ".".into(),
        mode: SearchFileMode::Fuzzy,
    });
    roundtrip_client(&SearchClientMessage::SearchDirectories {
        req_id: 10,
        query: "proto".into(),
        cwd: ".".into(),
    });
    roundtrip_client(&SearchClientMessage::SearchFiles {
        req_id: 3,
        query: "src/lib.rs".into(),
        cwd: ".".into(),
        mode: SearchFileMode::Exact,
    });
    roundtrip_client(&SearchClientMessage::SearchGrep {
        req_id: 4,
        query: "Search.*Message".into(),
        cwd: "neoism-protocol".into(),
        mode: SearchGrepMode::Regex,
        case_sensitive: Some(true),
        file_patterns: vec!["*.rs".into()],
    });
    roundtrip_client(&SearchClientMessage::SearchGrep {
        req_id: 5,
        query: String::new(),
        cwd: ".".into(),
        mode: SearchGrepMode::Fuzzy,
        case_sensitive: None,
        file_patterns: Vec::new(),
    });
    roundtrip_client(&SearchClientMessage::SearchGrep {
        req_id: 6,
        query: "needle".into(),
        cwd: ".".into(),
        mode: SearchGrepMode::Exact,
        case_sensitive: Some(false),
        file_patterns: vec!["src/**".into(), "tests/**".into()],
    });
    roundtrip_client(&SearchClientMessage::SearchGitChanges {
        req_id: 7,
        cwd: ".".into(),
    });
    roundtrip_client(&SearchClientMessage::GitRepoRoot {
        req_id: 8,
        cwd: "neoism-protocol/tests".into(),
    });
    roundtrip_client(&SearchClientMessage::CancelSearch { req_id: 9 });
}

#[test]
fn server_results_roundtrip() {
    roundtrip_server(&SearchServerMessage::CollectFilesResult {
        req_id: 1,
        paths: vec!["src/lib.rs".into(), "tests/search.rs".into()],
    });
    roundtrip_server(&SearchServerMessage::SearchFilesResult {
        req_id: 2,
        hits: vec![SearchFileHit {
            score: -17,
            path: "src/search.rs".into(),
        }],
    });
    roundtrip_server(&SearchServerMessage::SearchDirectoriesResult {
        req_id: 9,
        hits: vec![SearchDirectoryHit {
            score: 275,
            path: "neoism-protocol/tests".into(),
        }],
    });
    roundtrip_server(&SearchServerMessage::SearchGrepResult {
        req_id: 3,
        hits: vec![SearchGrepHit {
            score: 42,
            path: "src/search.rs".into(),
            line: 85,
            column: 5,
            text: "pub enum SearchClientMessage".into(),
        }],
    });
    roundtrip_server(&SearchServerMessage::SearchGitChangesResult {
        req_id: 4,
        hits: vec![SearchGitHit {
            path: "tests/search.rs".into(),
            status: SearchGitStatus::Untracked,
            line: 1,
            text: "new test".into(),
        }],
    });
    roundtrip_server(&SearchServerMessage::GitRepoRootResult {
        req_id: 5,
        path: Some("/workspace/neoism".into()),
    });
    roundtrip_server(&SearchServerMessage::GitRepoRootResult {
        req_id: 6,
        path: None,
    });
    roundtrip_server(&SearchServerMessage::SearchProgress {
        req_id: 7,
        found_so_far: u64::MAX,
    });
    roundtrip_server(&SearchServerMessage::SearchError {
        req_id: 8,
        message: "cancelled".into(),
    });
}

#[test]
fn git_status_names_are_stable() {
    let statuses = [
        (SearchGitStatus::Modified, "Modified"),
        (SearchGitStatus::Staged, "Staged"),
        (SearchGitStatus::Mixed, "Mixed"),
        (SearchGitStatus::Added, "Added"),
        (SearchGitStatus::Deleted, "Deleted"),
        (SearchGitStatus::Renamed, "Renamed"),
        (SearchGitStatus::Untracked, "Untracked"),
        (SearchGitStatus::Conflict, "Conflict"),
    ];

    for (status, expected) in statuses {
        let json = serde_json::to_string(&status).expect("serialize git status");
        assert_eq!(json, format!(r#""{expected}""#));
    }
}

#[test]
fn request_id_and_variant_tag_have_expected_json_shape() {
    let json = serde_json::to_string(&SearchClientMessage::CancelSearch {
        req_id: 9_007_199_254_740_991,
    })
    .expect("serialize cancellation");

    assert_eq!(json, r#"{"CancelSearch":{"req_id":9007199254740991}}"#);
}
