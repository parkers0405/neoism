use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use neoism_agent_service_api::{
    ActorType, TenantQuotas, WorkspaceWorkerBootstrap, WorkspaceWorkerCredentialClaims,
    WorkspaceWorkerSigningKey,
};

struct WorkerProcess {
    child: Child,
    directory: PathBuf,
}
impl Drop for WorkerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn standalone_worker_bootstraps_with_public_key_and_exits_on_expiry() {
    let directory = std::env::temp_dir()
        .join(format!("neoism-worker-process-{}", neoism_agent_core::new_session_id()));
    let workspace = directory.join("workspace");
    let state = directory.join("state");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let workspace = std::fs::canonicalize(workspace).unwrap();
    let state = std::fs::canonicalize(state).unwrap();
    let signer = WorkspaceWorkerSigningKey::new([42u8; 32]).unwrap();
    let now = neoism_agent_service_api::workspace_worker::unix_now().unwrap();
    let bootstrap = WorkspaceWorkerBootstrap {
        version: 1,
        tenant_id: "test-company".into(),
        workspace_id: "test-workspace".into(),
        runtime_id: "test-runtime".into(),
        runtime_generation: 3,
        root: workspace.clone(),
        expires_at: now + 15,
    };
    let bootstrap_file = directory.join("bootstrap.json");
    let public_file = directory.join("verification.key");
    std::fs::write(&bootstrap_file, serde_json::to_vec(&bootstrap).unwrap()).unwrap();
    std::fs::write(&public_file, signer.verification_key().as_bytes()).unwrap();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let log_file = directory.join("worker.log");
    let child = Command::new(env!("CARGO_BIN_EXE_neoism-agent"))
        .args([
            "serve",
            "--hostname",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--worker-bootstrap",
        ])
        .arg(&bootstrap_file)
        .arg("--worker-verification-key-file")
        .arg(&public_file)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &state)
        .env("USERPROFILE", &state)
        .env("NEOISM_AGENT_STATE_DIR", &state)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_DATA_HOME", state.join("data"))
        .env("XDG_CONFIG_HOME", state.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(&log_file).unwrap()))
        .spawn()
        .unwrap();
    let mut worker = WorkerProcess { child, directory };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let endpoint = format!("http://127.0.0.1:{port}");
    let started = Instant::now();
    loop {
        if client
            .get(format!("{endpoint}/v2/health"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        assert!(
            worker.child.try_wait().unwrap().is_none(),
            "worker exited before readiness: {}",
            std::fs::read_to_string(&log_file).unwrap_or_default()
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "worker readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        client
            .get(format!("{endpoint}/v2/runtime"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let issued_at = neoism_agent_service_api::workspace_worker::unix_now().unwrap();
    let token = signer
        .issue(&WorkspaceWorkerCredentialClaims {
            version: 1,
            tenant_id: bootstrap.tenant_id.clone(),
            workspace_id: bootstrap.workspace_id.clone(),
            runtime_id: bootstrap.runtime_id.clone(),
            runtime_generation: bootstrap.runtime_generation,
            subject: "test-member".into(),
            actor_type: ActorType::Human,
            directory_prefix: workspace.clone(),
            scopes: vec!["agent:use".into()],
            quotas: TenantQuotas::default(),
            issued_at,
            expires_at: bootstrap.expires_at,
        })
        .unwrap();
    let info: serde_json::Value = client
        .get(format!("{endpoint}/v2/runtime"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["worker"]["workspaceId"], "test-workspace");
    assert_eq!(info["worker"]["runtimeGeneration"], 3);
    assert_eq!(info["executionAvailable"], true);
    let response = client
        .post(format!("{endpoint}/v2/sessions"))
        .bearer_auth(&token)
        .json(&serde_json::json!({"agent":"build", "title":"Cloud worker smoke"}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let session: serde_json::Value = response.json().await.unwrap();
    assert!(status.is_success(), "worker session failed: {session}");
    assert_eq!(session["workspaceId"], "test-workspace");
    assert_eq!(session["directory"], workspace.to_string_lossy().as_ref());
    loop {
        if let Some(status) = worker.child.try_wait().unwrap() {
            assert!(
                status.success(),
                "worker shutdown failed: {}",
                std::fs::read_to_string(&log_file).unwrap_or_default()
            );
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "expired worker did not stop"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
