use super::*;
use axum::body::{Body, Bytes};
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use mcp_events::{EventStreamError, EventType, InvalidWire, PushMessage};

const TIMEOUT: Duration = Duration::from_secs(5);
const EVENTS: &str = "io.modelcontextprotocol/events";
const REVISION: &str = "2026-07-28";
const SUBSCRIPTION: &str = "io.modelcontextprotocol/subscriptionId";

fn notification(request: &Value, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({SUBSCRIPTION: request["id"]});
    json!({"jsonrpc": "2.0", "method": format!("notifications/events/{method}"), "params": params})
}

fn active(request: &Value, heartbeat_ms: u64) -> Value {
    notification(
        request,
        "active",
        json!({"heartbeatMs": heartbeat_ms, "cursor": "latest"}),
    )
}

fn frame(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\r\n\r\n"))
}

fn sse(body: Body) -> Response {
    ([(CONTENT_TYPE, "text/event-stream; charset=utf-8")], body).into_response()
}

async fn streaming_fixture(
    handler: impl Fn(Value) -> Response + Clone + Send + Sync + 'static,
    deadline: Duration,
) -> (Fixture, HttpMcpEvents, EventType) {
    let fixture = fixture(move |headers, request| match request["method"].as_str().unwrap() {
        "server/discover" => Json(discover(&request, &[REVISION], json!({EVENTS: {}}))).into_response(),
        "events/list" => Json(result(&request, json!({"events": [{
            "name": "issue.updated", "delivery": ["push"], "inputSchema": {}, "payloadSchema": {}
        }]}))).into_response(),
        "events/stream" => {
            assert_eq!(headers["mcp-method"], "events/stream");
            assert_eq!(headers["mcp-protocol-version"], REVISION);
            assert_eq!(request["params"]["name"], "issue.updated");
            handler(request)
        }
        _ => panic!("unexpected stream RPC"),
    }).await;
    let client = HttpMcpEvents::connect(&fixture.url, None, None, deadline)
        .await
        .unwrap()
        .unwrap();
    let event = client.list_page(None).await.unwrap().events.pop().unwrap();
    (fixture, client, event)
}

#[tokio::test]
async fn events_stream_is_incremental_cancellable_and_reopens_only_from_explicit_cursor()
{
    let (opened, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let (_fixture, client, event) = streaming_fixture(
        move |request| {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            let value = active(&request, 4000).to_string();
            // CR/LF split across chunks, multi-line data, BOM, and comment.
            let opening = format!(
                "\u{feff}: comment\r\n\r\ndata: {{\r\ndata: {}\r",
                &value[1..]
            );
            tx.try_send(Ok::<_, std::convert::Infallible>(Bytes::from(opening)))
                .unwrap();
            tx.try_send(Ok(Bytes::from_static(b"\n\r\n"))).unwrap();
            opened.send((request, tx)).unwrap();
            sse(Body::from_stream(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
        },
        Duration::from_secs(1),
    )
    .await;

    let mut stream = client.subscribe(&event, None, None, None).await.unwrap();
    let (first, producer) = requests.try_recv().unwrap();
    assert!(first["params"].get("cursor").is_none());
    assert!(first["params"].get("maxAgeMs").is_none());
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(PushMessage::Active(_))
    ));
    // The finite request deadline does not apply once active; no EOF is needed to deliver an event.
    tokio::time::sleep(Duration::from_millis(1050)).await;
    producer.send(Ok(frame(notification(&first, "event", json!({
        "name": "issue.updated", "eventId": "evt-1", "timestamp": "2026-07-28T10:00:00Z",
        "cursor": "committed", "data": {"issue": 1}
    }))))).await.unwrap();
    let Some(PushMessage::Event(delivered)) = stream.next().await.unwrap() else {
        panic!("expected event")
    };
    assert_eq!(delivered.event_id, "evt-1");
    assert_eq!(delivered.cursor.as_deref(), Some("committed"));
    stream.cancel();
    tokio::time::timeout(TIMEOUT, producer.closed())
        .await
        .expect("cancel dropped HTTP response body");
    assert!(requests.try_recv().is_err());

    let mut reopened = client
        .subscribe(&event, None, delivered.cursor.as_deref(), None)
        .await
        .unwrap();
    let (second, producer) = requests.try_recv().unwrap();
    assert_ne!(second["id"], first["id"]);
    assert_eq!(second["params"]["cursor"], "committed");
    assert_eq!(second["params"]["maxAgeMs"], 3_600_000);
    assert!(matches!(
        reopened.next().await.unwrap(),
        Some(PushMessage::Active(_))
    ));
    producer
        .send(Ok(frame(result(&second, json!({})))))
        .await
        .unwrap();
    assert!(reopened.next().await.unwrap().is_none());
    tokio::time::timeout(TIMEOUT, producer.closed())
        .await
        .expect("completion dropped HTTP response body");
}

#[tokio::test]
async fn events_stream_requires_active_and_exact_subscription_id() {
    let (_fixture, client, event) = streaming_fixture(
        |request| {
            let reply = match request["params"]["cursor"].as_str().unwrap() {
                "pre-active" => notification(&request, "heartbeat", json!({})),
                "wrong-id" => {
                    let mut active = active(&request, 1000);
                    active["params"]["_meta"][SUBSCRIPTION] =
                        json!(request["id"].to_string());
                    active
                }
                "grant" => active(&request, 300_001),
                "refused" => {
                    return Json(json!({"jsonrpc": "2.0", "id": request["id"],
                "error": {"code": -32024, "message": "forbidden"}}))
                    .into_response()
                }
                "oversize" => {
                    return sse(Body::from_stream(tokio_stream::iter([
                        Ok::<_, std::convert::Infallible>(Bytes::from_static(b"data: ")),
                        Ok(Bytes::from(vec![b'x'; mcp_events::MAX_JSON_BYTES])),
                    ])))
                }
                _ => panic!("unexpected cursor"),
            };
            sse(Body::from(frame(reply)))
        },
        TIMEOUT,
    )
    .await;
    for cursor in ["pre-active", "wrong-id", "grant", "refused", "oversize"] {
        let error = client
            .subscribe(&event, None, Some(cursor), None)
            .await
            .err()
            .unwrap();
        let error = error.downcast::<EventStreamError>().unwrap();
        assert!(
            matches!(
                (cursor, error),
                ("pre-active", EventStreamError::ExpectedActive)
                    | (
                        "wrong-id",
                        EventStreamError::Wire(InvalidWire::MismatchedResponse)
                    )
                    | ("grant", EventStreamError::HeartbeatGrant)
                    | ("refused", EventStreamError::Rejected(-32024))
                    | ("oversize", EventStreamError::Wire(InvalidWire::TooLarge))
            ),
            "{cursor}"
        );
    }
}

#[tokio::test]
async fn events_stream_distinguishes_deadlines_and_unclean_end() {
    let (opened, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let (_fixture, client, event) = streaming_fixture(
        move |request| {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            match request["params"]["cursor"].as_str() {
                Some("no-active") => tx
                    .try_send(Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        b": idle\n\n",
                    )))
                    .unwrap(),
                _ => tx.try_send(Ok(frame(active(&request, 60)))).unwrap(),
            }
            opened.send((request, tx)).unwrap();
            sse(Body::from_stream(
                tokio_stream::wrappers::ReceiverStream::new(rx),
            ))
        },
        Duration::from_millis(250),
    )
    .await;
    let error = client
        .subscribe(&event, None, Some("no-active"), None)
        .await
        .err()
        .unwrap();
    assert!(matches!(
        error.downcast_ref::<EventStreamError>(),
        Some(EventStreamError::HandshakeTimeout)
    ));
    drop(requests.try_recv().unwrap());

    let mut stream = client.subscribe(&event, None, None, None).await.unwrap();
    let (_, tx) = requests.try_recv().unwrap();
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(PushMessage::Active(_))
    ));
    tx.send(Ok(Bytes::from_static(b": idle\n\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n"))).await.unwrap();
    assert!(matches!(
        stream.next().await,
        Err(EventStreamError::HeartbeatTimeout)
    ));
    assert!(stream.next().await.unwrap().is_none());
    tokio::time::timeout(TIMEOUT, tx.closed()).await.unwrap();

    let mut stream = client.subscribe(&event, None, None, None).await.unwrap();
    drop(requests.try_recv().unwrap());
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(PushMessage::Active(_))
    ));
    let error = stream.next().await.unwrap_err();
    assert!(matches!(error, EventStreamError::Disconnected));
    let reopened = client
        .reconnect(&error, &event, None, "last-committed", Some(60_000))
        .await
        .unwrap();
    let (request, _) = requests.try_recv().unwrap();
    assert_eq!(request["params"]["cursor"], "last-committed");
    assert_eq!(request["params"]["maxAgeMs"], 60_000);
    reopened.cancel();
}

#[tokio::test]
async fn events_stream_checks_event_name_and_distinguishes_recoverable_and_terminal_errors(
) {
    let (_fixture, client, event) = streaming_fixture(|request| {
        let terminal = match request["params"]["cursor"].as_str() {
            Some("bad-event") => notification(&request, "event", json!({
                "name": "other.event", "eventId": "bad", "timestamp": "2026-07-28T00:00:00Z", "data": {}
            })),
            _ => notification(&request, "terminated", json!({"error": {"code": -32024, "message": "revoked"}})),
        };
        sse(Body::from_stream(tokio_stream::iter([
            Ok::<_, std::convert::Infallible>(frame(active(&request, 1000))),
            Ok(frame(notification(&request, "error", json!({"error": {"code": -32000, "message": "temporary"}})))),
            Ok(frame(notification(&request, "active", json!({"heartbeatMs": 2000, "truncated": true, "cursor": "new"})))),
            Ok(frame(notification(&request, "heartbeat", json!({"cursor": "checked"})))),
            Ok(frame(terminal)),
        ])))
    }, TIMEOUT).await;
    for cursor in ["terminated", "bad-event"] {
        let mut stream = client
            .subscribe(&event, None, Some(cursor), Some(0))
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await.unwrap(),
            Some(PushMessage::Active(_))
        ));
        assert!(
            matches!(stream.next().await.unwrap(), Some(PushMessage::Error(error)) if error.code == -32000)
        );
        assert!(
            matches!(stream.next().await.unwrap(), Some(PushMessage::Active(active)) if active.truncated && active.heartbeat_ms == 2000)
        );
        assert!(
            matches!(stream.next().await.unwrap(), Some(PushMessage::Heartbeat(heartbeat)) if heartbeat.cursor.as_deref() == Some("checked"))
        );
        let error = stream.next().await.unwrap_err();
        let guarded = client
            .reconnect(&error, &event, None, "not-sent", None)
            .await
            .err()
            .unwrap();
        assert!(matches!(
            guarded.downcast_ref::<EventStreamError>(),
            Some(EventStreamError::ReconnectNotAllowed)
        ));
        match (cursor, error) {
            ("terminated", EventStreamError::Terminated(error)) => {
                assert_eq!(error.code, -32024)
            }
            ("bad-event", EventStreamError::UnexpectedEventName) => {}
            (_, error) => panic!("unexpected stream error: {error}"),
        }
        assert!(stream.next().await.unwrap().is_none());
    }
}

struct Fixture {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fixture(
    handler: impl Fn(HeaderMap, Value) -> Response + Clone + Send + Sync + 'static,
) -> Fixture {
    let app = Router::new().route(
        "/mcp",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let handler = handler.clone();
            async move { handler(headers, body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fixture { url, task }
}

fn result(request: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
}

fn discover(request: &Value, versions: &[&str], extensions: Value) -> Value {
    result(
        request,
        json!({
            "resultType": "complete", "supportedVersions": versions,
            "capabilities": {"extensions": extensions}
        }),
    )
}

#[tokio::test]
async fn events_opt_in_negotiates_and_reads_typed_pages_over_http() {
    let fixture = fixture(|headers, request| {
        let method = request["method"].as_str().unwrap();
        let meta = &request["params"]["_meta"];
        assert_eq!(headers["mcp-method"], method);
        assert_eq!(headers["mcp-protocol-version"], REVISION);
        assert_eq!(headers[AUTHORIZATION], "Bearer fixture-token");
        assert!(!headers.contains_key("mcp-session-id"));
        assert_eq!(meta["io.modelcontextprotocol/protocolVersion"], REVISION);
        assert_eq!(
            meta["io.modelcontextprotocol/clientInfo"]["name"],
            "neoism-agent"
        );
        assert_eq!(
            meta["io.modelcontextprotocol/clientCapabilities"]["extensions"],
            json!({ EVENTS: {} })
        );
        match (method, request["id"].as_u64().unwrap()) {
            ("server/discover", 1) => (
                [("mcp-session-id", "unexpected-session")],
                Json(discover(
                    &request,
                    &["2025-11-25", REVISION],
                    json!({ EVENTS: {"listChanged": true} }),
                )),
            )
                .into_response(),
            ("events/list", 2) => {
                assert!(request["params"].get("cursor").is_none());
                Json(result(
                    &request,
                    json!({"events": [{
                    "name": "issue.updated", "delivery": ["push"],
                    "inputSchema": {}, "payloadSchema": {"type": "object"}
                }], "nextCursor": "opaque-page"}),
                ))
                .into_response()
            }
            ("events/list", 3) => {
                assert_eq!(request["params"]["cursor"], "opaque-page");
                let body = format!(
                    ": fixture\r\n\r\ndata: {}\r\n\r\n",
                    result(&request, json!({"resultType": "complete", "events": []}))
                );
                ([(CONTENT_TYPE, "text/event-stream; charset=utf-8")], body)
                    .into_response()
            }
            _ => panic!("unexpected method or request ID"),
        }
    })
    .await;
    let headers = BTreeMap::from([("mcp-session-id".into(), "legacy-session".into())]);
    let client = HttpMcpEvents::connect(
        &fixture.url,
        Some(&headers),
        Some("fixture-token".into()),
        TIMEOUT,
    )
    .await
    .unwrap()
    .expect("advertised events");
    assert!(client.capability.list_changed);
    let first = client.list_page(None).await.unwrap();
    assert_eq!(first.events[0].name, "issue.updated");
    assert!(first.events[0].supports_push());
    let second = client
        .list_page(first.next_cursor.as_deref())
        .await
        .unwrap();
    assert!(second.events.is_empty() && second.next_cursor.is_none());
}

#[tokio::test]
async fn events_opt_in_requires_revision_and_capability_and_leaves_legacy_unchanged() {
    let fixture = fixture(
        |headers, request| match request["method"].as_str().unwrap() {
            "server/discover" => {
                let (versions, extensions) =
                    if headers.contains_key("fixture-unsupported") {
                        (vec!["2025-11-25"], json!({ EVENTS: {} }))
                    } else {
                        (vec![REVISION], json!({"unrelated": {}}))
                    };
                Json(discover(&request, &versions, extensions)).into_response()
            }
            "initialize" => {
                assert!(!headers.contains_key("mcp-protocol-version"));
                assert!(!headers.contains_key("mcp-method"));
                assert!(request["params"].get("_meta").is_none());
                assert_eq!(request["params"]["protocolVersion"], "2024-11-05");
                Json(result(
                    &request,
                    json!({"protocolVersion": "2024-11-05", "capabilities": {}}),
                ))
                .into_response()
            }
            _ => panic!("did not expect list or side-channel requests"),
        },
    )
    .await;
    let absence = HttpMcpEvents::connect(&fixture.url, None, None, TIMEOUT)
        .await
        .unwrap();
    assert!(absence.is_none());
    let options = BTreeMap::from([("fixture-unsupported".into(), "true".into())]);
    let error = HttpMcpEvents::connect(&fixture.url, Some(&options), None, TIMEOUT)
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref(),
        Some(&mcp_events::InvalidWire::UnsupportedRevision)
    );
    let legacy = HttpJsonRpcClient::new(&fixture.url, None, None, TIMEOUT, None).unwrap();
    let initialized = legacy
        .request("initialize", json!({"protocolVersion": "2024-11-05"}))
        .await
        .unwrap();
    assert_eq!(initialized["protocolVersion"], "2024-11-05");
}

#[tokio::test]
async fn events_list_rejects_mismatched_ids_malformed_results_and_oversize_chunks() {
    let fixture = fixture(|_, request| match request["method"].as_str().unwrap() {
        "server/discover" => {
            Json(discover(&request, &[REVISION], json!({ EVENTS: {} }))).into_response()
        }
        "events/list" => match request["params"]["cursor"].as_str() {
            Some("wrong-id") => {
                Json(json!({"jsonrpc": "2.0", "id": 999, "result": {"events": []}}))
                    .into_response()
            }
            Some("bad-page") => {
                Json(result(&request, json!({"events": [42]}))).into_response()
            }
            Some("bad-type") => Json(result(
                &request,
                json!({"resultType": "input_required", "events": []}),
            ))
            .into_response(),
            Some("chunked") => {
                let mut response = Body::from_stream(tokio_stream::iter([
                    Ok::<_, std::convert::Infallible>(Bytes::from(
                        vec![b' '; mcp_events::MAX_JSON_BYTES],
                    )),
                    Ok(Bytes::from_static(b"!")),
                ]))
                .into_response();
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                response
            }
            _ => panic!("unexpected cursor"),
        },
        _ => panic!("unexpected method"),
    })
    .await;
    let client = HttpMcpEvents::connect(&fixture.url, None, None, TIMEOUT)
        .await
        .unwrap()
        .unwrap();
    for (cursor, expected) in [
        ("wrong-id", mcp_events::InvalidWire::MismatchedResponse),
        ("bad-page", mcp_events::InvalidWire::InvalidJson),
        ("bad-type", mcp_events::InvalidWire::InvalidJson),
        ("chunked", mcp_events::InvalidWire::TooLarge),
    ] {
        let error = client.list_page(Some(cursor)).await.unwrap_err();
        assert_eq!(error.downcast_ref(), Some(&expected), "{cursor}: {error:#}");
    }
}
