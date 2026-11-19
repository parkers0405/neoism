use neochat::{client::Client, protocol::TaskState};
use std::{path::Path, process::{Child, Command, Stdio}, thread, time::{Duration, Instant}};

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(dir: &Path) -> (Daemon, Client) {
    let mut daemon = Daemon(Command::new(env!("CARGO_BIN_EXE_neochatd"))
        .args(["--bind", "127.0.0.1:0", "--data-dir"]).arg(dir)
        .stdout(Stdio::null()).spawn().expect("start daemon"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(client) = Client::discover(Some(dir), None) {
            if client.events(0, 0).is_ok() { return (daemon, client); }
        }
        assert!(daemon.0.try_wait().unwrap().is_none(), "daemon exited before startup");
        assert!(Instant::now() < deadline, "daemon startup timed out");
        thread::sleep(Duration::from_millis(30));
    }
}

fn cli(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_neochat"))
        .args(["--data-dir"]).arg(dir).args(args)
        .env_remove("NEOCHAT_URL").env_remove("NEOCHAT_TOKEN").env_remove("NEOCHAT_PROFILE")
        .output().expect("run CLI");
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn cli_and_client_share_durable_board_with_scoped_guest_access() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let (daemon, client) = start(dir);
    let post = ["post", "dev", "Ready for review", "--label", "state=open", "--request-id", "release-review-1"];
    let root = cli(dir, &post);
    assert_eq!(cli(dir, &post), root);
    cli(dir, &["--as", "coder", "reply", &root, "Reviewing"]);
    cli(dir, &["post", "private", "Not visible to guests"]);
    let task = cli(dir, &["tasks", "create", "dev", "Review the release"]);
    cli(dir, &["--as", "coder", "tasks", "claim", &task]);
    cli(dir, &["--as", "coder", "tasks", "complete", &task, "Approved"]);
    let snapshot = client.snapshot().unwrap();
    assert!(snapshot.messages.iter().any(|m| m.text == "Reviewing" && m.parent == Some(root.parse().unwrap())));
    assert!(snapshot.messages.iter().any(|m| m.id == task.parse::<u64>().unwrap() && m.task.as_ref().unwrap().state == TaskState::Completed));

    let invite = cli(dir, &["invite", "create", "guest", "--channel", "dev", "--json"]);
    let invite: neochat::protocol::Invite = serde_json::from_str(&invite).unwrap();
    let guest = Client::new(client.endpoint(), &invite.token, None).unwrap();
    assert_eq!(guest.snapshot().unwrap().channels, ["dev"]);
    assert!(!serde_json::to_string(&guest.events(0, 0).unwrap()).unwrap().contains("Not visible to guests"));
    cli(dir, &["invite", "revoke", &invite.id.to_string()]);
    assert!(guest.snapshot().is_err());

    drop(daemon);
    let (_daemon, restarted) = start(dir);
    assert_eq!(restarted.snapshot().unwrap(), snapshot);
    assert!(restarted.events(0, 0).unwrap().windows(2).all(|events| events[0].cursor < events[1].cursor));
}