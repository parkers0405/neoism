use super::*;

fn commit(repo: &Repository, message: &str, parent: Option<git2::Oid>) -> git2::Oid {
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("sample.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::new("Ada Example", "ada@example.test", &git2::Time::new(1_700_000_000, 330)).unwrap();
    let parent = parent.map(|id| repo.find_commit(id).unwrap());
    let parents: Vec<_> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).unwrap()
}

#[test]
fn commit_read_is_immutable_root_and_first_parent_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("sample.txt"), "first\n").unwrap();
    let root = commit(&repo, "release: v1\n\nFull body", None);
    std::fs::write(dir.path().join("sample.txt"), "second\n").unwrap();
    let head = commit(&repo, "next", Some(root));
    std::fs::write(dir.path().join("sample.txt"), "unstaged, not historical\n").unwrap();
    let detail = read_commit(&repo, &root.to_string()).unwrap();
    assert_eq!(detail.author, "Ada Example");
    assert_eq!(detail.email, "ada@example.test");
    assert_eq!(detail.offset_minutes, 330);
    assert_eq!(detail.message, "release: v1\n\nFull body");
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.files[0].additions, 1);
    assert!(detail.diffs[0].patch.contains("+first"));
    assert!(!detail.diffs[0].patch.contains("unstaged"));
    let next = read_commit(&repo, "HEAD").unwrap();
    assert_eq!(next.sha, head.to_string());
    assert!(next.diffs[0].patch.contains("-first"));
    assert!(next.diffs[0].patch.contains("+second"));
    let request = GitClientMessage::ReadCommit { revision: root.to_string() };
    assert!(!request.is_mutation());
    assert!(matches!(required_permission(&request), Permission::ReadFiles));
    assert!(matches!(&handle_blocking(dir.path(), request)[0], GitServerMessage::CommitDetail { detail } if detail.sha == root.to_string()));
    assert_eq!(std::fs::read_to_string(dir.path().join("sample.txt")).unwrap(), "unstaged, not historical\n");
}

#[test]
fn commit_read_rejects_options_missing_objects_and_non_commits() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("sample.txt"), "first\n").unwrap();
    commit(&repo, "root", None);
    for revision in ["", "--output=/tmp/injected", "-p", "does-not-exist", "HEAD^{tree}", "HEAD:sample.txt", "HEAD\0junk"] {
        assert!(read_commit(&repo, revision).is_err(), "accepted {revision:?}");
    }
}
