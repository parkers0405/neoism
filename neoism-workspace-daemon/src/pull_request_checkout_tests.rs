//! Real local Git fixtures. No network, gh invocation, or hosted mutations.
use super::*;
use crate::crdt::{crdt_buffer_id_for_path, sync::CrdtSyncHub};
use neoism_protocol::crdt::{CrdtBufferEdit, CrdtServerMessage};

pub(super) fn fixture() -> (tempfile::TempDir, Prepared) {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::path::canonicalize(dir.path()).unwrap();
    let repo = Repository::init(&root).unwrap();
    fs::write(root.join("note.md"), "old\n").unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("note.md")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Offline fixture", "fixture@example.invalid").unwrap();
    let base = repo
        .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
        .unwrap();
    fs::write(root.join("note.md"), "new PR text\n").unwrap();
    index.add_path(Path::new("note.md")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let head = repo
        .commit(
            None,
            &sig,
            &sig,
            "PR",
            &tree,
            &[&repo.find_commit(base).unwrap()],
        )
        .unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let reference = format!("refs/neoism/pr-checkout/7/{nonce}");
    repo.reference(&reference, head, false, "fixture pin")
        .unwrap();
    let prepared = Prepared {
        root: root.clone(),
        workdir: root,
        head: head.to_string(),
        number: 7,
        reference,
        branch: format!("neoism/pr-7-{nonce}"),
        association: identity::Association {
            version: 1,
            number: 7,
            base: "base/repo".into(),
            head_repo: "head/repo".into(),
            head_ref: "original-pr".into(),
            remote: format!("neoism-pr-7-{nonce}"),
        },
    };
    (dir, prepared)
}
fn hub(prepared: &Prepared) -> (CrdtSyncHub, String) {
    let hub = CrdtSyncHub::default();
    let id = crdt_buffer_id_for_path(&prepared.root.join("note.md"));
    hub.open_buffer(&id, "stale cache");
    (hub, id)
}
#[test]
fn fetched_ref_must_match_the_full_expected_commit_before_any_checkout() {
    let (_dir, p) = fixture();
    verify_fetched(&p).unwrap();
    let repo = Repository::open(&p.root).unwrap();
    let before = repo.head().unwrap().target().unwrap();
    repo.reference(&p.reference, before, true, "simulate PR head race")
        .unwrap();
    assert!(matches!(
        verify_fetched(&p),
        Err((PullRequestErrorKind::Stale, _))
    ));
    assert_eq!(repo.head().unwrap().target().unwrap(), before);
    assert_eq!(fs::read_to_string(p.root.join("note.md")).unwrap(), "old\n");
}

#[test]
fn clean_current_checkout_switches_exact_head_and_publishes_before_return() {
    let (_dir, p) = fixture();
    let (hub, id) = hub(&p);
    let mut rx = hub.subscribe();
    hub.guarded_repository_checkout(&p.root, |paths| switch(&p, paths))
        .unwrap();
    assert_eq!(git(&p.root, &["rev-parse", "HEAD"]).unwrap(), p.head);
    assert_eq!(hub.buffers().text(&id).unwrap(), "new PR text\n");
    assert_eq!(
        fs::read_to_string(p.root.join("note.md")).unwrap(),
        "new PR text\n"
    );
    let messages: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(messages
        .iter()
        .any(|m| matches!(m, CrdtServerMessage::Sync { .. })));
    assert!(messages.iter().any(
        |m| matches!(m,CrdtServerMessage::Saved { buffer_id,.. } if buffer_id==&id)
    ));
}
#[test]
fn unsaved_authoritative_peer_prevents_entire_switch() {
    let (_dir, p) = fixture();
    let (hub, id) = hub(&p);
    hub.buffers()
        .apply_daemon_edit(
            &id,
            CrdtBufferEdit::Insert {
                index: 0,
                content: "unsaved peer ".into(),
            },
        )
        .unwrap();
    let old_head = git(&p.root, &["rev-parse", "HEAD"]).unwrap();
    let result = hub.guarded_repository_checkout(&p.root, |paths| switch(&p, paths));
    assert!(result.is_err());
    assert_eq!(git(&p.root, &["rev-parse", "HEAD"]).unwrap(), old_head);
    assert_eq!(fs::read_to_string(p.root.join("note.md")).unwrap(), "old\n");
    assert_eq!(hub.buffers().text(&id).unwrap(), "unsaved peer old\n");
}
#[test]
fn every_registered_document_is_checked_not_only_the_changed_file() {
    let (_dir, p) = fixture();
    let (hub, _) = hub(&p);
    let other = p.root.join("other.md");
    fs::write(&other, "other\n").unwrap();
    let id = crdt_buffer_id_for_path(&other);
    hub.open_buffer(&id, "");
    hub.buffers()
        .apply_daemon_edit(
            &id,
            CrdtBufferEdit::Insert {
                index: 0,
                content: "peer ".into(),
            },
        )
        .unwrap();
    assert!(hub
        .guarded_repository_checkout(&p.root, |_| -> std::result::Result<(), String> {
            panic!("all registered docs must be clean first")
        })
        .is_err());
    assert_eq!(fs::read_to_string(&other).unwrap(), "other\n");
}
#[test]
fn deleted_open_documents_reconcile_and_can_be_recreated_on_a_later_checkout() {
    let (_dir, p) = fixture();
    let (hub, id) = hub(&p);
    hub.guarded_repository_checkout(&p.root, |_| {
        fs::remove_file(p.root.join("note.md")).map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(hub.buffers().text(&id).unwrap(), "");
    hub.guarded_repository_checkout(&p.root, |_| {
        fs::write(p.root.join("note.md"), "restored\n").map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(hub.buffers().text(&id).unwrap(), "restored\n");
}
#[test]
fn open_text_replacement_with_binary_or_symlink_refuses_before_switch() {
    for (mode, bytes) in [(0o100644, &b"\xff\xfe"[..]), (0o120000, &b"outside"[..])] {
        let (_dir, mut p) = fixture();
        let (hub, id) = hub(&p);
        let repo = Repository::open(&p.root).unwrap();
        let parent = repo
            .find_commit(git2::Oid::from_str(&p.head).unwrap())
            .unwrap();
        let tree = parent.tree().unwrap();
        let mut builder = repo.treebuilder(Some(&tree)).unwrap();
        builder
            .insert("note.md", repo.blob(bytes).unwrap(), mode)
            .unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let sig = git2::Signature::now("fixture", "fixture@example.invalid").unwrap();
        p.head = repo
            .commit(
                None,
                &sig,
                &sig,
                "unsafe text replacement",
                &tree,
                &[&parent],
            )
            .unwrap()
            .to_string();
        assert!(hub
            .guarded_repository_checkout(&p.root, |paths| switch(&p, paths))
            .is_err());
        assert_eq!(fs::read_to_string(p.root.join("note.md")).unwrap(), "old\n");
        assert_eq!(hub.buffers().text(&id).unwrap(), "old\n");
    }
}

#[test]
fn dirty_index_or_untracked_files_never_carry_across_checkout() {
    let (_dir, p) = fixture();
    let (hub, _) = hub(&p);
    fs::write(p.root.join("untracked"), "keep me").unwrap();
    assert!(hub
        .guarded_repository_checkout(&p.root, |paths| switch(&p, paths))
        .is_err());
    assert_eq!(
        fs::read_to_string(p.root.join("untracked")).unwrap(),
        "keep me"
    );
    assert_eq!(fs::read_to_string(p.root.join("note.md")).unwrap(), "old\n");
}
#[test]
fn nested_current_scope_refuses_without_invoking_callback() {
    let (_dir, p) = fixture();
    let (hub, _) = hub(&p);
    assert!(hub
        .guarded_repository_checkout(
            &p.root.join("nested"),
            |_| -> std::result::Result<(), String> {
                panic!("must not broaden nested scope")
            }
        )
        .is_err());
    assert!(matches!(
        prepare(&p.root.join("nested"), 7, &p.head),
        Err((PullRequestErrorKind::Unsupported, _))
    ));
}
#[test]
fn new_worktree_is_real_exact_head_and_does_not_touch_dirty_source() {
    let (_dir, p) = fixture();
    let old_head = git(&p.root, &["rev-parse", "HEAD"]).unwrap();
    fs::write(p.root.join("note.md"), "unsaved disk edit\n").unwrap();
    let PullRequestReply::CheckedOut {
        path,
        worktree: true,
        number: 7,
    } = add_worktree(&p).unwrap()
    else {
        panic!("expected worktree success")
    };
    let path = PathBuf::from(path);
    assert!(path.starts_with(p.root.join(".neoism-pr-worktrees")));
    assert_eq!(git(&path, &["rev-parse", "HEAD"]).unwrap(), p.head);
    assert_eq!(
        fs::read_to_string(path.join("note.md")).unwrap(),
        "new PR text\n"
    );
    assert_eq!(git(&p.root, &["rev-parse", "HEAD"]).unwrap(), old_head);
    assert_eq!(
        fs::read_to_string(p.root.join("note.md")).unwrap(),
        "unsaved disk edit\n"
    );
}
#[test]
fn hosted_operations_reject_nested_declarations_before_credentials_or_network() {
    let (_dir, p) = fixture();
    let nested = p.root.join("nested");
    let requests = [
        PullRequestRequest::Discover { branch: None },
        PullRequestRequest::Read { number: 7 },
        PullRequestRequest::Create {
            title: "PR".into(),
            body: String::new(),
            base: "main".into(),
            draft: false,
            expected_branch: "topic".into(),
            expected_head: p.head.clone(),
        },
        PullRequestRequest::SubmitReview {
            number: 7,
            expected_head: p.head.clone(),
            body: "Review".into(),
            event: PullRequestReviewEvent::Approve,
            comments: vec![],
        },
        PullRequestRequest::ReplyThread {
            number: 7,
            thread_id: "thread".into(),
            body: "Reply".into(),
        },
        PullRequestRequest::ResolveThread {
            number: 7,
            thread_id: "thread".into(),
            resolved: true,
        },
        PullRequestRequest::Checkout {
            number: 7,
            expected_head: p.head.clone(),
            new_worktree: true,
        },
        PullRequestRequest::Merge {
            number: 7,
            expected_head: p.head.clone(),
            method: PullRequestMergeMethod::Merge,
        },
    ];
    for request in requests {
        crate::pull_request::tests::fixture(Vec::new(), || {
            assert!(
                matches!(handle_at(&nested, request), PullRequestReply::Error { kind:PullRequestErrorKind::Unsupported, message } if message.contains("repository-root workspace"))
            );
        });
    }
    let hub = CrdtSyncHub::default();
    crate::pull_request::tests::fixture(Vec::new(), || {
        assert!(matches!(
            checkout(&nested, 7, &p.head, false, Some(&hub)),
            Err((PullRequestErrorKind::Unsupported, _))
        ));
    });
}
#[test]
fn existing_worktree_destination_is_never_overwritten() {
    let (_dir, p) = fixture();
    let path = destination(&p).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep"), "precious").unwrap();
    assert!(add_worktree(&p).is_err());
    assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "precious");
}
#[cfg(unix)]
#[test]
fn managed_container_symlink_cannot_escape_declaration() {
    let (_dir, p) = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), p.root.join(".neoism-pr-worktrees"))
        .unwrap();
    assert!(add_worktree(&p).is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
#[test]
fn document_updates_and_saves_wait_until_checkout_reconciles() {
    use std::{sync::mpsc, time::Duration};
    let (_dir, p) = fixture();
    let (hub, id) = hub(&p);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let h = hub.clone();
    let root = p.root.clone();
    let worker = std::thread::spawn(move || {
        h.guarded_repository_checkout(&root, |paths| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            switch(&p, paths)
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let h = hub.clone();
    let save_id = id.clone();
    let save = std::thread::spawn(move || {
        h.save_buffer(&save_id);
        done_tx.send(()).unwrap();
    });
    assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
    release_tx.send(()).unwrap();
    worker.join().unwrap().unwrap();
    save.join().unwrap();
    assert_eq!(hub.buffers().text(&id).unwrap(), "new PR text\n");
}
