//! Opt-in integration of the generic host coordinator with a real isolated worker.
use neoism_cloud_host::{
    docker::{DockerConfig, DockerDevelopmentProvider},
    runtime::{
        IsolationKind, WorkerConnection, WorkerTransport, WorkspaceKey, WorkspaceSpec,
    },
    ActorType, FileSigningKeyStore, HostApprovedAccess, LaunchPolicy, TenantQuotas,
    TrustedEndpointPolicy, WorkspaceHost,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

struct DevelopmentLoopback;
impl TrustedEndpointPolicy for DevelopmentLoopback {
    fn allows(&self, connection: &WorkerConnection) -> bool {
        connection.transport() == WorkerTransport::DevelopmentLoopbackHttp
    }
}

struct TestDeployment {
    provider: String,
    root: PathBuf,
}
impl Drop for TestDeployment {
    fn drop(&mut self) {
        let filter = format!("label=dev.neoism.provider={}", self.provider);
        for (kind, list_args, remove_args) in [
            (
                "container",
                vec!["ps", "-aq", "--filter", &filter],
                vec!["rm", "-f"],
            ),
            (
                "volume",
                vec!["volume", "ls", "-q", "--filter", &filter],
                vec!["volume", "rm"],
            ),
        ] {
            if let Ok(output) = std::process::Command::new("/usr/bin/docker")
                .args(&list_args)
                .output()
            {
                if output.status.success() {
                    for name in String::from_utf8_lossy(&output.stdout).lines() {
                        let inspected = std::process::Command::new("/usr/bin/docker")
                            .args([kind, "inspect", name])
                            .output();
                        if let Ok(inspected) = inspected {
                            if let Ok(value) =
                                serde_json::from_slice::<Value>(&inspected.stdout)
                            {
                                let labels = if kind == "container" {
                                    &value[0]["Config"]["Labels"]
                                } else {
                                    &value[0]["Labels"]
                                };
                                if labels["dev.neoism.provider"] == self.provider {
                                    let _ = std::process::Command::new("/usr/bin/docker")
                                        .args(&remove_args)
                                        .arg(name)
                                        .output();
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn docker(args: &[&str]) -> Vec<u8> {
    let output = tokio::process::Command::new("/usr/bin/docker")
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "Docker test command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn host(
    root: &std::path::Path,
    provider: Arc<DockerDevelopmentProvider>,
) -> WorkspaceHost {
    WorkspaceHost::new_development(
        root.join("registry"),
        provider,
        Arc::new(FileSigningKeyStore::new(root.join("private-signing")).unwrap()),
        LaunchPolicy {
            root: "/workspace".into(),
            state_root: "/var/lib/neoism".into(),
            lease_seconds: 900,
            readiness_deadline: Duration::from_secs(90),
            probe_timeout: Duration::from_secs(5),
            expected_image_version: Some(env!("CARGO_PKG_VERSION").into()),
        },
        Arc::new(DevelopmentLoopback),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires Docker and NEOISM_DOCKER_TEST_IMAGE; creates only its own test workspace"]
async fn workspace_launch_broker_projects_restart_and_replacement_are_end_to_end() {
    let image =
        std::env::var("NEOISM_DOCKER_TEST_IMAGE").expect("set the explicit test image");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("neoism-host-docker-e2e-{id}"));
    std::fs::create_dir_all(&root).unwrap();
    let _cleanup = TestDeployment {
        provider: format!("e2e_{id}"),
        root: root.clone(),
    };
    let provider = Arc::new(
        DockerDevelopmentProvider::new(DockerConfig {
            id: format!("e2e_{id}"),
            controller_directory: root.join("provider"),
            image_catalog: BTreeMap::from([("worker".into(), image)]),
            allow_unenforced_disk: true,
        })
        .unwrap(),
    );
    let key = WorkspaceKey::new(format!("test_{id}"), "workspace").unwrap();
    let spec = WorkspaceSpec {
        image: "worker".into(),
        region: "local".into(),
        vcpus: 1,
        memory_mib: 1024,
        disk_gib: 8,
    };
    let manager = host(&root, provider.clone());
    let status = manager.ensure_worker(&key, &spec).await.unwrap();
    assert!(status.verification.is_some());
    let original = status.binding.status.as_ref().unwrap().handle.clone();
    let info: Value =
        serde_json::from_slice(&docker(&["inspect", &original.machine_id]).await)
            .unwrap();
    let volumes: Vec<String> = info[0]["Mounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|mount| {
            mount["Type"] == "volume"
                && matches!(
                    mount["Destination"].as_str(),
                    Some("/workspace" | "/var/lib/neoism")
                )
        })
        .map(|mount| mount["Name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(volumes.len(), 2);
    for mount in info[0]["Mounts"].as_array().unwrap() {
        assert!(!mount["Source"]
            .as_str()
            .unwrap_or_default()
            .contains("private-signing"));
    }
    let access = HostApprovedAccess {
        subject: "member".into(),
        actor_type: ActorType::Human,
        directory_prefix: "/workspace".into(),
        scopes: vec!["agent:use".into()],
        quotas: TenantQuotas::default(),
        ttl_seconds: 60,
    };
    let grant = manager.connect(&key, &access).await.unwrap();
    assert_eq!(grant.capabilities.isolation, IsolationKind::Container);
    assert!(!grant.capabilities.disk_limit);
    assert_eq!(grant.worker_generation, 1);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let endpoint = grant.base_url.trim_end_matches('/');
    assert_eq!(
        client
            .get(format!("{endpoint}/v2/runtime"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    docker(&["exec", &original.machine_id, "/bin/bash", "-lc",
        "git init -q /workspace/origin && printf cloud-source > /workspace/origin/source.txt && git -C /workspace/origin add source.txt && git -C /workspace/origin -c user.name=Test -c user.email=test@example.invalid commit -qm initial && mkdir -p /workspace/projects && git clone -q /workspace/origin /workspace/projects/x && git -C /workspace/projects/x worktree add -q -b agent-one /workspace/worktrees/one && git -C /workspace/projects/x worktree add -q -b agent-two /workspace/worktrees/two && printf persistent-cloud-file > /workspace/projects/x/cloud.txt"]
    ).await;
    let mut session_ids = Vec::new();
    for agent in ["build", "explore"] {
        let response = client
            .post(format!("{endpoint}/v2/sessions"))
            .query(&[("directory", "/workspace/projects/x")])
            .bearer_auth(&grant.bearer)
            .json(&json!({"agent":agent}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let session: Value = response.json().await.unwrap();
        assert!(status.is_success(), "session creation failed: {session}");
        assert_eq!(session["workspaceId"], "workspace");
        assert_eq!(session["directory"], "/workspace/projects/x");
        session_ids.push(session["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        manager.status(&key).unwrap().binding.allocation.generation,
        1
    );
    manager.stop(&original).await.unwrap();
    assert!(manager.connect(&key, &access).await.is_err());
    drop(manager);
    let manager = host(&root, provider.clone());
    let resumed = manager.start(&original).await.unwrap();
    assert!(resumed.verification.is_some());
    let resumed_grant = manager.connect(&key, &access).await.unwrap();
    assert_eq!(resumed_grant.handle, original);
    let endpoint = resumed_grant.base_url.trim_end_matches('/');
    for session in &session_ids {
        assert!(client
            .get(format!("{endpoint}/v2/sessions/{session}"))
            .bearer_auth(&resumed_grant.bearer)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    assert_eq!(
        docker(&[
            "exec",
            &original.machine_id,
            "cat",
            "/workspace/projects/x/cloud.txt"
        ])
        .await,
        b"persistent-cloud-file"
    );
    manager.destroy(&original).await.unwrap();
    let replacement = manager.ensure_worker(&key, &spec).await.unwrap();
    let replacement_handle = replacement.binding.status.as_ref().unwrap().handle.clone();
    assert_eq!(replacement_handle.generation, 2);
    assert_ne!(replacement_handle.machine_id, original.machine_id);
    let replacement_grant = manager.connect(&key, &access).await.unwrap();
    let endpoint = replacement_grant.base_url.trim_end_matches('/');
    assert_eq!(
        client
            .get(format!("{endpoint}/v2/runtime"))
            .bearer_auth(&grant.bearer)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert!(client
        .get(format!("{endpoint}/v2/sessions/{}", session_ids[0]))
        .bearer_auth(&replacement_grant.bearer)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        docker(&[
            "exec",
            &replacement_handle.machine_id,
            "cat",
            "/workspace/projects/x/cloud.txt"
        ])
        .await,
        b"persistent-cloud-file"
    );
    manager.destroy(&replacement_handle).await.unwrap();
    for volume in volumes {
        docker(&["volume", "rm", &volume]).await;
    }
    drop(manager);
    drop(provider);
    std::fs::remove_dir_all(root).unwrap();
}
