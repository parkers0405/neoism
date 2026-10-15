//! Real-router, headless desktop transport contracts.
//!
//! Repeat from the repository root (no GUI, release build, agent, or tunnel):
//! `bash scripts/test-server-contracts.sh`
//!
//! The fixture serves `workspace_daemon::server::router(AppState)` and the
//! production Fast ingress handlers, not an imitation authentication predicate.
//! Only temporary configuration/registry/auth data and ephemeral loopback ports
//! are used. A single test owns environment changes until its Tokio runtime has
//! shut down. This covers wire readiness before registry insertion and desktop
//! startup rejection/transport exit and the source Fast launcher with an owned
//! `/bin/false` early-exit child. The real ordinary-host startup driver is also
//! exercised with bounded, owned acknowledgement/exit fixtures, using the actual
//! standalone protocol type and constants. These children are not auth servers.
//! Registry insertion is fixture setup, not a UI installation-order test. This
//! does NOT cover standalone agent association, installed binary startup/exit
//! capture, root hosting eligibility, or public relay provisioning.

// Only the separately registered headless package enables this feature. Keep
// desktop Cargo's automatic test discovery from compiling this package's shim.
#![cfg(feature = "server-transport-contract")]

#[allow(dead_code)]
#[path = "../src/fast_servers.rs"]
mod fast_servers;

#[path = "support/ordinary_host_driver.rs"]
mod ordinary_host_driver;
#[path = "../src/standalone_executable.rs"]
mod service_process;

use neoism_protocol::workspace::WorkspaceServerMessage;
use neoism_server_transport_contract::daemon_client::{
    DaemonClient, DaemonClientOptions, DaemonClientStatus, DaemonEndpoint,
    DaemonServerMessage, DaemonUpgradeAuth,
};
use neoism_workspace_daemon::{
    auth::AuthService,
    handshake::PairingTokenStore,
    server::{self, AppState},
    workspace::WorkspaceManager,
};
use std::{
    ffi::OsString,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::task::JoinHandle;

const MASTER: &str = "contract-private-master-not-a-paired-device";
const GUEST: &str = "contract-guest-not-the-private-master";
const BOUND: Duration = Duration::from_secs(4);
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Environment {
    previous: Vec<(&'static str, Option<OsString>)>,
    directory: tempfile::TempDir,
}
impl Environment {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let values = [
            (
                "NEOISM_WORKSPACE_ROOT",
                directory.path().as_os_str().to_owned(),
            ),
            ("HOME", directory.path().as_os_str().to_owned()),
            ("NEOISM_REQUIRE_AUTH", OsString::from("1")),
            ("NEOISM_DAEMON_TOKEN", OsString::from(MASTER)),
            (
                "NEOISM_CONFIG_DIR",
                directory.path().join("config").into_os_string(),
            ),
            (
                "NEOISM_DAEMON_DATA_DIR",
                directory.path().join("auth").into_os_string(),
            ),
            (
                "NEOISM_WORKSPACE_REGISTRY",
                directory.path().join("workspaces.json").into_os_string(),
            ),
            (
                "XDG_CONFIG_HOME",
                directory.path().join("xdg-config").into_os_string(),
            ),
            (
                "XDG_DATA_HOME",
                directory.path().join("xdg-data").into_os_string(),
            ),
        ];
        let mut previous = Vec::new();
        for (key, value) in values {
            previous.push((key, std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        Self {
            previous,
            directory,
        }
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct Daemon {
    endpoint: String,
    task: JoinHandle<()>,
}
impl Daemon {
    async fn start(env: &Environment) -> Self {
        let workspaces = WorkspaceManager::bootstrap();
        workspaces.declare_startup_workspace(env.directory.path());
        let app = server::router(AppState {
            auth: AuthService::bootstrap(&env.directory.path().join("auth")).unwrap(),
            workspaces,
            pairing_tokens: PairingTokenStore::in_memory(),
            sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
            crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
            paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
            lsp_runtime: neoism_agent_server::language_server::LspRuntime::new(
                neoism_agent_neoism_adapter::neoism_services(),
            ),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/session", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self { endpoint, task }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// A transparent wire tap, not an auth server: every upgrade/Hello decision is
// still made by the real daemon upstream. Record the exact frame the production
// Fast proxy sends, because query preauthentication alone would otherwise hide
// a missing Hello-token rewrite.
async fn private_wire_tap(
    upstream: String,
) -> (
    Daemon,
    tokio::sync::mpsc::Receiver<String>,
    Arc<tokio::sync::Semaphore>,
) {
    use axum::{
        extract::{ws::WebSocketUpgrade, RawQuery, State},
        routing::get,
    };
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    type TapState = (
        String,
        tokio::sync::mpsc::Sender<String>,
        Arc<tokio::sync::Semaphore>,
        Arc<AtomicBool>,
    );
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let once = Arc::new(AtomicBool::new(true));
    let app = axum::Router::new().route("/session", get(
        |ws: WebSocketUpgrade, RawQuery(query): RawQuery,
         State((upstream, tx, release, once)): State<TapState>| async move {
            ws.on_upgrade(move |mut guest| async move {
                let url = format!("{upstream}?{}", query.unwrap_or_default());
                let (mut private, _) = tokio_tungstenite::connect_async(url).await.unwrap();
                let mut recorded = false;
                loop {
                    tokio::select! {
                        frame = guest.recv() => match frame {
                            Some(Ok(axum::extract::ws::Message::Text(text))) => {
                                if !recorded {
                                    tx.send(text.clone()).await.unwrap();
                                    recorded = true;
                                }
                                if private.send(Message::Text(text)).await.is_err() { break; }
                            }
                            _ => break,
                        },
                        frame = private.next() => match frame {
                            Some(Ok(Message::Text(text))) => {
                                if once.swap(false, Ordering::AcqRel) {
                                    let _permit = release.acquire().await.unwrap();
                                }
                                if guest.send(axum::extract::ws::Message::Text(text)).await.is_err() { break; }
                            }
                            Some(Ok(Message::Ping(bytes))) => {
                                if private.send(Message::Pong(bytes)).await.is_err() { break; }
                            }
                            Some(Ok(Message::Pong(_))) => {},
                            _ => break,
                        },
                    }
                }
            })
        }
    )).with_state((upstream, tx, Arc::clone(&release), once));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/session", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (Daemon { endpoint, task }, rx, release)
}

async fn assert_private_hello(rx: &mut tokio::sync::mpsc::Receiver<String>) {
    let frame = tokio::time::timeout(BOUND, rx.recv())
        .await
        .unwrap()
        .unwrap();
    let hello: serde_json::Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(
        hello.pointer("/Workspace/message/Hello/token").unwrap(),
        MASTER
    );
}

fn options(endpoint: &str, token: Option<&str>) -> DaemonClientOptions {
    let mut options = DaemonClientOptions::new(DaemonEndpoint::parse(endpoint).unwrap());
    options.token = token.map(str::to_owned);
    options.reconnect.handshake = Duration::from_secs(1);
    options
}

async fn workspace_reply(client: &mut DaemonClient) -> WorkspaceServerMessage {
    tokio::time::timeout(BOUND, async {
        loop {
            if let DaemonServerMessage::Workspace { message, .. } =
                client.recv().await.expect("daemon reply channel closed")
            {
                return message;
            }
        }
    })
    .await
    .expect("bounded real-daemon workspace reply")
}

async fn authenticated_snapshot(options: DaemonClientOptions) {
    let mut client = DaemonClient::connect_with_options(options).await.unwrap();
    assert!(matches!(
        workspace_reply(&mut client).await,
        WorkspaceServerMessage::HelloAck { accepted: true, .. }
    ));
    assert!(matches!(
        workspace_reply(&mut client).await,
        WorkspaceServerMessage::FullSnapshot { .. }
    ));
    assert_eq!(client.status(), DaemonClientStatus::Open);
}

async fn assert_startup_failure(options: DaemonClientOptions, expected: &str) {
    let client = DaemonClient::connect_with_options(options).await.unwrap();
    let mut status = client.status_receiver();
    tokio::time::timeout(BOUND, async {
        loop {
            let current = *status.borrow_and_update();
            assert_ne!(
                current,
                DaemonClientStatus::Open,
                "failed startup must never become ready"
            );
            if current == DaemonClientStatus::Closed {
                break;
            }
            status
                .changed()
                .await
                .expect("runner reports terminal startup failure");
        }
    })
    .await
    .expect("startup must fail promptly, not remain connecting");
    assert_eq!(
        client.transport_contract_startup_error().as_deref(),
        Some(expected)
    );
}

#[test]
fn contract_real_daemon_and_fast_ingress() {
    let _serial = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let env = Environment::new();
    // Runtime is dropped before environment restoration, including after panic.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let early = tokio::time::timeout(
            BOUND,
            fast_servers::transport_contract::early_exit(env.directory.path().to_owned()),
        )
        .await
        .expect("owned early-exit child must not hang startup")
        .unwrap_err();
        assert!(
            early.contains("workspace daemon exited before readiness")
                || early.contains("invalid or closed daemon readiness protocol"),
            "{early}"
        );
        assert!(!early.contains(MASTER));
        assert!(!early.contains(GUEST));
        let mut daemon = Daemon::start(&env).await;
        let ordinary = options(&daemon.endpoint, Some(MASTER));
        assert_eq!(ordinary.upgrade_auth, DaemonUpgradeAuth::HelloOnly);
        authenticated_snapshot(ordinary).await;

        // Query preauthentication is independent of Hello credentials. In
        // particular it must not accidentally acquire a master-token Bearer.
        authenticated_snapshot(options(
            &format!("{}?token={MASTER}", daemon.endpoint),
            None,
        ))
        .await;
        authenticated_snapshot(options(
            &format!("{}?token={MASTER}", daemon.endpoint),
            Some(MASTER),
        ))
        .await;

        // Negative control: the actual session_routes bearer branch rejects a
        // master token because it is NOT a paired-device bearer. This is what
        // makes a global-Bearer regression in the default runner fail above.
        let mut invalid_bearer = options(&daemon.endpoint, Some(MASTER));
        invalid_bearer.upgrade_auth = DaemonUpgradeAuth::FastBearer;
        assert_startup_failure(
            invalid_bearer,
            "daemon authentication rejected (HTTP 401)",
        )
        .await;
        let mut query_bearer =
            options(&format!("{}?token={MASTER}", daemon.endpoint), Some(MASTER));
        query_bearer.upgrade_auth = DaemonUpgradeAuth::FastBearer;
        assert_startup_failure(query_bearer, "daemon authentication rejected (HTTP 401)")
            .await;
        assert_startup_failure(
            options(&daemon.endpoint, Some("wrong-master")),
            "daemon authentication rejected",
        )
        .await;

        let (tap, mut hellos, release) = private_wire_tap(daemon.endpoint.clone()).await;
        let ingress = fast_servers::transport_contract::LocalIngress::start(
            tap.endpoint.clone(),
            env.directory.path().to_owned(),
            MASTER,
            GUEST,
        )
        .await;
        assert!(ingress.ready("wrong-guest").await.is_err());
        let mut readiness = ingress.ready_task(GUEST);
        assert_private_hello(&mut hellos).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut readiness)
                .await
                .is_err(),
            "readiness must wait for the real daemon's HelloAck, not just upgrade"
        );
        release.add_permits(1);
        tokio::time::timeout(BOUND, readiness)
            .await
            .expect("bounded readiness task")
            .expect("readiness task panicked")
            .expect("readiness includes authenticated private HelloAck");

        let registry_dir = tempfile::tempdir().unwrap();
        let mut registry =
            neoism_server_transport_contract::server_registry::ServerRegistry::load(
                registry_dir.path().to_owned(),
            )
            .unwrap();
        assert!(
            registry.servers().is_empty(),
            "readiness precedes any registration"
        );
        let mut fast = options(&ingress.endpoint, Some(GUEST));
        fast.upgrade_auth = DaemonUpgradeAuth::FastBearer;
        authenticated_snapshot(fast).await;
        assert_private_hello(&mut hellos).await;
        let entry = registry
            .add_temporary(&ingress.endpoint, Some("contract"), Some(GUEST), false)
            .unwrap();
        assert!(registry.is_temporary(&entry.id));
        assert_eq!(registry.token(&entry.id), Some(GUEST));
        registry.remove(&entry.id).unwrap();
        // A guest token cannot authenticate directly against the private daemon;
        // successful Fast Hello therefore requires the real filter's rewrite.
        assert_startup_failure(
            options(&daemon.endpoint, Some(GUEST)),
            "daemon authentication rejected",
        )
        .await;
        assert_startup_failure(
            options(&ingress.endpoint, Some("wrong-guest")),
            "daemon authentication rejected (HTTP 401)",
        )
        .await;
        ingress.end();
        let mut ended = options(&ingress.endpoint, Some(GUEST));
        ended.upgrade_auth = DaemonUpgradeAuth::FastBearer;
        assert_startup_failure(ended, "daemon session unavailable (HTTP 410)").await;
        // Drop the real listener before dialing: no mock close frame or public
        // service. A transport exit must not report authenticated readiness.
        let offline_endpoint = daemon.endpoint.clone();
        daemon.task.abort();
        let _ = (&mut daemon.task).await;
        let mut offline =
            DaemonClient::connect_with_options(options(&offline_endpoint, Some(MASTER)))
                .await
                .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), offline.recv())
                .await
                .is_err()
        );
        assert_ne!(offline.status(), DaemonClientStatus::Open);
    });
}

#[path = "support/document_sync_contract.rs"]
mod document_sync_contract;
