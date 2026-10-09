use neoism_cloud_runtime::*;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        Self(
            std::env::temp_dir().join(format!("neoism-runtime-{}", uuid::Uuid::new_v4())),
        )
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Fake {
    machines: Mutex<HashMap<String, MachineStatus>>,
    creates: AtomicUsize,
    ensures: AtomicUsize,
    starts: AtomicUsize,
    inspections: AtomicUsize,
    inspect_mode: AtomicUsize,
    lose_reply: AtomicBool,
    wrong: AtomicBool,
    fail_stop: AtomicBool,
    hang: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
fn key(a: &Allocation) -> String {
    serde_json::to_string(&(a.owner.clone(), a.generation)).unwrap()
}
impl Fake {
    fn mutate<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
        state: Option<MachineState>,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async move {
            if self.hang.load(Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            let mut machines = self.machines.lock().unwrap();
            let s = machines
                .get_mut(&key(a))
                .ok_or(ProviderError::new(FailureCode::NotFound, false))?;
            if &s.handle != h {
                return Err(ProviderError::new(FailureCode::Identity, false));
            }
            if let Some(state) = state {
                if s.state == MachineState::Destroyed && state != MachineState::Destroyed
                {
                    return Err(ProviderError::new(FailureCode::Conflict, false));
                }
                s.state = state;
                s.ready = state == MachineState::Running;
                s.connection = if s.ready {
                    Some(connection(&s.handle))
                } else {
                    None
                };
            }
            let mut result = s.clone();
            if self.wrong.load(Ordering::SeqCst) {
                result.handle.machine_id = "other-machine".into();
            }
            Ok(result)
        })
    }
}
impl RuntimeProvider for Fake {
    fn id(&self) -> &str {
        "fake"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            isolation: IsolationKind::VirtualMachine,
            cpu_limit: true,
            memory_limit: true,
            disk_limit: true,
            durable_workspace: true,
            stop_start: true,
        }
    }
    fn ensure<'a>(&'a self, a: &'a Allocation) -> ProviderFuture<'a, MachineStatus> {
        self.ensures.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let result = {
                let mut machines = self.machines.lock().unwrap();
                if machines.values().any(|s| {
                    s.handle.owner == a.owner
                        && (s.handle.generation > a.generation
                            || s.handle.generation < a.generation
                                && s.state != MachineState::Destroyed
                            || s.handle.generation == a.generation
                                && s.state == MachineState::Destroyed)
                }) {
                    return Err(ProviderError::new(FailureCode::Conflict, false));
                }
                machines
                    .entry(key(a))
                    .or_insert_with(|| {
                        self.creates.fetch_add(1, Ordering::SeqCst);
                        MachineStatus {
                            handle: MachineHandle {
                                owner: a.owner.clone(),
                                generation: a.generation,
                                provider: a.provider.clone(),
                                machine_id: uuid::Uuid::new_v4().to_string(),
                            },
                            state: MachineState::Stopped,
                            ready: false,
                            connection: None,
                            failure: None,
                        }
                    })
                    .clone()
            };
            if self.hang.load(Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if self.lose_reply.swap(false, Ordering::SeqCst) {
                return Err(ProviderError::new(FailureCode::Timeout, true));
            }
            Ok(result)
        })
    }
    fn inspect<'a>(
        &'a self,
        a: &'a Allocation,
        h: Option<&'a MachineHandle>,
    ) -> ProviderFuture<'a, MachineStatus> {
        self.inspections.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let mut status = match h {
                Some(h) => self.mutate(a, h, None).await?,
                None => self
                    .machines
                    .lock()
                    .unwrap()
                    .get(&key(a))
                    .cloned()
                    .ok_or(ProviderError::new(FailureCode::NotFound, false))?,
            };
            match self.inspect_mode.load(Ordering::SeqCst) {
                1 => status.handle.owner.tenant = "foreign".into(),
                2 => status.handle.generation += 1,
                3 => status.handle.provider = "foreign".into(),
                4 => status.handle.machine_id = "foreign".into(),
                5 => return Err(ProviderError::new(FailureCode::NotFound, false)),
                6 => status.state = MachineState::Destroyed,
                _ => {}
            }
            Ok(status)
        })
    }
    fn start<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.mutate(a, h, Some(MachineState::Running))
    }
    fn stop<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        if self.fail_stop.swap(false, Ordering::SeqCst) {
            Box::pin(async { Err(ProviderError::new(FailureCode::Unavailable, true)) })
        } else {
            self.mutate(a, h, Some(MachineState::Stopped))
        }
    }
    fn destroy<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        self.mutate(a, h, Some(MachineState::Destroyed))
    }
}
fn spec() -> WorkspaceSpec {
    WorkspaceSpec {
        image: "ubuntu-2404".into(),
        region: "us-east".into(),
        vcpus: 2,
        memory_mib: 2048,
        disk_gib: 20,
    }
}
fn connection(handle: &MachineHandle) -> WorkerConnection {
    WorkerConnection::new(handle.clone(), "https://worker.example/agent/").unwrap()
}
fn owner() -> WorkspaceKey {
    WorkspaceKey::new("tenant-a", "workspace").unwrap()
}

#[tokio::test]
async fn provisioning_recovers_lost_reply_across_restart() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    p.lose_reply.store(true, Ordering::SeqCst);
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    assert!(runtime.ensure(&owner(), &spec()).await.is_err());
    let b = runtime.binding(&owner()).unwrap().unwrap();
    assert_eq!(b.pending, Some(Intent::Ensure));
    assert!(b.status.is_none());
    assert!(b.last_error.is_some());
    drop(runtime);
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let recovered = runtime.reconcile(&owner()).await.unwrap();
    assert_eq!(recovered.pending, None);
    assert!(recovered.revision > b.revision);
    runtime.ensure(&owner(), &spec()).await.unwrap();
    assert_eq!(p.creates.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn lifecycle_replacement_and_generation_fences() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let b = runtime.ensure(&owner(), &spec()).await.unwrap();
    let h = b.status.unwrap().handle;
    let running = runtime.start(&h).await.unwrap();
    assert!(running.status.unwrap().ready);
    p.fail_stop.store(true, Ordering::SeqCst);
    assert!(runtime.stop(&h).await.is_err());
    let failed = runtime.binding(&owner()).unwrap().unwrap();
    assert_eq!(failed.pending, Some(Intent::Stop));
    assert_eq!(failed.status.unwrap().state, MachineState::Running);
    assert_eq!(
        runtime
            .reconcile(&owner())
            .await
            .unwrap()
            .status
            .unwrap()
            .state,
        MachineState::Stopped
    );
    let mut replacement = spec();
    replacement.disk_gib += 1;
    assert!(matches!(
        runtime.ensure(&owner(), &replacement).await,
        Err(Error::Conflict)
    ));
    let retired = runtime.destroy(&h).await.unwrap();
    assert!(retired.retired);
    assert_eq!(runtime.destroy(&h).await.unwrap(), retired);
    let new = runtime.ensure(&owner(), &replacement).await.unwrap();
    assert_eq!(new.allocation.generation, h.generation + 1);
    assert!(new.revision > retired.revision);
    assert!(matches!(runtime.start(&h).await, Err(Error::Fenced)));
    assert!(matches!(runtime.destroy(&h).await, Err(Error::Fenced)));
}
#[tokio::test]
async fn ownership_and_wrong_machine_cannot_overwrite_binding() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let a = runtime
        .ensure(&owner(), &spec())
        .await
        .unwrap()
        .status
        .unwrap()
        .handle;
    let other = WorkspaceKey::new("tenant-b", "workspace").unwrap();
    let b = runtime
        .ensure(&other, &spec())
        .await
        .unwrap()
        .status
        .unwrap()
        .handle;
    assert_ne!(a.machine_id, b.machine_id);
    let mut forged = a.clone();
    forged.owner = other;
    assert!(matches!(runtime.stop(&forged).await, Err(Error::Fenced)));
    p.wrong.store(true, Ordering::SeqCst);
    assert!(matches!(
        runtime.reconcile(&owner()).await,
        Err(Error::Provider(ProviderError {
            code: FailureCode::Identity,
            ..
        }))
    ));
    assert_eq!(
        runtime
            .binding(&owner())
            .unwrap()
            .unwrap()
            .status
            .unwrap()
            .handle,
        a
    );
}
#[tokio::test]
async fn process_lock_and_cancellation_leave_recoverable_intent() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    p.hang.store(true, Ordering::SeqCst);
    let runtime = Arc::new(Runtime::new(&temp.0, p.clone()).unwrap());
    let r = runtime.clone();
    let task = tokio::spawn(async move { r.ensure(&owner(), &spec()).await });
    p.entered.notified().await;
    let second = Runtime::new(&temp.0, p.clone()).unwrap();
    assert!(matches!(second.binding(&owner()), Err(Error::Busy)));
    task.abort();
    let _ = task.await;
    p.hang.store(false, Ordering::SeqCst);
    assert_eq!(
        second.binding(&owner()).unwrap().unwrap().pending,
        Some(Intent::Ensure)
    );
    second.reconcile(&owner()).await.unwrap();
    assert_eq!(p.creates.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn daemon_namespaced_owner_identity_is_preserved_and_fenced() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let key = WorkspaceKey::new("workspace:daemon.id-123", "repo.notes:v1").unwrap();
    let b = runtime.ensure(&key, &spec()).await.unwrap();
    let h = b.status.unwrap().handle;
    assert_eq!(h.owner, key);
    let started = runtime.start(&h).await.unwrap();
    assert_eq!(started.worker_connection().unwrap().handle().owner, key);
    let other = WorkspaceKey::new("workspace:daemon.id-456", "repo.notes:v1").unwrap();
    runtime.ensure(&other, &spec()).await.unwrap();
    let mut forged = h;
    forged.owner = other;
    assert!(matches!(runtime.stop(&forged).await, Err(Error::Fenced)));
    for id in [
        "workspace:bad/id",
        "workspace:bad\\\\id",
        "workspace:bad id",
        "workspace:bad\n",
    ] {
        assert!(WorkspaceKey::new(id, "workspace").is_err());
    }
    let reopened = Runtime::new(&temp.0, p).unwrap();
    assert_eq!(
        reopened.binding(&key).unwrap().unwrap().allocation.owner,
        key
    );
}

#[test]
fn bounded_identifiers_and_corrupt_registry() {
    assert!(WorkspaceKey::new("../escape", "w").is_err());
    assert!(WorkspaceKey::new("a".repeat(129), "w").is_err());
    let temp = Temp::new();
    let runtime = Runtime::new(&temp.0, Arc::new(Fake::default())).unwrap();
    // Unknown data may never be interpreted as an empty registry.
    use sha2::{Digest, Sha256};
    let k = owner();
    let mut hash = Sha256::new();
    hash.update((k.tenant.len() as u64).to_be_bytes());
    hash.update(k.tenant.as_bytes());
    hash.update(k.workspace.as_bytes());
    std::fs::write(temp.0.join(format!("{:x}.json", hash.finalize())), b"{}").unwrap();
    assert!(matches!(runtime.binding(&k), Err(Error::Corrupt)));
}

#[tokio::test]
async fn transitional_provisioning_can_be_destroyed_without_replacement_overlap() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    p.lose_reply.store(true, Ordering::SeqCst);
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    assert!(runtime.ensure(&owner(), &spec()).await.is_err());
    let allocation = runtime.binding(&owner()).unwrap().unwrap().allocation;
    p.machines
        .lock()
        .unwrap()
        .get_mut(&key(&allocation))
        .unwrap()
        .state = MachineState::Provisioning;
    let pending = runtime.reconcile(&owner()).await.unwrap();
    assert_eq!(pending.pending, Some(Intent::Ensure));
    let h = pending.status.unwrap().handle;
    assert!(runtime.destroy(&h).await.unwrap().retired);
    let replacement = runtime.ensure(&owner(), &spec()).await.unwrap();
    assert_eq!(replacement.allocation.generation, 2);
    assert_eq!(p.creates.load(Ordering::SeqCst), 2);
}

#[test]
fn readiness_and_failure_are_distinct_and_strict() {
    let allocation = Allocation {
        owner: owner(),
        generation: 1,
        provider: "fake".into(),
        spec: spec(),
        launch: None,
    };
    let mut status = MachineStatus {
        handle: MachineHandle {
            owner: owner(),
            generation: 1,
            provider: "fake".into(),
            machine_id: "machine".into(),
        },
        state: MachineState::Running,
        ready: false,
        connection: None,
        failure: None,
    };
    assert!(status.validate(&allocation, None).is_ok());
    status.connection = Some(connection(&status.handle));
    assert!(status.validate(&allocation, None).is_ok());
    let binding = Binding {
        allocation: allocation.clone(),
        revision: 1,
        status: Some(status.clone()),
        pending: None,
        last_error: None,
        retired: false,
    };
    assert!(binding.worker_connection().is_none());
    status.connection = None;
    status.state = MachineState::Failed;
    assert!(status.validate(&allocation, None).is_err());
    status.failure = Some(ProviderError::new(FailureCode::Unavailable, true));
    assert!(status.validate(&allocation, None).is_ok());
    status.ready = true;
    assert!(status.validate(&allocation, None).is_err());
    status.ready = false;
    status.state = MachineState::Stopped;
    assert!(status.validate(&allocation, None).is_err());
    status.failure = None;
    status.ready = true;
    assert!(status.validate(&allocation, None).is_err());
}

#[tokio::test]
async fn cancelled_stop_suppresses_stale_endpoint_and_replays_durable_intent() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Arc::new(Runtime::new(&temp.0, p.clone()).unwrap());
    let b = runtime.ensure(&owner(), &spec()).await.unwrap();
    let h = b.status.unwrap().handle;
    let running = runtime.start(&h).await.unwrap();
    assert!(running.worker_connection().is_some());
    p.hang.store(true, Ordering::SeqCst);
    let r = runtime.clone();
    let task = tokio::spawn(async move { r.stop(&h).await });
    p.entered.notified().await;
    assert!(matches!(runtime.binding(&owner()), Err(Error::Busy)));
    let path = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let persisted: Binding = serde_json::from_value(record["binding"].clone()).unwrap();
    assert_eq!(persisted.pending, Some(Intent::Stop));
    assert!(persisted.status.as_ref().unwrap().ready); // last-known snapshot, not current readiness
    assert!(persisted.worker_connection().is_none());
    task.abort();
    let _ = task.await;
    p.hang.store(false, Ordering::SeqCst);
    let reopened = Runtime::new(&temp.0, p).unwrap();
    assert_eq!(reopened.binding(&owner()).unwrap().unwrap(), persisted);
    let stopped = reopened.reconcile(&owner()).await.unwrap();
    assert!(stopped.worker_connection().is_none());
    assert!(stopped.status.unwrap().connection.is_none());
}

#[tokio::test]
async fn tombstone_contract_rejects_delayed_ensure_and_start() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let b = runtime.ensure(&owner(), &spec()).await.unwrap();
    let a = b.allocation;
    let h = b.status.unwrap().handle;
    runtime.destroy(&h).await.unwrap();
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Conflict);
    assert_eq!(
        p.start(&a, &h).await.unwrap_err().code,
        FailureCode::Conflict
    );
    assert_eq!(
        p.destroy(&a, &h).await.unwrap().state,
        MachineState::Destroyed
    );
    runtime.ensure(&owner(), &spec()).await.unwrap();
    assert_eq!(p.ensure(&a).await.unwrap_err().code, FailureCode::Conflict);
    assert_eq!(p.creates.load(Ordering::SeqCst), 2);
}

#[test]
fn worker_connection_validation_is_versioned_scoped_and_nonsecret() {
    let h = MachineHandle {
        owner: owner(),
        generation: 1,
        provider: "fake".into(),
        machine_id: "vm".into(),
    };
    let c = WorkerConnection::new(h.clone(), "https://worker.example/agent").unwrap();
    assert_eq!(c.agent_api_base_url(), "https://worker.example/agent/");
    for endpoint in [
        "http://worker.example/",
        "https://user:secret@worker.example/",
        "https://worker.example/#x",
        "https://worker.example/?token=secret",
        "https://worker.example/\n",
        "https://worker.example/\u{007f}",
        "file:///worker",
        "https://worker.example/\\path",
    ] {
        assert!(
            WorkerConnection::new(h.clone(), endpoint).is_err(),
            "{endpoint:?}"
        );
    }
    assert!(WorkerConnection::new(
        h.clone(),
        &format!("https://worker.example/{}", "x".repeat(2048))
    )
    .is_err());
    for endpoint in ["http://127.0.0.1:8080/", "http://[::1]:8080/"] {
        assert!(WorkerConnection::new(h.clone(), endpoint).is_err());
        assert!(WorkerConnection::new_development_loopback(h.clone(), endpoint).is_ok());
    }
    for endpoint in [
        "http://localhost:8080/",
        "http://192.168.1.2/",
        "http://worker.example/",
    ] {
        assert!(WorkerConnection::new_development_loopback(h.clone(), endpoint).is_err());
    }
    let mut foreign = h.clone();
    foreign.generation += 1;
    assert_eq!(
        c.validate(&foreign).unwrap_err().code,
        FailureCode::Identity
    );
    let mut raw = serde_json::to_value(&c).unwrap();
    raw["version"] = 2.into();
    let invalid: WorkerConnection = serde_json::from_value(raw).unwrap();
    assert_eq!(
        invalid.validate(&h).unwrap_err().code,
        FailureCode::Protocol
    );
    let a = Allocation {
        owner: owner(),
        generation: 1,
        provider: "fake".into(),
        spec: spec(),
        launch: None,
    };
    let mut status = MachineStatus {
        handle: h.clone(),
        state: MachineState::Running,
        ready: true,
        connection: None,
        failure: None,
    };
    assert!(status.validate(&a, None).is_err());
    status.connection = Some(c);
    assert!(status.validate(&a, None).is_ok());
    for state in [
        MachineState::Provisioning,
        MachineState::Starting,
        MachineState::Stopping,
        MachineState::Stopped,
        MachineState::Failed,
        MachineState::Destroyed,
    ] {
        status.state = state;
        status.ready = false;
        status.failure = if state == MachineState::Failed {
            Some(ProviderError::new(FailureCode::Unavailable, true))
        } else {
            None
        };
        assert!(status.validate(&a, None).is_err());
    }
}

#[tokio::test]
async fn http_real_protocol_identity_status_timeout_and_redaction() {
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    let mode = Arc::new(AtomicUsize::new(0));
    async fn handler(
        State(mode): State<Arc<AtomicUsize>>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        assert_eq!(headers.get("authorization").unwrap(), "Bearer secret-token");
        assert_eq!(body["version"], 2);
        let m = mode.load(Ordering::SeqCst);
        if m == 2 {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"secret": "secret-token"})),
            );
        }
        if m == 3 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if m == 4 {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({})));
        }
        let a: Allocation = serde_json::from_value(body["allocation"].clone()).unwrap();
        let s = MachineStatus {
            handle: MachineHandle {
                owner: if m == 1 {
                    WorkspaceKey::new("foreign", "workspace").unwrap()
                } else {
                    a.owner.clone()
                },
                generation: a.generation,
                provider: a.provider.clone(),
                machine_id: "machine-1".into(),
            },
            state: MachineState::Running,
            ready: true,
            connection: Some(connection(&MachineHandle {
                owner: a.owner.clone(),
                generation: a.generation,
                provider: a.provider.clone(),
                machine_id: "machine-1".into(),
            })),
            failure: None,
        };
        (
            StatusCode::OK,
            Json(serde_json::json!({"version": 2, "status": s})),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/v2/runtime/ensure", post(handler))
        .with_state(mode.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let caps = Capabilities {
        isolation: IsolationKind::VirtualMachine,
        cpu_limit: true,
        memory_limit: true,
        disk_limit: true,
        durable_workspace: true,
        stop_start: true,
    };
    assert!(HttpProvider::new(
        "bridge",
        &endpoint,
        "secret-token",
        Duration::from_secs(1),
        caps
    )
    .is_err());
    let provider = HttpProvider::new_plaintext(
        "bridge",
        &endpoint,
        "secret-token",
        Duration::from_millis(80),
        caps,
    )
    .unwrap();
    let a = Allocation {
        owner: owner(),
        generation: 1,
        provider: "bridge".into(),
        spec: spec(),
        launch: None,
    };
    assert!(provider.ensure(&a).await.unwrap().ready);
    for (m, code) in [
        (1, FailureCode::Identity),
        (2, FailureCode::Unauthorized),
        (3, FailureCode::Timeout),
        (4, FailureCode::NotFound),
    ] {
        mode.store(m, Ordering::SeqCst);
        let err = provider.ensure(&a).await.unwrap_err();
        assert_eq!(err.code, code);
        assert!(!format!("{err:?} {err}").contains("secret-token"));
        assert!(!format!("{err:?}").contains("127.0.0.1"));
    }
    server.abort();
}

struct Initializer {
    calls: AtomicUsize,
    seeds: Mutex<Vec<AllocationSeed>>,
    hang: AtomicBool,
    entered: tokio::sync::Notify,
}
impl Default for Initializer {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            seeds: Mutex::new(vec![]),
            hang: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
        }
    }
}
impl AllocationInitializer for Initializer {
    fn initialize<'a>(
        &'a self,
        seed: &'a AllocationSeed,
    ) -> ProviderFuture<'a, WorkerLaunchDescriptor> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seeds.lock().unwrap().push(seed.clone());
            self.entered.notify_one();
            if self.hang.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            use base64::Engine;
            WorkerLaunchDescriptor::new(
                format!("runtime-{}", seed.generation),
                "/workspace",
                "/worker-state",
                i64::MAX,
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7; 32]),
            )
            .map_err(|_| ProviderError::new(FailureCode::Protocol, false))
        })
    }
}
#[tokio::test]
async fn initializer_is_once_after_commit_and_generation_is_locked() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let init = Initializer::default();
    p.lose_reply.store(true, Ordering::SeqCst);
    assert!(runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .is_err());
    let b = runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .unwrap();
    assert_eq!(init.calls.load(Ordering::SeqCst), 1);
    assert_eq!(p.creates.load(Ordering::SeqCst), 1);
    assert_eq!(
        b.allocation.launch.as_ref().unwrap().runtime_id(),
        "runtime-1"
    );
    let mut changed = spec();
    changed.disk_gib += 1;
    assert!(matches!(
        runtime
            .ensure_with_initializer(&owner(), &changed, &init)
            .await,
        Err(Error::Conflict)
    ));
    runtime.destroy(&b.status.unwrap().handle).await.unwrap();
    let b = runtime
        .ensure_with_initializer(&owner(), &changed, &init)
        .await
        .unwrap();
    assert_eq!(b.allocation.generation, 2);
    assert_eq!(init.seeds.lock().unwrap()[1].generation, 2);
    assert_eq!(init.calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn cancellation_before_commit_replays_same_seed_without_provider_effects() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p.clone()).unwrap();
    let init = Initializer::default();
    init.hang.store(true, Ordering::SeqCst);
    let key = owner();
    let desired = spec();
    let mut future = Box::pin(runtime.ensure_with_initializer(&key, &desired, &init));
    tokio::select! { _ = &mut future => panic!("must wait"), _ = init.entered.notified() => {} }
    assert!(matches!(runtime.binding(&key), Err(Error::Busy)));
    drop(future);
    assert!(runtime.binding(&key).unwrap().is_none());
    assert_eq!(p.creates.load(Ordering::SeqCst), 0);
    init.hang.store(false, Ordering::SeqCst);
    runtime
        .ensure_with_initializer(&key, &desired, &init)
        .await
        .unwrap();
    let seeds = init.seeds.lock().unwrap();
    assert_eq!(seeds[0], seeds[1]);
}
#[tokio::test]
async fn machine_only_is_valid_but_never_auto_repaired() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p).unwrap();
    let init = Initializer::default();
    let b = runtime.ensure(&owner(), &spec()).await.unwrap();
    assert!(b.allocation.launch.is_none());
    assert_eq!(
        serde_json::to_value(&b.allocation).unwrap()["launch"],
        serde_json::Value::Null
    );
    assert!(matches!(
        runtime
            .ensure_with_initializer(&owner(), &spec(), &init)
            .await,
        Err(Error::MissingLaunch)
    ));
    assert_eq!(init.calls.load(Ordering::SeqCst), 0);
    runtime.destroy(&b.status.unwrap().handle).await.unwrap();
    assert!(runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .unwrap()
        .allocation
        .launch
        .is_some());
}
struct Authorizer;
impl BridgeAuthorizer for Authorizer {
    fn authorize<'a>(
        &'a self,
        bearer: &'a str,
        owner: &'a WorkspaceKey,
        action: RuntimeAction,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            if bearer == "workspace-token"
                && owner == &self::owner()
                && action != RuntimeAction::Destroy
            {
                Ok(())
            } else {
                Err(ProviderError::new(FailureCode::Unauthorized, false))
            }
        })
    }
}
#[tokio::test]
async fn reusable_bridge_enforces_auth_owner_operation_and_optional_inspection() {
    let p = Arc::new(Fake::default());
    let app = provider_router(p.clone(), Arc::new(Authorizer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let a = Allocation {
        owner: owner(),
        generation: 1,
        provider: "fake".into(),
        spec: spec(),
        launch: None,
    };
    let request = ProtocolRequest::new(RuntimeAction::Ensure, a.clone(), None).unwrap();
    let url = format!("http://{addr}/v2/runtime/ensure");
    assert_eq!(
        client
            .post(&url)
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("privileged-admin")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut wrong = request.clone();
    wrong.allocation.owner.tenant = "wrong".into();
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("workspace-token")
            .json(&wrong)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(p.creates.load(Ordering::SeqCst), 0);
    let status: ProtocolResponse = client
        .post(&url)
        .bearer_auth("workspace-token")
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let request = ProtocolRequest::new(RuntimeAction::Inspect, a.clone(), None).unwrap();
    assert_eq!(
        client
            .post(format!("http://{addr}/v2/runtime/inspect"))
            .bearer_auth("workspace-token")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let request =
        ProtocolRequest::new(RuntimeAction::Destroy, a, Some(status.status.handle))
            .unwrap();
    assert_eq!(
        client
            .post(format!("http://{addr}/v2/runtime/destroy"))
            .bearer_auth("workspace-token")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(format!("http://{addr}/v1/runtime/ensure"))
            .bearer_auth("workspace-token")
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    task.abort();
}

#[tokio::test]
async fn expired_active_launch_requires_confirmed_destroy_before_new_generation() {
    let temp = Temp::new();
    let p = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, p).unwrap();
    let init = Initializer::default();
    let b = runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .unwrap();
    let record = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    data["binding"]["allocation"]["launch"]["expires_at"] = 1.into();
    std::fs::write(record, serde_json::to_vec(&data).unwrap()).unwrap();
    assert!(runtime.binding(&owner()).unwrap().is_some());
    assert!(matches!(
        runtime
            .ensure_with_initializer(&owner(), &spec(), &init)
            .await,
        Err(Error::ExpiredLaunch)
    ));
    assert!(runtime.reconcile(&owner()).await.is_ok());
    assert!(matches!(
        runtime.start(&b.status.as_ref().unwrap().handle).await,
        Err(Error::ExpiredLaunch)
    ));
    assert_eq!(init.calls.load(Ordering::SeqCst), 1);
    runtime.destroy(&b.status.unwrap().handle).await.unwrap();
    assert_eq!(
        runtime
            .ensure_with_initializer(&owner(), &spec(), &init)
            .await
            .unwrap()
            .allocation
            .generation,
        2
    );
}
struct CapabilityProvider(Capabilities);
impl RuntimeProvider for CapabilityProvider {
    fn id(&self) -> &str {
        "capability-provider"
    }
    fn capabilities(&self) -> Capabilities {
        self.0
    }
    fn ensure<'a>(&'a self, _: &'a Allocation) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async { unreachable!() })
    }
    fn inspect<'a>(
        &'a self,
        _: &'a Allocation,
        _: Option<&'a MachineHandle>,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async { unreachable!() })
    }
    fn start<'a>(
        &'a self,
        _: &'a Allocation,
        _: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async { unreachable!() })
    }
    fn stop<'a>(
        &'a self,
        _: &'a Allocation,
        _: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async { unreachable!() })
    }
    fn destroy<'a>(
        &'a self,
        _: &'a Allocation,
        _: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(async { unreachable!() })
    }
}
#[test]
fn production_requires_vm_and_all_limits_development_is_explicit() {
    let temp = Temp::new();
    let base = Capabilities {
        isolation: IsolationKind::VirtualMachine,
        stop_start: true,
        cpu_limit: true,
        memory_limit: true,
        disk_limit: true,
        durable_workspace: true,
    };
    assert!(Runtime::new(&temp.0, Arc::new(CapabilityProvider(base))).is_ok());
    for field in 0..5 {
        let mut caps = base;
        match field {
            0 => caps.isolation = IsolationKind::Container,
            1 => caps.cpu_limit = false,
            2 => caps.memory_limit = false,
            3 => caps.disk_limit = false,
            _ => caps.durable_workspace = false,
        }
        assert!(matches!(
            Runtime::new(&temp.0, Arc::new(CapabilityProvider(caps))),
            Err(Error::Invalid)
        ));
    }
    let mut dev = base;
    dev.isolation = IsolationKind::Container;
    dev.disk_limit = false;
    assert!(Runtime::new_development(&temp.0, Arc::new(CapabilityProvider(dev))).is_ok());
    dev.cpu_limit = false;
    assert!(
        Runtime::new_development(&temp.0, Arc::new(CapabilityProvider(dev))).is_err()
    );
}

fn expire_record(temp: &Temp) -> std::path::PathBuf {
    let path = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["binding"]["allocation"]["launch"]["expires_at"] = 1.into();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    path
}
#[tokio::test]
async fn expired_lost_create_reply_recovers_only_by_inspection_then_destroy() {
    let temp = Temp::new();
    let provider = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, provider.clone()).unwrap();
    let init = Initializer::default();
    provider.lose_reply.store(true, Ordering::SeqCst);
    assert!(runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .is_err());
    let b = runtime.binding(&owner()).unwrap().unwrap();
    assert!(b.status.is_none());
    assert_eq!(b.pending, Some(Intent::Ensure));
    expire_record(&temp);
    let expired = runtime.binding(&owner()).unwrap().unwrap();
    assert!(matches!(
        runtime
            .ensure_with_initializer(&owner(), &spec(), &init)
            .await,
        Err(Error::ExpiredLaunch)
    ));
    let recovered = runtime.recover_handle(&owner()).await.unwrap();
    assert_eq!(recovered.allocation, expired.allocation);
    assert_eq!(recovered.pending, expired.pending);
    assert_eq!(recovered.last_error, expired.last_error);
    let handle = recovered.status.as_ref().unwrap().handle.clone();
    assert!(matches!(
        runtime.start(&handle).await,
        Err(Error::ExpiredLaunch)
    ));
    // Reconcile must not replay the expired Ensure, even after discovering its handle.
    runtime.reconcile(&owner()).await.unwrap();
    assert_eq!(provider.ensures.load(Ordering::SeqCst), 1);
    assert_eq!(provider.starts.load(Ordering::SeqCst), 0);
    assert_eq!(init.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider
            .machines
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.state != MachineState::Destroyed)
            .count(),
        1
    );
    runtime.destroy(&handle).await.unwrap();
    let next = runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .unwrap();
    assert_eq!(next.allocation.generation, 2);
    assert_ne!(next.allocation.launch, expired.allocation.launch);
    assert_eq!(init.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        provider
            .machines
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.state != MachineState::Destroyed)
            .count(),
        1
    );
}
#[tokio::test]
async fn failed_or_foreign_recovery_never_changes_registry_or_infers_retirement() {
    let temp = Temp::new();
    let provider = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, provider.clone()).unwrap();
    let init = Initializer::default();
    provider.lose_reply.store(true, Ordering::SeqCst);
    assert!(runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .is_err());
    let path = expire_record(&temp);
    for mode in [1, 2, 3, 5, 6] {
        let before = std::fs::read(&path).unwrap();
        provider.inspect_mode.store(mode, Ordering::SeqCst);
        assert!(runtime.recover_handle(&owner()).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!runtime.binding(&owner()).unwrap().unwrap().retired);
    }
    provider.inspect_mode.store(0, Ordering::SeqCst);
    runtime.recover_handle(&owner()).await.unwrap();
    // With a known handle, a different machine reference also fails closed.
    provider.inspect_mode.store(4, Ordering::SeqCst);
    let before = std::fs::read(&path).unwrap();
    assert!(runtime.recover_handle(&owner()).await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[tokio::test]
async fn expired_pending_destroy_reconciles_without_renewal_or_recreation() {
    let temp = Temp::new();
    let provider = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, provider.clone()).unwrap();
    let init = Initializer::default();
    let b = runtime
        .ensure_with_initializer(&owner(), &spec(), &init)
        .await
        .unwrap();
    let handle = b.status.unwrap().handle;
    provider.hang.store(true, Ordering::SeqCst);
    let mut destroy = Box::pin(runtime.destroy(&handle));
    tokio::select! { _ = &mut destroy => panic!("must wait"), _ = provider.entered.notified() => {} }
    drop(destroy);
    assert_eq!(
        runtime.binding(&owner()).unwrap().unwrap().pending,
        Some(Intent::Destroy)
    );
    expire_record(&temp);
    provider.hang.store(false, Ordering::SeqCst);
    let retired = runtime.reconcile(&owner()).await.unwrap();
    assert!(retired.retired);
    assert!(retired.pending.is_none());
    assert_eq!(provider.ensures.load(Ordering::SeqCst), 1);
    assert_eq!(provider.starts.load(Ordering::SeqCst), 0);
    assert_eq!(init.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repeated_observations_preserve_revision_but_health_changes_and_error_clears_save(
) {
    let temp = Temp::new();
    let provider = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, provider.clone()).unwrap();
    let other_actor = Runtime::new(&temp.0, provider.clone()).unwrap();
    let initial = runtime.ensure(&owner(), &spec()).await.unwrap();
    let path = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    // Independent brokers inspecting the same health do not invalidate each other's CAS.
    for _ in 0..3 {
        assert_eq!(runtime.reconcile(&owner()).await.unwrap(), initial);
        assert_eq!(other_actor.recover_handle(&owner()).await.unwrap(), initial);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    {
        let mut machines = provider.machines.lock().unwrap();
        let status = machines.get_mut(&key(&initial.allocation)).unwrap();
        status.state = MachineState::Running;
        status.ready = false;
        status.connection = Some(connection(&status.handle));
    }
    let changed = other_actor.recover_handle(&owner()).await.unwrap();
    assert_eq!(changed.revision, initial.revision + 1);
    assert_ne!(changed.status, initial.status);
    assert_eq!(runtime.reconcile(&owner()).await.unwrap(), changed);
    {
        let mut machines = provider.machines.lock().unwrap();
        machines.get_mut(&key(&initial.allocation)).unwrap().ready = true;
    }
    let ready = runtime.reconcile(&owner()).await.unwrap();
    assert_eq!(ready.revision, changed.revision + 1);
    provider.inspect_mode.store(5, Ordering::SeqCst);
    assert!(runtime.reconcile(&owner()).await.is_err());
    let failed = runtime.binding(&owner()).unwrap().unwrap();
    assert_eq!(failed.revision, ready.revision + 1);
    // Re-observing the exact same failure is also not a state transition.
    assert!(runtime.reconcile(&owner()).await.is_err());
    assert_eq!(runtime.binding(&owner()).unwrap().unwrap(), failed);
    provider.inspect_mode.store(0, Ordering::SeqCst);
    let cleared = runtime.reconcile(&owner()).await.unwrap();
    assert_eq!(cleared.status, ready.status);
    assert_eq!(cleared.revision, failed.revision + 1);
    assert!(cleared.last_error.is_none());
}
#[tokio::test]
async fn pending_ensure_completion_saves_even_when_machine_status_is_unchanged() {
    let temp = Temp::new();
    let provider = Arc::new(Fake::default());
    let runtime = Runtime::new(&temp.0, provider.clone()).unwrap();
    provider.lose_reply.store(true, Ordering::SeqCst);
    assert!(runtime.ensure(&owner(), &spec()).await.is_err());
    let discovered = runtime.recover_handle(&owner()).await.unwrap();
    assert_eq!(discovered.pending, Some(Intent::Ensure));
    assert!(discovered.last_error.is_some());
    assert_eq!(runtime.recover_handle(&owner()).await.unwrap(), discovered);
    let completed = runtime.reconcile(&owner()).await.unwrap();
    assert_eq!(completed.status, discovered.status);
    assert_eq!(completed.revision, discovered.revision + 1);
    assert!(completed.pending.is_none());
    assert!(completed.last_error.is_none());
    assert_eq!(runtime.reconcile(&owner()).await.unwrap(), completed);
}
