//! Real daemon lifecycle, discovery, exclusive startup, and restart persistence.
use neochat::protocol::{Discovery, Snapshot};
use std::{path::Path, process::{Child, Command, Stdio}, time::{Duration, Instant}};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) { let _=self.0.kill(); let _=self.0.wait(); }
}
fn spawn(dir:&Path)->Daemon {
    Daemon(Command::new(env!("CARGO_BIN_EXE_neochatd")).arg("--data-dir").arg(dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap())
}
fn discover(dir:&Path)->Discovery {
    let deadline=Instant::now()+Duration::from_secs(10);
    loop {
        if let Ok(bytes)=std::fs::read(dir.join("endpoint.json")) {
            if let Ok(endpoint)=serde_json::from_slice(&bytes) { return endpoint; }
        }
        assert!(Instant::now()<deadline,"daemon did not publish discovery file");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn daemon_discovery_singleton_and_restart() {
    let dir=tempfile::tempdir().unwrap();
    let mut daemon=spawn(dir.path()); let endpoint=discover(dir.path());
    assert!(endpoint.url.starts_with("http://127.0.0.1:")); assert_eq!(endpoint.token.len(),64);
    assert_ne!(endpoint.token,std::fs::read_to_string(dir.path().join("admin-token")).unwrap());
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,0o700);
        for file in ["endpoint.json","board-token","admin-token"] {
            assert_eq!(std::fs::metadata(dir.path().join(file)).unwrap().permissions().mode() & 0o777,0o600);
        }
    }
    let client=reqwest::blocking::Client::builder().timeout(Duration::from_secs(3)).build().unwrap();
    client.post(format!("{}/v1/actions",endpoint.url)).bearer_auth(&endpoint.token).json(&serde_json::json!({"type":"post","channel":"general","text":"persists after process exit"})).send().unwrap().error_for_status().unwrap();
    let mut second=spawn(dir.path());
    let deadline=Instant::now()+Duration::from_secs(3);
    loop {
        if let Some(status)=second.0.try_wait().unwrap() { assert!(!status.success()); break; }
        assert!(Instant::now()<deadline,"second daemon should reject locked data directory");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("endpoint.json")).unwrap(),serde_json::to_string_pretty(&endpoint).unwrap());
    // Exercise SIGTERM drain on Unix; other platforms exercise abrupt restart durability.
    #[cfg(unix)] {
        let status=Command::new("kill").args(["-TERM",&daemon.0.id().to_string()]).status().unwrap(); assert!(status.success());
        let deadline=Instant::now()+Duration::from_secs(5);
        loop {
            if let Some(status)=daemon.0.try_wait().unwrap() { assert!(status.success()); break; }
            assert!(Instant::now()<deadline,"graceful shutdown timed out"); std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!dir.path().join("endpoint.json").exists());
    }
    #[cfg(not(unix))] { daemon.0.kill().unwrap(); daemon.0.wait().unwrap(); std::fs::remove_file(dir.path().join("endpoint.json")).unwrap(); }
    let _restart=spawn(dir.path()); let next=discover(dir.path()); assert_eq!(endpoint.token,next.token);
    let snapshot:Snapshot=client.get(format!("{}/v1/snapshot",next.url)).bearer_auth(&next.token).send().unwrap().error_for_status().unwrap().json().unwrap();
    assert_eq!(snapshot.messages.len(),1); assert_eq!(snapshot.messages[0].text,"persists after process exit");
}