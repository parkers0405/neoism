//! Roundtrip every git-protocol variant through serde_json.

use neoism_protocol::git::{
    CommitSummary, DiffHunk, GitClientMessage, GitFileStatus, GitServerMessage,
    GitStatusEntry,
};

fn roundtrip_client(msg: &GitClientMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: GitClientMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

fn roundtrip_server(msg: &GitServerMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: GitServerMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

#[test]
fn apply_block_wire_and_mutation_contract() {
    use neoism_protocol::git::GitChangeBlock;
    let block = GitChangeBlock { old_start: 0, new_start: 0, old_lines: vec![], new_lines: vec!["insert".into(), "".into()] };
    for restore in [false, true] {
        let message = GitClientMessage::ApplyBlock { path: "literal[*].rs".into(), block: block.clone(), restore };
        assert!(message.is_mutation());
        roundtrip_client(&message);
        let decoded: GitClientMessage = serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert!(matches!(decoded, GitClientMessage::ApplyBlock { block: b, restore: r, .. } if b == block && r == restore));
    }
}

#[test]
fn client_status_roundtrip() {
    roundtrip_client(&GitClientMessage::Status);
}

#[test]
fn client_diff_roundtrip() {
    roundtrip_client(&GitClientMessage::Diff {
        path: Some("src/lib.rs".into()),
    });
    roundtrip_client(&GitClientMessage::Diff { path: None });
}

#[test]
fn client_log_roundtrip() {
    roundtrip_client(&GitClientMessage::Log {
        max_count: Some(20),
    });
    roundtrip_client(&GitClientMessage::Log { max_count: None });
}

#[test]
fn server_status_roundtrip() {
    roundtrip_server(&GitServerMessage::Status {
        entries: vec![
            GitStatusEntry {
                path: "src/lib.rs".into(),
                status: GitFileStatus::Modified,
            },
            GitStatusEntry {
                path: "new.rs".into(),
                status: GitFileStatus::Untracked,
            },
            GitStatusEntry {
                path: "old.rs".into(),
                status: GitFileStatus::Deleted,
            },
            GitStatusEntry {
                path: "added.rs".into(),
                status: GitFileStatus::Added,
            },
            GitStatusEntry {
                path: "renamed.rs".into(),
                status: GitFileStatus::Renamed,
            },
            GitStatusEntry {
                path: "conflict.rs".into(),
                status: GitFileStatus::Conflicted,
            },
        ],
    });
}

#[test]
fn server_diff_roundtrip() {
    roundtrip_server(&GitServerMessage::Diff {
        hunks: vec![DiffHunk {
            path: "src/lib.rs".into(),
            old_start: 10,
            old_lines: 2,
            new_start: 10,
            new_lines: 3,
            patch: "@@ -10,2 +10,3 @@\n-old\n+new1\n+new2\n".into(),
        }],
    });
    roundtrip_server(&GitServerMessage::Diff { hunks: Vec::new() });
}

#[test]
fn server_log_roundtrip() {
    roundtrip_server(&GitServerMessage::Log {
        commits: vec![CommitSummary {
            sha: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into(),
            short_sha: "deadbee".into(),
            author: "Test <test@example.com>".into(),
            message: "fix: bug".into(),
            timestamp: 1_700_000_000,
        }],
    });
}

#[test]
fn server_error_roundtrip() {
    roundtrip_server(&GitServerMessage::Error {
        message: "not a repo".into(),
    });
}

#[test]
fn server_branch_roundtrip() {
    roundtrip_server(&GitServerMessage::Branch {
        name: Some("main".into()),
    });
    roundtrip_server(&GitServerMessage::Branch { name: None });
}

#[test]
fn server_changes_roundtrip() {
    roundtrip_server(&GitServerMessage::Changes {
        added: 0,
        deleted: 0,
    });
    roundtrip_server(&GitServerMessage::Changes {
        added: 12,
        deleted: 3,
    });
}

#[test]
fn server_changes_json_shape() {
    let msg = GitServerMessage::Changes {
        added: 4,
        deleted: 1,
    };
    let json = serde_json::to_string(&msg).expect("serialize");
    assert_eq!(json, r#"{"Changes":{"added":4,"deleted":1}}"#);
}

#[test]
fn git_file_status_variants_distinct() {
    use GitFileStatus::*;
    let all = [Modified, Added, Deleted, Renamed, Untracked, Conflicted];
    for s in &all {
        let json = serde_json::to_string(s).expect("serialize");
        let back: GitFileStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*s, back);
    }
}
