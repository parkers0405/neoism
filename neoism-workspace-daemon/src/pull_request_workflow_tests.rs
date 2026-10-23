//! End-to-end local Git workflow tests; all remotes are local bare fixtures or mocked gh.
use super::tests::fixture;
use super::*;
use crate::pull_request::fixture_tests::{api, git_call, pr, setup};

fn status(root: &Path) -> String {
    git(root, &["status", "--porcelain=v1", "--untracked-files=all"]).unwrap()
}
fn expected_pr(p: &Prepared) -> Value {
    let mut value = pr(&p.head);
    value["base"]["repo"] = json!({"full_name":"base/repo"});
    value["head"]["repo"] = json!({"full_name":"head/repo"});
    value["head"]["ref"] = json!("original-pr");
    value
}
#[test]
fn worktree_keeps_source_status_and_all_existing_exclude_bytes_and_rules() {
    let (_dir, p) = fixture();
    let repo = Repository::open(&p.root).unwrap();
    let exclude = repo.commondir().join("info/exclude");
    let before=b"# user rules, non-UTF8 comment \xff\r\nignored-user.log\r\n!keep\r\n# no final newline";
    fs::write(&exclude, before).unwrap();
    fs::write(p.root.join("ignored-user.log"), "ignored by user").unwrap();
    fs::write(p.root.join("keep"), "visible").unwrap();
    let before_status = status(&p.root);
    add_worktree(&p).unwrap();
    assert_eq!(status(&p.root), before_status);
    let after = fs::read(&exclude).unwrap();
    assert!(after.starts_with(before));
    assert!(after.ends_with(b"\n/.neoism-pr-worktrees/\n"));
    assert!(!p.root.join(".gitignore").exists());
    ignore::managed_container(&p).unwrap();
    assert_eq!(
        fs::read(&exclude).unwrap(),
        after,
        "idempotent local-only rule"
    );
    assert!(!exclude.with_extension("lock").exists());
}
#[test]
fn source_remains_clean_and_current_checkout_works_after_managed_worktree_creation() {
    let (_dir, mut p) = fixture();
    add_worktree(&p).unwrap();
    assert!(status(&p.root).is_empty());
    // Use a fresh generated branch/remote for the next explicit current checkout.
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    p.branch = format!("neoism/pr-7-{nonce}");
    p.association.remote = format!("neoism-pr-7-{nonce}");
    let hub = crate::crdt::sync::CrdtSyncHub::default();
    hub.guarded_repository_checkout(&p.root, |paths| switch(&p, paths))
        .unwrap();
    assert!(status(&p.root).is_empty());
}
#[test]
fn linked_worktrees_use_common_exclude_including_nested_managed_worktrees() {
    let (_dir, p) = fixture();
    let repo = Repository::open(&p.root).unwrap();
    let exclude = repo.commondir().join("info/exclude");
    let before = fs::read(&exclude).unwrap();
    let PullRequestReply::CheckedOut { path, .. } = add_worktree(&p).unwrap() else {
        panic!("worktree")
    };
    let root = PathBuf::from(path);
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut association = p.association.clone();
    association.remote = format!("neoism-pr-7-{nonce}");
    let child = Prepared {
        root: root.clone(),
        workdir: root.clone(),
        head: p.head.clone(),
        number: 7,
        reference: format!("refs/neoism/pr-checkout/7/{nonce}"),
        branch: format!("neoism/pr-7-{nonce}"),
        association,
    };
    add_worktree(&child).unwrap();
    assert!(status(&p.root).is_empty());
    assert!(status(&root).is_empty());
    assert!(fs::read(&exclude).unwrap().starts_with(&before));
    assert_eq!(
        Repository::open(&root).unwrap().commondir(),
        repo.commondir()
    );
    assert!(root.join(".git").is_file());
}
#[test]
fn reserved_directory_collision_and_tracked_target_alias_never_get_silently_hidden() {
    let (_dir, p) = fixture();
    let repo = Repository::open(&p.root).unwrap();
    let exclude = repo.commondir().join("info/exclude");
    let before = fs::read(&exclude).unwrap();
    let container = p.root.join(".neoism-pr-worktrees");
    fs::create_dir(&container).unwrap();
    fs::write(container.join("precious"), "keep").unwrap();
    assert!(add_worktree(&p).is_err());
    assert_eq!(fs::read(&exclude).unwrap(), before);
    assert_eq!(
        fs::read_to_string(container.join("precious")).unwrap(),
        "keep"
    );
    fs::remove_file(container.join("precious")).unwrap();
    fs::remove_dir(&container).unwrap();
    let parent = repo
        .find_commit(git2::Oid::from_str(&p.head).unwrap())
        .unwrap();
    let tree = parent.tree().unwrap();
    let mut builder = repo.treebuilder(Some(&tree)).unwrap();
    builder
        .insert(".neoism-pr-worktrees", repo.blob(b"../").unwrap(), 0o120000)
        .unwrap();
    let tree = repo.find_tree(builder.write().unwrap()).unwrap();
    let sig = git2::Signature::now("fixture", "fixture@example.invalid").unwrap();
    let mut p = p;
    p.head = repo
        .commit(None, &sig, &sig, "reserved alias", &tree, &[&parent])
        .unwrap()
        .to_string();
    assert!(add_worktree(&p).is_err());
    assert_eq!(fs::read(&exclude).unwrap(), before);
    assert!(!container.exists());
}
#[test]
fn tracked_reserved_name_and_locked_exclude_are_never_overwritten() {
    let (_dir, p) = fixture();
    let repo = Repository::open(&p.root).unwrap();
    let exclude = repo.commondir().join("info/exclude");
    let before = fs::read(&exclude).unwrap();
    let path = p.root.join(".neoism-pr-worktrees");
    fs::write(&path, "tracked").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(".neoism-pr-worktrees")).unwrap();
    index.write().unwrap();
    assert!(add_worktree(&p).is_err());
    assert_eq!(fs::read(&exclude).unwrap(), before);
    assert_eq!(fs::read_to_string(&path).unwrap(), "tracked");
    index
        .remove_path(Path::new(".neoism-pr-worktrees"))
        .unwrap();
    index.write().unwrap();
    fs::remove_file(path).unwrap();
    let lock = repo.commondir().join("info/exclude.lock");
    fs::write(&lock, "another writer").unwrap();
    assert!(add_worktree(&p).is_err());
    assert_eq!(fs::read(&exclude).unwrap(), before);
    assert_eq!(fs::read_to_string(lock).unwrap(), "another writer");
    assert!(status(&p.root).is_empty());
}
fn local_commit(repo: &Repository, root: &Path, text: &str) -> git2::Oid {
    fs::write(root.join("note.md"), text).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("note.md")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let sig = git2::Signature::now("fixture", "fixture@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "edit PR", &tree, &[&parent])
        .unwrap()
}
fn push_fixture(p: &Prepared) -> (tempfile::TempDir, Repository, Repository) {
    let storage = tempfile::tempdir().unwrap();
    let fork = Repository::init_bare(storage.path().join("fork.git")).unwrap();
    let base = Repository::init_bare(storage.path().join("base.git")).unwrap();
    let repo = Repository::open(&p.root).unwrap();
    // The production route stays explicit github.com. A test-only Git URL rewrite
    // redirects ONLY this source repository to a local bare fixture, never a network.
    repo.config()
        .unwrap()
        .set_str(
            &format!("url.{}.insteadOf", fork.path().display()),
            "https://github.com/head/repo.git",
        )
        .unwrap();
    repo.remote("origin", base.path().to_str().unwrap())
        .unwrap();
    git(
        &p.root,
        &[
            "push",
            "--",
            fork.path().to_str().unwrap(),
            &format!("{}:refs/heads/original-pr", p.head),
        ],
    )
    .unwrap();
    (storage, fork, base)
}
#[test]
fn generated_current_branch_pushes_only_original_fork_branch_without_force() {
    let (_dir, p) = fixture();
    let (_storage, fork, base) = push_fixture(&p);
    switch(&p, &[]).unwrap();
    let repo = Repository::open(&p.root).unwrap();
    let commit = local_commit(&repo, &p.root, "edit for original PR\n");
    crate::pull_request::push_if_associated(&p.root)
        .unwrap()
        .unwrap();
    assert_eq!(
        fork.find_reference("refs/heads/original-pr")
            .unwrap()
            .target(),
        Some(commit)
    );
    assert!(fork
        .find_reference(&format!("refs/heads/{}", p.branch))
        .is_err());
    // The persisted refspec also protects ordinary/offline `git push` when
    // the generated local name differs from the original source branch.
    let ordinary = local_commit(&repo, &p.root, "second edit through ordinary Git\n");
    git(&p.root, &["push"]).unwrap();
    assert_eq!(
        fork.find_reference("refs/heads/original-pr")
            .unwrap()
            .target(),
        Some(ordinary)
    );
    assert!(fork
        .find_reference(&format!("refs/heads/{}", p.branch))
        .is_err());
    assert_eq!(base.references().unwrap().count(), 0);
    let push = repo
        .config()
        .unwrap()
        .get_string(&format!("remote.{}.push", p.association.remote))
        .unwrap();
    assert_eq!(
        push,
        format!("refs/heads/{}:refs/heads/original-pr", p.branch)
    );
    assert!(!push.starts_with('+'));
    assert_eq!(
        git(&p.root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).unwrap(),
        format!("{}/original-pr", p.association.remote)
    );
}
#[test]
fn pr_aware_push_refuses_non_fast_forward_and_never_creates_generated_remote_branch() {
    let (_dir, p) = fixture();
    let (_storage, fork, _base) = push_fixture(&p);
    switch(&p, &[]).unwrap();
    let repo = Repository::open(&p.root).unwrap();
    local_commit(&repo, &p.root, "local edit\n");
    let old = fork
        .find_commit(git2::Oid::from_str(&p.head).unwrap())
        .unwrap();
    let tree = old.tree().unwrap();
    let sig = git2::Signature::now("remote", "remote@example.invalid").unwrap();
    let remote = fork
        .commit(
            Some("refs/heads/original-pr"),
            &sig,
            &sig,
            "other collaborator",
            &tree,
            &[&old],
        )
        .unwrap();
    assert!(crate::pull_request::push_if_associated(&p.root)
        .unwrap()
        .is_err());
    assert_eq!(
        fork.find_reference("refs/heads/original-pr")
            .unwrap()
            .target(),
        Some(remote)
    );
    assert!(fork
        .find_reference(&format!("refs/heads/{}", p.branch))
        .is_err());
}
#[test]
fn created_worktree_discovers_original_pr_not_generated_branch_or_origin() {
    let (_dir, p) = fixture();
    let PullRequestReply::CheckedOut { path, .. } = add_worktree(&p).unwrap() else {
        panic!("worktree")
    };
    let root = PathBuf::from(path);
    let mut calls = setup();
    calls.extend([
        git_call(&["symbolic-ref","--quiet","--short","HEAD"],&p.branch),git_call(&["rev-parse","--verify",&format!("refs/heads/{}",p.branch)],&p.head),api("GET","repos/base/repo",None,json!({"default_branch":"main"})),
        api("GET","repos/base/repo/pulls/7",None,expected_pr(&p)),
        api("POST","graphql",Some(json!({"query":METADATA_QUERY,"variables":{"owner":"base","repo":"repo","number":7}})),json!({"data":{"repository":{"pullRequest":{"reviewDecision":"APPROVED","commits":{"nodes":[]}}}}})),
    ]);
    crate::pull_request::tests::fixture(calls, || {
        assert!(
            matches!(handle_at(&root,PullRequestRequest::Discover {branch:None}),PullRequestReply::Discovered {pull_request:Some(pr),..} if pr.number==7 && pr.head_branch=="original-pr")
        )
    });
}
#[test]
fn partial_associations_and_nested_pr_push_routes_fail_closed_without_processes() {
    let (_dir, p) = fixture();
    let repo = Repository::open(&p.root).unwrap();
    let local = repo.head().unwrap().shorthand().unwrap().to_string();
    repo.config()
        .unwrap()
        .set_str(
            &format!("branch.{local}.neoism-pr-association"),
            &serde_json::to_string(&p.association).unwrap(),
        )
        .unwrap();
    crate::pull_request::tests::fixture(Vec::new(), || {
        assert!(crate::pull_request::push_if_associated(&p.root)
            .unwrap()
            .is_err());
        assert!(crate::pull_request::is_associated_branch(&p.root, &local).is_err());
    });
    let (_dir, p) = fixture();
    switch(&p, &[]).unwrap();
    let nested = p.root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    crate::pull_request::tests::fixture(Vec::new(), || {
        assert!(crate::pull_request::push_if_associated(&nested)
            .unwrap()
            .unwrap_err()
            .contains("repository-root workspace"));
    });
}

#[test]
fn association_revalidates_source_identity_and_ref_and_refuses_deleted_heads() {
    let (_dir, p) = fixture();
    let base = Identity {
        owner: "base".into(),
        repo: "repo".into(),
    };
    let initial = expected_pr(&p);
    p.association.verify_pr(&base, &initial).unwrap();
    for key in ["repo", "ref"] {
        let mut changed = initial.clone();
        changed["head"][key] = if key == "repo" {
            json!({"full_name":"different/repo"})
        } else {
            json!("other-branch")
        };
        assert!(matches!(
            p.association.verify_pr(&base, &changed),
            Err((PullRequestErrorKind::Stale, _))
        ));
    }
    let mut deleted = initial;
    deleted["head"]["repo"] = Value::Null;
    assert!(matches!(
        p.association.verify_pr(&base, &deleted),
        Err((PullRequestErrorKind::Unsupported, _))
    ));
}
#[test]
fn setup_does_not_overwrite_user_remote_and_modified_push_routes_fail_before_processes() {
    let (_dir, p) = fixture();
    let repo = Repository::open(&p.root).unwrap();
    let target = repo
        .find_commit(git2::Oid::from_str(&p.head).unwrap())
        .unwrap();
    repo.branch(&p.branch, &target, false).unwrap();
    let key = format!("remote.{}.url", p.association.remote);
    repo.config()
        .unwrap()
        .set_str(&key, "https://github.com/user/precious.git")
        .unwrap();
    assert!(identity::configure(&repo, &p.branch, &p.head, &p.association).is_err());
    assert_eq!(
        repo.config().unwrap().get_string(&key).unwrap(),
        "https://github.com/user/precious.git"
    );
    let (_dir, p) = fixture();
    switch(&p, &[]).unwrap();
    let repo = Repository::open(&p.root).unwrap();
    repo.config()
        .unwrap()
        .set_str(
            &format!("remote.{}.push", p.association.remote),
            "+refs/heads/main:refs/heads/main",
        )
        .unwrap();
    crate::pull_request::tests::fixture(Vec::new(), || {
        assert!(crate::pull_request::push_if_associated(&p.root)
            .unwrap()
            .is_err())
    });
}
#[tokio::test]
async fn publish_cannot_create_an_unrelated_branch_for_a_pr_checkout() {
    let (_dir, p) = fixture();
    switch(&p, &[]).unwrap();
    let replies = crate::git::handle_with_root(
        p.root.clone(),
        neoism_protocol::git::GitClientMessage::Publish,
    )
    .await;
    assert!(
        matches!(&replies[..],[neoism_protocol::git::GitServerMessage::ChangedFiles {error:Some(message),..}] if message.contains("use Push"))
    );
}
