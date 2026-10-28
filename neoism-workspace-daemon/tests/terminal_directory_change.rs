//! Fail-closed live-directory control: no shell text injection, even at a prompt.
#![cfg(unix)]
use neoism_protocol::pty::{ClientMessage, ServerMessage};
use neoism_workspace_daemon::sessions::{SessionOutputSubscription, SessionRegistry};
use std::time::Duration;

struct Cleanup(SessionRegistry, String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.0.handle(ClientMessage::ClosePty {
            session_id: self.1.clone(),
        });
    }
}

async fn until(rx: &mut SessionOutputSubscription, needle: &str) -> String {
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(ServerMessage::PtyOutput { bytes, .. }) = rx.recv().await {
                output.push_str(&String::from_utf8_lossy(&bytes));
                if output.contains(needle) {
                    break;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("missing {needle:?}: {output:?}"));
    output
}

fn request(id: &str, directory: &str, generation: &str) -> ClientMessage {
    ClientMessage::ChangeTerminalDirectory {
        session_id: id.into(),
        directory: directory.into(),
        request_id: generation.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_pty_busy_quoted_directory_request_sends_no_input_and_preserves_cwd() {
    let root = tempfile::tempdir().unwrap();
    let quoted = root.path().join("a 'quoted' ; $(touch INJECTED) directory");
    std::fs::create_dir(&quoted).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='cwd-contract'\nversion='0.1.0'\n",
    )
    .unwrap();
    let (registry, mut owner) = SessionRegistry::new();
    let created = registry.handle_subscribed(
        ClientMessage::CreatePty {
            presentation: None,
            cwd: Some(root.path().to_string_lossy().into_owned()),
            cols: 120,
            rows: 24,
            shell: Some("/bin/sh".into()),
        },
        Some(&owner),
    );
    let id = match &created[0] {
        ServerMessage::PtyCreated { session_id, .. } => session_id.clone(),
        other => panic!("{other:?}"),
    };
    // Disable echo so merely echoing a submitted command cannot satisfy markers.
    registry.handle(ClientMessage::PtyInput { session_id: id.clone(), bytes: b"stty -echo; printf 'READY_%s\\n' BUSY; IFS= read -r answer; printf 'ANSWER=%s\\n' \"$answer\"; pwd; cargo locate-project --message-format plain; printf 'DONE_%s\\n' CHECK\n".to_vec() });
    until(&mut owner, "READY_BUSY").await;
    let peer = registry.subscribe();
    for (rx, generation) in [(&peer, "peer"), (&owner, "A"), (&owner, "B")] {
        let replies = registry.handle_subscribed(
            request(&id, quoted.to_str().unwrap(), generation),
            Some(rx),
        );
        assert!(
            matches!(&replies[..], [ServerMessage::TerminalDirectoryChangeFailed { session_id, request_id, .. }] if session_id == &id && request_id == generation)
        );
    }
    // If any cd text had been injected, `read` would finish and print ANSWER.
    let mut unexpected = String::new();
    while let Ok(Ok(message)) =
        tokio::time::timeout(Duration::from_millis(150), owner.recv()).await
    {
        if let ServerMessage::PtyOutput { bytes, .. } = message {
            unexpected.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    assert!(
        !unexpected.contains("ANSWER="),
        "unexpected PTY bytes: {unexpected:?}"
    );
    registry.handle(ClientMessage::PtyInput {
        session_id: id.clone(),
        bytes: b"untouched\n".to_vec(),
    });
    let result = until(&mut owner, "DONE_CHECK").await;
    assert!(result.contains("ANSWER=untouched"), "{result}");
    assert!(result.contains(root.path().to_str().unwrap()), "{result}");
    assert!(
        result.contains(root.path().join("Cargo.toml").to_str().unwrap()),
        "{result}"
    );
    assert!(!root.path().join("INJECTED").exists());
    registry.handle(ClientMessage::ClosePty { session_id: id });
}

async fn confirmed(
    rx: &mut SessionOutputSubscription,
    id: &str,
    generation: &str,
) -> String {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match rx.recv().await.unwrap() {
                ServerMessage::TerminalDirectoryChanged {
                    session_id,
                    request_id,
                    directory,
                } if session_id == id && request_id == generation => return directory,
                ServerMessage::TerminalDirectoryChangeFailed { message, .. } => {
                    panic!("unexpected failure: {message}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("real shell cwd confirmation")
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integrated_shells_home_to_quoted_repo_confirms_real_pwd_and_cargo() {
    for shell in ["/bin/bash", "/bin/zsh"] {
        if !std::path::Path::new(shell).exists() {
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo 'quoted' ; $(touch INJECTED)");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname='selected-repo'\nversion='0.1.0'\n",
        )
        .unwrap();
        let (registry, mut owner) = SessionRegistry::new();
        let created = registry.handle_subscribed(
            ClientMessage::CreatePty {
                presentation: None,
                cwd: Some(home.path().to_string_lossy().into_owned()),
                cols: 120,
                rows: 24,
                shell: Some(shell.into()),
            },
            Some(&owner),
        );
        let id = match &created[0] {
            ServerMessage::PtyCreated { session_id, .. } => session_id.clone(),
            other => panic!("{other:?}"),
        };
        let _cleanup = Cleanup(registry.clone(), id.clone());
        until(&mut owner, "\x1b]133;B").await;
        let peer = registry.subscribe();
        let denied = registry.handle_subscribed(
            request(&id, repo.to_str().unwrap(), "unauthorized"),
            Some(&peer),
        );
        assert!(matches!(
            denied.as_slice(),
            [ServerMessage::TerminalDirectoryChangeFailed { .. }]
        ));
        assert!(registry
            .handle_subscribed(
                request(&id, repo.to_str().unwrap(), "selected"),
                Some(&owner)
            )
            .is_empty());
        assert_eq!(
            confirmed(&mut owner, &id, "selected").await,
            repo.canonicalize().unwrap().to_str().unwrap()
        );
        registry.handle(ClientMessage::PtyInput { session_id: id.clone(), bytes: b"stty -echo; printf 'PWD_%s\\n' CHECK; pwd; cargo locate-project --message-format plain; printf 'DONE_%s\\n' SELECTED\r".to_vec() });
        let result = until(&mut owner, "DONE_SELECTED").await;
        assert!(result.contains(repo.to_str().unwrap()), "{result}");
        assert!(
            result.contains(repo.join("Cargo.toml").to_str().unwrap()),
            "{result}"
        );
        assert!(!home.path().join("INJECTED").exists());
        registry.handle(ClientMessage::ClosePty { session_id: id });
    }
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integrated_bash_busy_builtin_read_defers_without_any_control_bytes() {
    let shell = "/bin/bash";
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("selected");
    std::fs::create_dir(&target).unwrap();
    let (registry, mut owner) = SessionRegistry::new();
    let created = registry.handle_subscribed(
        ClientMessage::CreatePty {
            presentation: None,
            cwd: Some(home.path().to_string_lossy().into_owned()),
            cols: 120,
            rows: 24,
            shell: Some(shell.into()),
        },
        Some(&owner),
    );
    let id = match &created[0] {
        ServerMessage::PtyCreated { session_id, .. } => session_id.clone(),
        other => panic!("{other:?}"),
    };
    let _cleanup = Cleanup(registry.clone(), id.clone());
    until(&mut owner, "\x1b]133;B").await;
    registry.handle(ClientMessage::PtyInput { session_id: id.clone(), bytes: b"stty -echo; printf 'BUSY_%s\\n' READY; if IFS= read -r -t 0.7 answer; then printf 'INJECTED=%s\\n' \"$answer\"; else printf 'NO_%s\\n' INPUT; fi\r".to_vec() });
    until(&mut owner, "BUSY_READY").await;
    let start = std::time::Instant::now();
    assert!(registry
        .handle_subscribed(request(&id, target.to_str().unwrap(), "busy"), Some(&owner))
        .is_empty());
    // Even another client's ordinary writes cannot sneak commands into old cwd.
    let blocked = registry.handle(ClientMessage::PtyInput {
        session_id: id.clone(),
        bytes: b"touch WRONG_CWD\r".to_vec(),
    });
    assert!(matches!(blocked.as_slice(), [ServerMessage::Error { .. }]));
    assert!(tokio::time::timeout(
        Duration::from_millis(120),
        confirmed(&mut owner, &id, "busy")
    )
    .await
    .is_err());
    assert_eq!(
        confirmed(&mut owner, &id, "busy").await,
        target.to_str().unwrap()
    );
    assert!(start.elapsed() >= Duration::from_millis(400));
    registry.handle(ClientMessage::PtyInput {
        session_id: id.clone(),
        bytes: b"pwd; printf 'DONE_%s\\n' BUSY\r".to_vec(),
    });
    let result = until(&mut owner, "DONE_BUSY").await;
    assert!(result.contains(target.to_str().unwrap()), "{result}");
    assert!(
        !result.contains("INJECTED="),
        "control bytes entered foreground read: {result}"
    );
    assert!(!home.path().join("WRONG_CWD").exists());
    registry.handle(ClientMessage::ClosePty { session_id: id });
}

#[test]
fn unknown_or_unattached_request_is_correlated_failure_not_success() {
    let (registry, _) = SessionRegistry::new();
    let replies = registry.handle(request("unknown", "/tmp", "generation"));
    assert!(
        matches!(&replies[..], [ServerMessage::TerminalDirectoryChangeFailed { session_id, request_id, .. }] if session_id == "unknown" && request_id == "generation")
    );
}
