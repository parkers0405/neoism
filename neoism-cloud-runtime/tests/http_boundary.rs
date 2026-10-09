use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, post},
    Json, Router,
};
use neoism_cloud_runtime::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn allocation() -> Allocation {
    Allocation {
        owner: WorkspaceKey::new("tenant", "workspace").unwrap(),
        generation: 2,
        provider: "bridge".into(),
        spec: WorkspaceSpec {
            image: "ubuntu".into(),
            region: "east".into(),
            vcpus: 2,
            memory_mib: 2048,
            disk_gib: 20,
        },
        launch: None,
    }
}
fn handle() -> MachineHandle {
    let a = allocation();
    MachineHandle {
        owner: a.owner,
        generation: a.generation,
        provider: a.provider,
        machine_id: "vm-1".into(),
    }
}
fn provider(endpoint: &str) -> HttpProvider {
    HttpProvider::new_plaintext(
        "bridge",
        endpoint,
        "private-bearer",
        Duration::from_secs(2),
        Capabilities {
            isolation: IsolationKind::VirtualMachine,
            cpu_limit: true,
            memory_limit: true,
            disk_limit: true,
            durable_workspace: true,
            stop_start: true,
        },
    )
    .unwrap()
}
#[derive(Default)]
struct Modes {
    mode: AtomicUsize,
    calls: AtomicUsize,
    leaks: AtomicUsize,
}
async fn mock(
    State(m): State<Arc<Modes>>,
    Path(action): Path<String>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Response {
    m.calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        headers.get("authorization").unwrap(),
        "Bearer private-bearer"
    );
    assert_eq!(request["version"], 2);
    assert_eq!(
        request["allocation"],
        serde_json::to_value(allocation()).unwrap()
    );
    if action == "ensure" {
        assert!(request["handle"].is_null());
    } else {
        assert_eq!(request["handle"], serde_json::to_value(handle()).unwrap());
    }
    match m.mode.load(Ordering::SeqCst) {
        1 => {
            return (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", "/leak")],
                "private-bearer",
            )
                .into_response()
        }
        2 => return (StatusCode::FORBIDDEN, "private-bearer").into_response(),
        3 => return (StatusCode::OK, "not-json private-bearer").into_response(),
        4 => return (StatusCode::OK, "x".repeat(65_537)).into_response(),
        5 => return StatusCode::NO_CONTENT.into_response(),
        6 => return StatusCode::CONFLICT.into_response(),
        7 => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        _ => {}
    }
    let mut h = handle();
    if m.mode.load(Ordering::SeqCst) == 8 {
        h.machine_id = "wrong-vm".into();
    }
    let state = match action.as_str() {
        "destroy" => MachineState::Destroyed,
        "stop" => MachineState::Stopped,
        _ => MachineState::Running,
    };
    let status = MachineStatus {
        connection: if state == MachineState::Running {
            Some(
                WorkerConnection::new(h.clone(), "https://worker.example/agent/")
                    .unwrap(),
            )
        } else {
            None
        },
        handle: h,
        state,
        ready: state == MachineState::Running,
        failure: None,
    };
    let mut response = serde_json::json!({"version": 2, "status": status});
    match m.mode.load(Ordering::SeqCst) {
        9 => response["version"] = 1.into(),
        10 => response["secret"] = "private-bearer".into(),
        11 => response["status"]["connection"]["handle"]["generation"] = 1.into(),
        12 => {
            response["status"]["connection"]["agent_api_base_url"] =
                "https://user:private-bearer@worker.example/".into()
        }
        _ => {}
    }
    (StatusCode::ACCEPTED, Json(response)).into_response()
}
async fn leak(State(m): State<Arc<Modes>>) -> StatusCode {
    m.leaks.fetch_add(1, Ordering::SeqCst);
    StatusCode::OK
}

#[tokio::test]
async fn real_http_actions_and_adversarial_responses() {
    let modes = Arc::new(Modes::default());
    let router = Router::new()
        .route("/bridge/v2/runtime/:action", post(mock))
        .route("/leak", any(leak))
        .with_state(modes.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/bridge/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let p = provider(&endpoint);
    let a = allocation();
    let h = handle();
    let ensured = p.ensure(&a).await.unwrap();
    assert_eq!(ensured.connection.unwrap().handle(), &h);
    p.inspect(&a, Some(&h)).await.unwrap();
    p.start(&a, &h).await.unwrap();
    assert_eq!(p.stop(&a, &h).await.unwrap().state, MachineState::Stopped);
    assert_eq!(
        p.destroy(&a, &h).await.unwrap().state,
        MachineState::Destroyed
    );
    for (mode, code, retryable) in [
        (1, FailureCode::Rejected, false),
        (2, FailureCode::Unauthorized, false),
        (3, FailureCode::Protocol, false),
        (4, FailureCode::Protocol, false),
        (5, FailureCode::Rejected, false),
        (6, FailureCode::Conflict, false),
        (7, FailureCode::Unavailable, true),
        (8, FailureCode::Identity, false),
        (9, FailureCode::Protocol, false),
        (10, FailureCode::Protocol, false),
        (11, FailureCode::Identity, false),
        (12, FailureCode::Protocol, false),
    ] {
        modes.mode.store(mode, Ordering::SeqCst);
        let error = p.inspect(&a, Some(&h)).await.unwrap_err();
        assert_eq!(error.code, code, "mode={mode}");
        assert_eq!(error.retryable, retryable);
        assert!(!format!("{error:?} {error}").contains("private-bearer"));
    }
    assert_eq!(
        modes.leaks.load(Ordering::SeqCst),
        0,
        "redirect must never be followed"
    );
    let calls = modes.calls.load(Ordering::SeqCst);
    let mut foreign = h.clone();
    foreign.owner.tenant = "foreign".into();
    assert_eq!(
        p.start(&a, &foreign).await.unwrap_err().code,
        FailureCode::Identity
    );
    assert_eq!(
        modes.calls.load(Ordering::SeqCst),
        calls,
        "foreign input must not hit bridge"
    );
    server.abort();
}

#[tokio::test]
async fn chunked_response_cannot_bypass_size_limit() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        // Drain the request body before closing, to avoid a TCP reset obscuring the test.
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers =
                    String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let len: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + len {
                    break;
                }
            }
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n").await.unwrap();
        let bytes = vec![b'x'; 32_768];
        for _ in 0..3 {
            if socket.write_all(b"8000\r\n").await.is_err() {
                return;
            }
            if socket.write_all(&bytes).await.is_err() {
                return;
            }
            if socket.write_all(b"\r\n").await.is_err() {
                return;
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    assert_eq!(
        provider(&endpoint)
            .ensure(&allocation())
            .await
            .unwrap_err()
            .code,
        FailureCode::Protocol
    );
    server.await.unwrap();
}

#[test]
fn bridge_configuration_has_no_implicit_endpoint_or_credential_url() {
    let caps = Capabilities {
        isolation: IsolationKind::VirtualMachine,
        cpu_limit: true,
        memory_limit: true,
        disk_limit: true,
        durable_workspace: true,
        stop_start: true,
    };
    for endpoint in [
        "",
        "https://user:pass@bridge.example/",
        "https://bridge.example/?token=x",
        "https://bridge.example/#fragment",
        "file:///bridge",
        "https://bridge.example/\n",
    ] {
        assert!(
            HttpProvider::new(
                "bridge",
                endpoint,
                "private-bearer",
                Duration::from_secs(1),
                caps
            )
            .is_err(),
            "endpoint={endpoint:?}"
        );
    }
}
