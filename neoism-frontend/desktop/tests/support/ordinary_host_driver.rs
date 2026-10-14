//! These are process-protocol fixtures for the real ordinary-host driver, not
//! mock session/auth routes. Authentication is tested against the real router
//! in the parent integration target. No fixture opens a network port or Agent.
use neoism_server_transport_contract::launch_persistent_host;
use std::{
    cell::Cell,
    io,
    process::Command,
    time::{Duration, Instant},
};

const PORT: u16 = 43123; // Protocol metadata only, never bound (not 7878/9877).

fn child(mode: &str, pid_file: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ordinary_host_fixture"));
    command.args([mode, &PORT.to_string()]).arg(pid_file);
    command
}

#[cfg(target_os = "linux")]
fn assert_reaped(pid_file: &std::path::Path) {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
        "the driver must kill/reap only its failed, owned child"
    );
}

#[test]
fn contract_ordinary_host_startup_driver() {
    let directory = tempfile::tempdir().unwrap();
    for mode in ["exit", "fast", "nonce", "port"] {
        let pid_file = directory.path().join(format!("{mode}.pid"));
        let committed = Cell::new(false);
        let began = Instant::now();
        let error = launch_persistent_host(
            &mut child(mode, &pid_file),
            PORT,
            Some("contract-ordinary-secret"),
            || {
                committed.set(true);
                Ok(())
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            began.elapsed() < Duration::from_secs(4),
            "{mode}: startup must fail promptly"
        );
        assert!(
            !committed.get(),
            "{mode}: rejected readiness must not commit metadata"
        );
        assert!(
            !error.contains("contract-ordinary-secret"),
            "credential leaked: {error}"
        );
        if mode == "exit" {
            assert!(
                error.contains("association 409"),
                "real child diagnostics must survive: {error}"
            );
            assert!(error.contains("[redacted]"), "{error}");
        } else {
            assert!(
                error.contains("invalid persistent-host readiness"),
                "{mode}: {error}"
            );
        }
        #[cfg(target_os = "linux")]
        assert_reaped(&pid_file);
    }

    let failed_pid = directory.path().join("failed-commit.pid");
    let error =
        launch_persistent_host(&mut child("ready", &failed_pid), PORT, None, || {
            Err(io::Error::other("owned sidecar commit failed"))
        })
        .unwrap_err();
    assert!(error.to_string().contains("owned sidecar commit failed"));
    #[cfg(target_os = "linux")]
    assert_reaped(&failed_pid);

    let success_pid = directory.path().join("ready.pid");
    let sidecar = directory.path().join("persistent-sidecar.json");
    let committed = Cell::new(false);
    let address =
        launch_persistent_host(&mut child("ready", &success_pid), PORT, None, || {
            std::fs::write(&sidecar, b"owned persistent metadata")?;
            committed.set(true);
            Ok(())
        })
        .unwrap();
    assert_eq!(address, std::net::SocketAddr::from(([0, 0, 0, 0], PORT)));
    assert!(
        committed.get(),
        "source driver must commit before returning success/detaching"
    );
    assert_eq!(
        std::fs::read(&sidecar).unwrap(),
        b"owned persistent metadata"
    );
    // A successful persistent launch relinquishes the handle to its reaper.
    // This fixture self-expires; wait for that bounded lifetime, never kill a
    // listener or some unrelated existing daemon by port/PID.
    std::thread::sleep(Duration::from_millis(1100));
    #[cfg(target_os = "linux")]
    assert_reaped(&success_pid);
}
