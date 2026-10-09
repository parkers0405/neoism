#![cfg(unix)]
use neoism_agent_service_api::WorkspaceWorkerCredentialClaims;
use neoism_cloud_host::*;
use runtime::*;
const WORKSPACE_WORKER_CREDENTIAL_PREFIX: &str = "neoism-workspace-worker-v1";
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("neo-host-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Shared {
    allocation: Mutex<Option<Allocation>>,
    state: Mutex<Option<MachineState>>,
    mode: AtomicUsize,
    creates: AtomicUsize,
    hang: AtomicUsize,
    ensure_calls: AtomicUsize,
    inspect_calls: AtomicUsize,
    inspect_error: AtomicUsize,
    destroy_calls: AtomicUsize,
    destroy_lag: AtomicUsize,
    destroy_lost_reply: AtomicUsize,
    events: Mutex<Vec<String>>,
    workspace_contents: Mutex<String>,
}
struct TestProvider {
    shared: Arc<Shared>,
    endpoint: String,
    caps: Capabilities,
}
fn caps() -> Capabilities {
    Capabilities {
        isolation: IsolationKind::VirtualMachine,
        stop_start: true,
        cpu_limit: true,
        memory_limit: true,
        disk_limit: true,
        durable_workspace: true,
    }
}
fn handle(a: &Allocation) -> MachineHandle {
    MachineHandle {
        owner: a.owner.clone(),
        generation: a.generation,
        provider: a.provider.clone(),
        machine_id: format!("vm-{}", a.generation),
    }
}
impl TestProvider {
    fn status(&self, a: &Allocation, state: MachineState) -> MachineStatus {
        let h = handle(a);
        MachineStatus {
            handle: h.clone(),
            state,
            ready: false,
            connection: if state == MachineState::Running {
                Some(
                    WorkerConnection::new_development_loopback(h, &self.endpoint)
                        .unwrap(),
                )
            } else {
                None
            },
            failure: None,
        }
    }
}
impl RuntimeProvider for TestProvider {
    fn id(&self) -> &str {
        "test-vm"
    }
    fn capabilities(&self) -> Capabilities {
        self.caps
    }
    fn ensure<'a>(&'a self, a: &'a Allocation) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async move {
            self.shared.ensure_calls.fetch_add(1, Ordering::SeqCst);
            self.shared
                .events
                .lock()
                .unwrap()
                .push(format!("ensure:{}", a.generation));
            {
                let mut current = self.shared.allocation.lock().unwrap();
                if current.as_ref() != Some(a) {
                    if current.is_some() {
                        assert_eq!(
                            *self.shared.state.lock().unwrap(),
                            Some(MachineState::Destroyed)
                        );
                    }
                    *current = Some(a.clone());
                    self.shared.creates.fetch_add(1, Ordering::SeqCst);
                    *self.shared.state.lock().unwrap() = Some(MachineState::Running);
                }
            }
            if self.shared.hang.load(Ordering::SeqCst) == 1 {
                std::future::pending::<()>().await;
            }
            Ok(self.status(a, self.shared.state.lock().unwrap().unwrap()))
        })
    }
    fn inspect<'a>(
        &'a self,
        a: &'a Allocation,
        h: Option<&'a MachineHandle>,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async move {
            self.shared.inspect_calls.fetch_add(1, Ordering::SeqCst);
            self.shared.events.lock().unwrap().push(format!(
                "inspect:{}:{}",
                a.generation,
                h.is_some()
            ));
            if let Some(h) = h {
                assert_eq!(*h, handle(a));
            }
            if self.shared.inspect_error.load(Ordering::SeqCst) == 1 {
                return Err(ProviderError::new(FailureCode::NotFound, false));
            }
            let mut status = self.status(a, self.shared.state.lock().unwrap().unwrap());
            if self.shared.inspect_error.load(Ordering::SeqCst) == 2 {
                status.handle.owner.tenant = "foreign".into();
                status.connection = None;
            }
            Ok(status)
        })
    }
    fn start<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async move {
            assert_eq!(*h, handle(a));
            *self.shared.state.lock().unwrap() = Some(MachineState::Running);
            Ok(self.status(a, MachineState::Running))
        })
    }
    fn stop<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async move {
            assert_eq!(*h, handle(a));
            *self.shared.state.lock().unwrap() = Some(MachineState::Stopped);
            Ok(self.status(a, MachineState::Stopped))
        })
    }
    fn destroy<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async move {
            assert_eq!(*h, handle(a));
            self.shared.destroy_calls.fetch_add(1, Ordering::SeqCst);
            if self
                .shared
                .destroy_lag
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                *self.shared.state.lock().unwrap() = Some(MachineState::Stopping);
                self.shared
                    .events
                    .lock()
                    .unwrap()
                    .push(format!("destroy-pending:{}", a.generation));
                return Ok(self.status(a, MachineState::Stopping));
            }
            *self.shared.state.lock().unwrap() = Some(MachineState::Destroyed);
            if self.shared.destroy_lost_reply.swap(0, Ordering::SeqCst) == 1 {
                self.shared
                    .events
                    .lock()
                    .unwrap()
                    .push(format!("destroy-lost-reply:{}", a.generation));
                return Err(ProviderError::new(FailureCode::Transport, true));
            }
            self.shared
                .events
                .lock()
                .unwrap()
                .push(format!("destroy-confirmed:{}", a.generation));
            Ok(self.status(a, MachineState::Destroyed))
        })
    }
}
fn authenticated(headers: &HeaderMap, a: &Allocation) -> bool {
    let Some(token) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    else {
        return false;
    };
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts[0] != WORKSPACE_WORKER_CREDENTIAL_PREFIX {
        return false;
    }
    let Ok(sig) = URL_SAFE_NO_PAD.decode(parts[2]) else {
        return false;
    };
    let d = a.launch.as_ref().unwrap();
    let public = URL_SAFE_NO_PAD.decode(d.verification_key()).unwrap();
    if UnparsedPublicKey::new(&ED25519, public)
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
        .is_err()
    {
        return false;
    }
    let Ok(payload) = URL_SAFE_NO_PAD.decode(parts[1]) else {
        return false;
    };
    let Ok(c) = serde_json::from_slice::<WorkspaceWorkerCredentialClaims>(&payload)
    else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    c.tenant_id == a.owner.tenant
        && c.workspace_id == a.owner.workspace
        && c.runtime_id == d.runtime_id()
        && c.runtime_generation == a.generation
        && c.expires_at > now
        && c.expires_at <= d.expires_at()
        && c.expires_at - c.issued_at <= 300
        && c.scopes.contains(&"agent:read".into())
}
async fn worker(State(s): State<Arc<Shared>>, h: HeaderMap) -> Response {
    let a = s.allocation.lock().unwrap().clone().unwrap();
    if !authenticated(&h, &a) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mode = s.mode.load(Ordering::SeqCst);
    if mode == 5 {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "http://127.0.0.1:1/steal")],
        )
            .into_response();
    }
    if mode == 6 {
        return ([("content-type", "text/html")], "<html>not worker</html>")
            .into_response();
    }
    if mode == 7 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if mode == 11 {
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    if mode == 8 {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if mode == 9 {
        return Json(json!({"oversized":"a".repeat(17_000)})).into_response();
    }
    let d = a.launch.as_ref().unwrap();
    Json(json!({"deployment":"workspace-worker","executionAvailable":true,"worker":{
        "version":if mode==10 {2} else {1},"tenantId":if mode==1 {"foreign"} else {&a.owner.tenant},"workspaceId":a.owner.workspace,
        "runtimeId":d.runtime_id(),"runtimeGeneration":a.generation+u64::from(mode==2),"root":if mode==3 {"/wrong"} else if mode==12 {r"\\?\C:\workspace"} else if mode==13 {r"C:\workspace\child"} else if mode==14 {r"D:\workspace"} else {d.root()},
        "expiresAt":d.expires_at()+i64::from(mode==4)}})).into_response()
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({"healthy":true,"version":"test-image"}))
}
struct Fixture {
    dir: Dir,
    shared: Arc<Shared>,
    provider: Arc<TestProvider>,
    store: Arc<FileSigningKeyStore>,
    host: WorkspaceHost,
    key: WorkspaceKey,
    spec: WorkspaceSpec,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(lease: u32) -> Self {
        let dir = Dir::new();
        let shared = Arc::new(Shared::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/v2/runtime", get(worker))
            .route("/v2/health", get(health))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = Arc::new(TestProvider {
            shared: shared.clone(),
            endpoint: format!("{origin}/"),
            caps: caps(),
        });
        let store = Arc::new(FileSigningKeyStore::new(dir.0.join("signers")).unwrap());
        let host = Self::manager(&dir, provider.clone(), store.clone(), lease);
        Self {
            dir,
            shared,
            provider,
            store,
            host,
            key: WorkspaceKey::new("tenant:a", "workspace:b").unwrap(),
            spec: WorkspaceSpec {
                image: "stock".into(),
                region: "local".into(),
                vcpus: 2,
                memory_mib: 512,
                disk_gib: 5,
            },
            task,
        }
    }
    fn manager(
        dir: &Dir,
        provider: Arc<TestProvider>,
        store: Arc<FileSigningKeyStore>,
        lease: u32,
    ) -> WorkspaceHost {
        let origin = provider.endpoint.trim_end_matches('/').to_owned();
        WorkspaceHost::new(
            dir.0.join("registry"),
            provider,
            store,
            LaunchPolicy {
                root: "/workspace".into(),
                state_root: "/state".into(),
                lease_seconds: lease,
                readiness_deadline: Duration::from_millis(300),
                probe_timeout: Duration::from_millis(75),
                expected_image_version: Some("test-image".into()),
            },
            Arc::new(AllowedOrigins::new([origin]).unwrap()),
        )
        .unwrap()
    }
    fn access(&self) -> HostApprovedAccess {
        HostApprovedAccess {
            subject: "alice".into(),
            actor_type: ActorType::Human,
            directory_prefix: "/workspace".into(),
            scopes: vec!["agent:read".into()],
            quotas: TenantQuotas::default(),
            ttl_seconds: 60,
        }
    }
}
#[tokio::test]
async fn authenticated_lifecycle_restart_and_redacted_grant() {
    let f = Fixture::new(3600).await;
    let first = f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert!(first.verification.is_some());
    assert!(!first.binding.status.as_ref().unwrap().ready);
    let grant = f.host.connect(&f.key, &f.access()).await.unwrap();
    assert!(!format!("{grant:?}").contains(&grant.bearer));
    assert_eq!(
        serde_json::to_value(&grant).unwrap()["bearer"],
        grant.bearer
    );
    let restarted = Fixture::manager(&f.dir, f.provider.clone(), f.store.clone(), 3600);
    let next = restarted.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(first.binding.allocation, next.binding.allocation);
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 1);
    assert!(restarted.status(&f.key).unwrap().verification.is_none());
    restarted.stop(&grant.handle).await.unwrap();
    assert!(restarted.connect(&f.key, &f.access()).await.is_err());
    let started = restarted.start(&grant.handle).await.unwrap();
    assert!(started.verification.is_some());
    restarted.destroy(&grant.handle).await.unwrap();
    assert!(restarted.connect(&f.key, &f.access()).await.is_err());
    let next = restarted.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(next.binding.allocation.generation, 2);
    assert_ne!(
        next.binding.allocation.launch,
        first.binding.allocation.launch
    );
    assert!(restarted.start(&grant.handle).await.is_err());
}
#[tokio::test]
async fn rejects_identity_html_redirect_error_oversize_timeout() {
    let f = Fixture::new(3600).await;
    f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    for mode in 1..=10 {
        f.shared.mode.store(mode, Ordering::SeqCst);
        assert!(
            f.host.connect(&f.key, &f.access()).await.is_err(),
            "mode {mode}"
        );
    }
    f.shared.mode.store(0, Ordering::SeqCst);
    let mut access = f.access();
    access.directory_prefix = "/workspace/../secret".into();
    assert!(f.host.connect(&f.key, &access).await.is_err());
    access = f.access();
    access.ttl_seconds = 301;
    assert!(f.host.connect(&f.key, &access).await.is_err());
}
#[tokio::test]
async fn cancelled_provider_intent_reuses_public_descriptor_and_signer() {
    let f = Fixture::new(3600).await;
    f.shared.hang.store(1, Ordering::SeqCst);
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        f.host.ensure_worker(&f.key, &f.spec)
    )
    .await
    .is_err());
    let pending = f.host.status(&f.key).unwrap().binding;
    assert_eq!(pending.pending, Some(Intent::Ensure));
    assert!(pending.status.is_none());
    assert!(f.host.connect(&f.key, &f.access()).await.is_err());
    f.shared.hang.store(0, Ordering::SeqCst);
    let host = Fixture::manager(&f.dir, f.provider.clone(), f.store.clone(), 3600);
    let resumed = host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(resumed.binding.allocation, pending.allocation);
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn expired_replacement_requires_confirmed_destroy_and_fences_old_handles() {
    let f = Fixture::new(2).await;
    let first = f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    let old = first.binding.status.unwrap().handle;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(f.host.connect(&f.key, &f.access()).await.is_err());
    assert!(f.host.start(&old).await.is_err());
    let next = f.host.replace_expired(&old, &f.spec).await.unwrap();
    assert_eq!(next.binding.allocation.generation, 2);
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 2);
    assert!(f.host.stop(&old).await.is_err());
}
#[test]
fn signer_private_modes_durable_identity_and_corruption() {
    let dir = Dir::new();
    let root = dir.0.join("private");
    let store = FileSigningKeyStore::new(&root).unwrap();
    let id = SigningIdentity {
        provider: "a".into(),
        owner: WorkspaceKey::new("tenant:a", "workspace:b").unwrap(),
        generation: 1,
    };
    let a = store.load_or_create(&id).unwrap();
    let b = FileSigningKeyStore::new(&root).unwrap().load(&id).unwrap();
    assert_eq!(a.key.verification_key(), b.key.verification_key());
    assert_eq!(a.created_at, b.created_at);
    assert!(!format!("{store:?}").contains(root.to_str().unwrap()));
    let mut other = id.clone();
    other.generation = 2;
    assert_ne!(
        store.load_or_create(&other).unwrap().key.verification_key(),
        a.key.verification_key()
    );
    use std::os::unix::fs::MetadataExt;
    assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
    let path = root.join(format!("{}.seed", id.digest()));
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    std::fs::write(path, b"corrupt").unwrap();
    assert!(store.load_or_create(&id).is_err());
}
#[tokio::test]
async fn admission_and_endpoint_policy_fail_closed() {
    let f = Fixture::new(3600).await;
    let provider = Arc::new(TestProvider {
        shared: f.shared.clone(),
        endpoint: f.provider.endpoint.clone(),
        caps: Capabilities {
            disk_limit: false,
            isolation: IsolationKind::Container,
            ..caps()
        },
    });
    let policy = LaunchPolicy {
        root: "/workspace".into(),
        state_root: "/state".into(),
        lease_seconds: 60,
        readiness_deadline: Duration::from_millis(100),
        probe_timeout: Duration::from_millis(50),
        expected_image_version: None,
    };
    assert!(WorkspaceHost::new(
        f.dir.0.join("bad"),
        provider.clone(),
        f.store.clone(),
        policy.clone(),
        Arc::new(AllowedOrigins::new([]).unwrap())
    )
    .is_err());
    let host = WorkspaceHost::new_development(
        f.dir.0.join("dev"),
        provider,
        f.store.clone(),
        policy,
        Arc::new(AllowedOrigins::new([]).unwrap()),
    )
    .unwrap();
    assert!(matches!(
        host.ensure_worker(&f.key, &f.spec).await,
        Err(HostError::Denied)
    ));
}
struct Policy {
    wrong_workspace: bool,
}
impl HostPolicy for Policy {
    fn authorize<'a>(
        &'a self,
        bearer: &'a str,
        workspace: &'a str,
        action: HostAction,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = neoism_cloud_host::Result<TrustedApproval>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if bearer != "approved"
                || workspace != "workspace:b"
                || matches!(
                    action,
                    HostAction::Stop | HostAction::Destroy | HostAction::Start
                )
            {
                return Err(HostError::Denied);
            }
            Ok(TrustedApproval {
                key: WorkspaceKey::new(
                    "tenant:a",
                    if self.wrong_workspace {
                        "wrong"
                    } else {
                        workspace
                    },
                )
                .unwrap(),
                spec: WorkspaceSpec {
                    image: "stock".into(),
                    region: "local".into(),
                    vcpus: 2,
                    memory_mib: 512,
                    disk_gib: 5,
                },
                access: HostApprovedAccess {
                    subject: "alice".into(),
                    actor_type: ActorType::Human,
                    directory_prefix: "/workspace".into(),
                    scopes: vec!["agent:read".into()],
                    quotas: TenantQuotas::default(),
                    ttl_seconds: 60,
                },
            })
        })
    }
}
#[tokio::test]
async fn host_http_policy_cannot_select_tenant_roles_or_workspace() {
    use tower::ServiceExt;
    let f = Fixture::new(3600).await;
    f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    // Reopen using the same durable signer and binding, as a real launcher does.
    let host = Arc::new(Fixture::manager(
        &f.dir,
        f.provider.clone(),
        f.store.clone(),
        3600,
    ));
    let app = host_router(
        host.clone(),
        Arc::new(Policy {
            wrong_workspace: false,
        }),
    );
    for (path, bearer, body, expected) in [
        ("connection", "approved", "{}", 200),
        ("connection", "bad", "{}", 403),
        ("connection", "approved", "{\"tenant\":\"foreign\"}", 400),
        ("stop", "approved", "{}", 400),
    ] {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/v1/workspaces/workspace:b/runtime/{path}"))
            .header("authorization", format!("Bearer {bearer}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), expected);
    }
    let old = host.status(&f.key).unwrap().binding.status.unwrap().handle;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/workspaces/workspace:b/runtime/stop")
        .header("authorization", "Bearer approved")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&LifecycleRequest {
                expected_handle: old,
            })
            .unwrap(),
        ))
        .unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), 403);
    let wrong = host_router(
        host,
        Arc::new(Policy {
            wrong_workspace: true,
        }),
    );
    let request = axum::http::Request::builder()
        .uri("/v1/workspaces/workspace:b/runtime/status")
        .header("authorization", "Bearer approved")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(wrong.oneshot(request).await.unwrap().status(), 403);
    let schema = neoism_cloud_host::canonical_openapi();
    assert!(
        schema["paths"]["/v1/workspaces/{workspace}/runtime/connection"]["post"]
            .is_object()
    );
    assert_eq!(
        schema["components"]["schemas"]["ConnectionGrant"]["required"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
}

#[tokio::test]
async fn in_flight_probe_cannot_publish_after_stop_and_old_token_fails_new_generation() {
    let f = Fixture::new(3600).await;
    f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    let grant = f.host.connect(&f.key, &f.access()).await.unwrap();
    f.shared.mode.store(11, Ordering::SeqCst);
    let access = f.access();
    let probe = f.host.connect(&f.key, &access);
    let stop = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        f.host.stop(&grant.handle).await.unwrap();
    };
    let (result, ()) = tokio::join!(probe, stop);
    assert!(matches!(result, Err(HostError::Stale)));
    f.shared.mode.store(0, Ordering::SeqCst);
    f.host.destroy(&grant.handle).await.unwrap();
    f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    let response = reqwest::Client::new()
        .get(format!("{}v2/runtime", f.provider.endpoint))
        .bearer_auth(&grant.bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let new = f.host.connect(&f.key, &f.access()).await.unwrap();
    let forged = format!("{}x", new.bearer);
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}v2/runtime", f.provider.endpoint))
            .bearer_auth(forged)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

struct BridgePolicy;
impl BridgeAuthorizer for BridgePolicy {
    fn authorize<'a>(
        &'a self,
        bearer: &'a str,
        owner: &'a WorkspaceKey,
        _: RuntimeAction,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            if bearer == "private-bridge-token" && owner.tenant == "tenant:a" {
                Ok(())
            } else {
                Err(ProviderError::new(FailureCode::Unauthorized, false))
            }
        })
    }
}
#[tokio::test]
async fn real_http_v2_bridge_lifecycle_and_authenticated_worker_probe() {
    let f = Fixture::new(3600).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = provider_router(f.provider.clone(), Arc::new(BridgePolicy));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let bridge = Arc::new(
        HttpProvider::new_plaintext(
            "test-vm",
            &origin,
            "private-bridge-token",
            Duration::from_secs(1),
            caps(),
        )
        .unwrap(),
    );
    let host = WorkspaceHost::new(
        f.dir.0.join("http-registry"),
        bridge,
        f.store.clone(),
        LaunchPolicy {
            root: "/workspace".into(),
            state_root: "/state".into(),
            lease_seconds: 3600,
            readiness_deadline: Duration::from_secs(2),
            probe_timeout: Duration::from_millis(150),
            expected_image_version: Some("test-image".into()),
        },
        Arc::new(
            AllowedOrigins::new([f.provider.endpoint.trim_end_matches('/').into()])
                .unwrap(),
        ),
    )
    .unwrap();
    let ensured = host.ensure_worker(&f.key, &f.spec).await.unwrap();
    let h = ensured.binding.status.unwrap().handle;
    host.connect(&f.key, &f.access()).await.unwrap();
    host.stop(&h).await.unwrap();
    host.start(&h).await.unwrap();
    host.destroy(&h).await.unwrap();
    assert!(host.status(&f.key).unwrap().binding.retired);
    task.abort();
}

#[tokio::test]
async fn deadline_cancels_provider_and_next_call_replays_intent() {
    let f = Fixture::new(3600).await;
    f.shared.hang.store(1, Ordering::SeqCst);
    assert!(matches!(
        f.host.ensure_worker(&f.key, &f.spec).await,
        Err(HostError::Timeout)
    ));
    let pending = f.host.status(&f.key).unwrap().binding;
    assert_eq!(pending.pending, Some(Intent::Ensure));
    f.shared.hang.store(0, Ordering::SeqCst);
    assert_eq!(
        f.host
            .ensure_worker(&f.key, &f.spec)
            .await
            .unwrap()
            .binding
            .allocation,
        pending.allocation
    );
}

/// Models durable secret-manager preparation records; advancing their age avoids
/// sleeping or exposing private seeds through a production API.
#[derive(Default)]
struct AgingStore {
    record: Mutex<Option<SigningAuthority>>,
    renewals: AtomicUsize,
}
impl AgingStore {
    fn expire_preparation(&self) {
        self.record.lock().unwrap().as_mut().unwrap().created_at -= 5000;
    }
}
impl SigningKeyStore for AgingStore {
    fn load_or_create(
        &self,
        _: &SigningIdentity,
    ) -> neoism_cloud_host::Result<SigningAuthority> {
        let mut record = self.record.lock().unwrap();
        Ok(record
            .get_or_insert_with(|| SigningAuthority {
                key: neoism_agent_service_api::WorkspaceWorkerSigningKey::new([1; 32])
                    .unwrap(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            })
            .clone())
    }
    fn load(&self, _: &SigningIdentity) -> neoism_cloud_host::Result<SigningAuthority> {
        self.record
            .lock()
            .unwrap()
            .clone()
            .ok_or(HostError::Signing)
    }
    fn renew_uncommitted(
        &self,
        _: &SigningIdentity,
        expected: &neoism_agent_service_api::WorkspaceWorkerVerificationKey,
    ) -> neoism_cloud_host::Result<SigningAuthority> {
        let mut record = self.record.lock().unwrap();
        let existing = record.as_ref().ok_or(HostError::Signing)?;
        if &existing.key.verification_key() != expected {
            return Err(HostError::Stale);
        }
        let authority = SigningAuthority {
            key: neoism_agent_service_api::WorkspaceWorkerSigningKey::new([2; 32])
                .unwrap(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        };
        *record = Some(authority.clone());
        self.renewals.fetch_add(1, Ordering::SeqCst);
        Ok(authority)
    }
}
struct CancelBeforeCommit<'a>(&'a WorkspaceHost);
impl AllocationInitializer for CancelBeforeCommit<'_> {
    fn initialize<'a>(
        &'a self,
        seed: &'a AllocationSeed,
    ) -> ProviderFuture<'a, WorkerLaunchDescriptor> {
        Box::pin(async move {
            let _prepared = self.0.initialize(seed).await?;
            // Authority was committed, but Runtime has not received the descriptor
            // and therefore cannot commit allocation or call the provider.
            std::future::pending().await
        })
    }
}
#[tokio::test]
async fn expired_cancelled_precommit_preparation_renews_only_before_allocation() {
    let f = Fixture::new(3600).await;
    let store = Arc::new(AgingStore::default());
    let host = WorkspaceHost::new(
        f.dir.0.join("precommit"),
        f.provider.clone(),
        store.clone(),
        LaunchPolicy {
            root: "/workspace".into(),
            state_root: "/state".into(),
            lease_seconds: 3600,
            readiness_deadline: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(100),
            expected_image_version: None,
        },
        Arc::new(
            AllowedOrigins::new([f.provider.endpoint.trim_end_matches('/').into()])
                .unwrap(),
        ),
    )
    .unwrap();
    let runtime = Runtime::new(f.dir.0.join("precommit"), f.provider.clone()).unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(10),
        runtime.ensure_with_initializer(&f.key, &f.spec, &CancelBeforeCommit(&host))
    )
    .await
    .is_err());
    assert!(runtime.binding(&f.key).unwrap().is_none());
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 0);
    let old = store
        .record
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .key
        .verification_key();
    store.expire_preparation();
    let ready = host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(ready.binding.allocation.generation, 1);
    assert_eq!(store.renewals.load(Ordering::SeqCst), 1);
    assert_ne!(
        store
            .record
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .key
            .verification_key(),
        old
    );
    // Once allocated, even an aged preparation timestamp cannot cause renewal.
    store.expire_preparation();
    let again = host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(again.binding.allocation, ready.binding.allocation);
    assert_eq!(store.renewals.load(Ordering::SeqCst), 1);
    // Corruption is not a clean absence and cannot establish precommit proof.
    let registry = std::fs::read_dir(f.dir.0.join("precommit"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    std::fs::write(registry, b"invalid registry").unwrap();
    assert!(matches!(
        host.ensure_worker(&f.key, &f.spec).await,
        Err(HostError::Runtime(runtime::Error::Corrupt))
    ));
    assert_eq!(store.renewals.load(Ordering::SeqCst), 1);
}

#[test]
fn file_store_uncommitted_renewal_is_atomic_cas_with_fresh_seed() {
    let dir = Dir::new();
    let store = FileSigningKeyStore::new(dir.0.join("signers")).unwrap();
    let id = SigningIdentity {
        provider: "test-vm".into(),
        owner: WorkspaceKey::new("tenant", "workspace").unwrap(),
        generation: 1,
    };
    let first = store.load_or_create(&id).unwrap();
    // Trusted initializer establishes uncommitted/expired proof; this test covers
    // the concrete store's atomic swap and stale-CAS refusal independently.
    let renewed = store
        .renew_uncommitted(&id, &first.key.verification_key())
        .unwrap();
    assert_ne!(first.key.verification_key(), renewed.key.verification_key());
    assert_eq!(
        store.load(&id).unwrap().key.verification_key(),
        renewed.key.verification_key()
    );
    assert!(store
        .renew_uncommitted(&id, &first.key.verification_key())
        .is_err());
}

#[tokio::test]
async fn windows_vm_namespace_accepts_equivalent_roots_but_not_children_or_other_drives()
{
    let f = Fixture::new(3600).await;
    let host = WorkspaceHost::new(
        f.dir.0.join("windows"),
        f.provider.clone(),
        f.store.clone(),
        LaunchPolicy {
            root: r"\\?\c:\Workspace".into(),
            state_root: r"D:\State".into(),
            lease_seconds: 3600,
            readiness_deadline: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(100),
            expected_image_version: None,
        },
        Arc::new(
            AllowedOrigins::new([f.provider.endpoint.trim_end_matches('/').into()])
                .unwrap(),
        ),
    )
    .unwrap();
    f.shared.mode.store(12, Ordering::SeqCst);
    let ready = host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(
        ready.binding.allocation.launch.as_ref().unwrap().root(),
        "C:/Workspace"
    );
    let mut access = f.access();
    access.directory_prefix = r"C:\workspace\src".into();
    let grant = host.connect(&f.key, &access).await.unwrap();
    assert_eq!(grant.root, "C:/Workspace");
    for mode in [13, 14] {
        f.shared.mode.store(mode, Ordering::SeqCst);
        assert!(host.connect(&f.key, &access).await.is_err());
    }
    f.shared.mode.store(12, Ordering::SeqCst);
    access.directory_prefix = r"D:\Workspace".into();
    assert!(host.connect(&f.key, &access).await.is_err());
}

#[tokio::test]
async fn automatic_expiry_recovers_lost_create_reply_and_confirms_destroy_before_next_generation(
) {
    let f = Fixture::new(2).await;
    *f.shared.workspace_contents.lock().unwrap() = "retained workspace and state".into();
    f.shared.hang.store(1, Ordering::SeqCst);
    assert!(matches!(
        f.host.ensure_worker(&f.key, &f.spec).await,
        Err(HostError::Timeout)
    ));
    let lost = f.host.status(&f.key).unwrap().binding;
    assert!(lost.status.is_none());
    assert_eq!(lost.pending, Some(Intent::Ensure));
    tokio::time::sleep(Duration::from_secs(3)).await;
    f.shared.hang.store(0, Ordering::SeqCst);
    f.shared.destroy_lag.store(1, Ordering::SeqCst);
    let replacement = f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert!(replacement.verification.is_some());
    assert_eq!(replacement.binding.allocation.generation, 2);
    assert_eq!(f.shared.ensure_calls.load(Ordering::SeqCst), 2); // no expired ensure replay
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 2);
    assert_eq!(
        &*f.shared.workspace_contents.lock().unwrap(),
        "retained workspace and state"
    );
    assert_eq!(
        replacement
            .binding
            .allocation
            .launch
            .as_ref()
            .unwrap()
            .state_root(),
        lost.allocation.launch.as_ref().unwrap().state_root()
    );
    assert_ne!(
        replacement.binding.allocation.launch,
        lost.allocation.launch
    );
    let events = f.shared.events.lock().unwrap();
    let recovered = events.iter().position(|e| e == "inspect:1:false").unwrap();
    let confirmed = events
        .iter()
        .position(|e| e == "destroy-confirmed:1")
        .unwrap();
    let new_create = events.iter().position(|e| e == "ensure:2").unwrap();
    assert!(recovered < confirmed && confirmed < new_create);
    drop(events);
    // The committed generation's key was never renewed during recovery.
    let old = f
        .store
        .load(&SigningIdentity::from_allocation(&lost.allocation))
        .unwrap();
    assert_eq!(
        URL_SAFE_NO_PAD.encode(old.key.verification_key().as_bytes()),
        lost.allocation.launch.as_ref().unwrap().verification_key()
    );
    assert!(f.host.start(&handle(&lost.allocation)).await.is_err());
}

#[tokio::test]
async fn expired_unknown_or_foreign_inspection_fails_closed_and_explicit_handle_is_fenced(
) {
    let f = Fixture::new(2).await;
    f.shared.hang.store(1, Ordering::SeqCst);
    assert!(f.host.ensure_worker(&f.key, &f.spec).await.is_err());
    let lost = f.host.status(&f.key).unwrap().binding;
    tokio::time::sleep(Duration::from_secs(3)).await;
    f.shared.hang.store(0, Ordering::SeqCst);
    for mode in [1, 2] {
        f.shared.inspect_error.store(mode, Ordering::SeqCst);
        assert!(f.host.ensure_worker(&f.key, &f.spec).await.is_err());
        assert_eq!(f.host.status(&f.key).unwrap().binding, lost);
        assert_eq!(f.shared.destroy_calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.shared.ensure_calls.load(Ordering::SeqCst), 1);
    }
    f.shared.inspect_error.store(0, Ordering::SeqCst);
    let expected = handle(&lost.allocation);
    let mut foreign = expected.clone();
    foreign.generation += 1;
    assert!(matches!(
        f.host.replace_expired(&foreign, &f.spec).await,
        Err(HostError::Stale)
    ));
    assert_eq!(f.host.status(&f.key).unwrap().binding, lost);
    let replacement = f.host.replace_expired(&expected, &f.spec).await.unwrap();
    assert_eq!(replacement.binding.allocation.generation, 2);
}

#[tokio::test]
async fn expired_destroy_lost_reply_replays_existing_intent_without_inspecting_absence() {
    let f = Fixture::new(2).await;
    let first = f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    f.shared.destroy_lost_reply.store(1, Ordering::SeqCst);
    assert!(f.host.ensure_worker(&f.key, &f.spec).await.is_err());
    let pending = f.host.status(&f.key).unwrap().binding;
    assert_eq!(pending.pending, Some(Intent::Destroy));
    assert!(pending.last_error.is_some());
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 1);
    let inspections = f.shared.inspect_calls.load(Ordering::SeqCst);
    let restarted = Fixture::manager(&f.dir, f.provider.clone(), f.store.clone(), 2);
    let replacement = restarted.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(replacement.binding.allocation.generation, 2);
    // The only extra inspection is of the NEW running generation for readiness.
    assert_eq!(
        f.shared.inspect_calls.load(Ordering::SeqCst),
        inspections + 1
    );
    assert_ne!(
        replacement.binding.allocation.launch,
        first.binding.allocation.launch
    );
    let events = f.shared.events.lock().unwrap();
    assert!(
        events
            .iter()
            .position(|e| e == "destroy-confirmed:1")
            .unwrap()
            < events.iter().position(|e| e == "ensure:2").unwrap()
    );
}

#[tokio::test]
async fn expired_lost_reply_recovery_through_real_v2_http_bridge_and_authenticated_probe()
{
    let f = Fixture::new(2).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = provider_router(f.provider.clone(), Arc::new(BridgePolicy));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let bridge = Arc::new(
        HttpProvider::new_plaintext(
            "test-vm",
            &origin,
            "private-bridge-token",
            Duration::from_millis(100),
            caps(),
        )
        .unwrap(),
    );
    let host = WorkspaceHost::new(
        f.dir.0.join("http-expiry"),
        bridge,
        f.store.clone(),
        LaunchPolicy {
            root: "/workspace".into(),
            state_root: "/state".into(),
            lease_seconds: 2,
            readiness_deadline: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(100),
            expected_image_version: Some("test-image".into()),
        },
        Arc::new(
            AllowedOrigins::new([f.provider.endpoint.trim_end_matches('/').into()])
                .unwrap(),
        ),
    )
    .unwrap();
    f.shared.hang.store(1, Ordering::SeqCst);
    assert!(host.ensure_worker(&f.key, &f.spec).await.is_err());
    let lost = host.status(&f.key).unwrap().binding;
    assert!(lost.status.is_none());
    assert!(lost.last_error.is_some());
    tokio::time::sleep(Duration::from_secs(3)).await;
    f.shared.hang.store(0, Ordering::SeqCst);
    let replacement = host.ensure_worker(&f.key, &f.spec).await.unwrap();
    assert_eq!(replacement.binding.allocation.generation, 2);
    assert!(replacement.verification.is_some());
    let grant = host.connect(&f.key, &f.access()).await.unwrap();
    assert_eq!(grant.worker_generation, 2);
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 2);
    let events = f.shared.events.lock().unwrap();
    assert!(
        events.iter().position(|e| e == "inspect:1:false").unwrap()
            < events
                .iter()
                .position(|e| e == "destroy-confirmed:1")
                .unwrap()
    );
    assert!(
        events
            .iter()
            .position(|e| e == "destroy-confirmed:1")
            .unwrap()
            < events.iter().position(|e| e == "ensure:2").unwrap()
    );
    task.abort();
}

#[tokio::test]
async fn concurrent_replacement_fences_the_original_destroy_poller_without_extra_compute()
{
    let f = Fixture::new(2).await;
    let first = f.host.ensure_worker(&f.key, &f.spec).await.unwrap();
    let old = first.binding.status.unwrap().handle;
    tokio::time::sleep(Duration::from_secs(3)).await;
    f.shared.destroy_lag.store(1, Ordering::SeqCst);
    let original = f.host.ensure_worker(&f.key, &f.spec);
    let competing = async {
        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if f.shared
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| event == "destroy-pending:1")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        f.host.destroy(&old).await.unwrap();
        f.host.ensure_worker(&f.key, &f.spec).await.unwrap()
    };
    let (original, replacement) = tokio::join!(original, competing);
    assert!(matches!(original, Err(HostError::Stale)));
    assert_eq!(replacement.binding.allocation.generation, 2);
    assert_eq!(f.shared.creates.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.host.status(&f.key).unwrap().binding.allocation.generation,
        2
    );
}
