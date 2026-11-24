use super::*;
use serde_json::json;

const ID: &str = "io.modelcontextprotocol/subscriptionId";
const ACTIVE: &str = "notifications/events/active";
const EVENT: &str = "notifications/events/event";
const HEARTBEAT: &str = "notifications/events/heartbeat";
const ERROR: &str = "notifications/events/error";
const TERMINATED: &str = "notifications/events/terminated";

fn notification(method: &str, params: Value) -> Result<PushNotification, InvalidWire> {
    PushNotification::decode(method, &serde_json::to_vec(&params).unwrap())
}

fn event_type() -> Value {
    json!({
        "name": "issue.updated", "description": "Issue changes", "delivery": ["push", "future"],
        "inputSchema": {"type": "object"}, "payloadSchema": {"type": "object"}, "_meta": {}
    })
}

#[test]
fn capability_requires_the_namespaced_object() {
    assert_eq!(PROTOCOL_REVISION, "2026-07-28");
    assert_eq!(capability(br#"{"tools":{}}"#).unwrap(), None);
    assert_eq!(
        capability(br#"{"extensions":{"events":{}}}"#).unwrap(),
        None
    );
    for (settings, expected) in [(json!({}), false), (json!({"listChanged": true}), true)]
    {
        let bytes =
            serde_json::to_vec(&json!({"extensions": {EXTENSION: settings}})).unwrap();
        assert_eq!(
            capability(&bytes),
            Ok(Some(EventCapability {
                list_changed: expected
            }))
        );
    }
    for settings in [
        json!(null),
        json!(true),
        json!([]),
        json!({"listChanged": "true"}),
    ] {
        let bytes =
            serde_json::to_vec(&json!({"extensions": {EXTENSION: settings}})).unwrap();
        assert_eq!(capability(&bytes), Err(InvalidWire::InvalidJson));
    }
    assert_eq!(capability(b"[]"), Err(InvalidWire::InvalidJson));
}

#[test]
fn list_and_stream_use_the_client_wire_shape() {
    assert_eq!(LIST_METHOD, "events/list");
    assert_eq!(STREAM_METHOD, "events/stream");
    assert_eq!(
        serde_json::to_value(ListParams::new(None).unwrap()).unwrap(),
        json!({})
    );
    let page = ListPage::decode(
        &serde_json::to_vec(&json!({
            "events": [event_type()], "nextCursor": "page-2", "_meta": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let after = page.next_cursor.as_deref();
    assert_eq!(
        serde_json::to_value(ListParams::new(after).unwrap()).unwrap(),
        json!({"cursor": "page-2"})
    );
    let event = &page.events[0];
    assert_eq!(event.delivery, ["push", "future"]);
    assert_eq!(event.input_schema["type"], "object");
    assert_eq!(event.payload_schema["type"], "object");
    let args = json!({"project": "infra"});
    let stream =
        StreamParams::new(event, args.as_object(), Some("replay"), Some(3_600_000))
            .unwrap();
    assert_eq!(
        serde_json::to_value(stream).unwrap(),
        json!({
            "name": "issue.updated", "arguments": {"project": "infra"}, "cursor": "replay", "maxAgeMs": 3_600_000
        })
    );
    let stream = StreamParams::new(event, None, None, None).unwrap();
    assert_eq!(
        serde_json::to_value(stream).unwrap(),
        json!({"name": "issue.updated"})
    );
    assert!(StreamParams::new(event, None, None, Some(MAX_SAFE_INTEGER + 1)).is_err());
    let mut poll_only = event_type();
    poll_only["delivery"] = json!(["poll"]);
    let page =
        ListPage::decode(&serde_json::to_vec(&json!({"events": [poll_only]})).unwrap())
            .unwrap();
    assert!(matches!(
        StreamParams::new(&page.events[0], None, None, None),
        Err(InvalidWire::InvalidField("delivery"))
    ));
}

#[test]
fn list_rejects_malformed_entries_and_bounds_the_page() {
    for bytes in [
        br#"{"events":[],"nextCursor":null}"#.as_slice(),
        br#"{"events":[["name","description",["push"],{},{}]]}"#,
        br#"{"events":[{"name":"x","delivery":["push"],"inputSchema":[],"payloadSchema":{}}]}"#,
        br#"{"events":[{"name":"x","delivery":[],"inputSchema":{},"payloadSchema":{}}]}"#,
        br#"{"events":[],"events":[]}"#,
    ] {
        assert!(ListPage::decode(bytes).is_err());
    }
    let mut page = json!({"events": vec![event_type(); MAX_EVENTS_PER_PAGE]});
    assert!(ListPage::decode(&serde_json::to_vec(&page).unwrap()).is_ok());
    page["events"].as_array_mut().unwrap().push(event_type());
    assert!(matches!(
        ListPage::decode(&serde_json::to_vec(&page).unwrap()),
        Err(InvalidWire::InvalidField("events"))
    ));
    let mut page = br#"{"events":[]}"#.to_vec();
    page.resize(MAX_JSON_BYTES, b' ');
    assert!(ListPage::decode(&page).is_ok());
    page.push(b' ');
    assert_eq!(ListPage::decode(&page).unwrap_err(), InvalidWire::TooLarge);
    let cursor = "é".repeat(MAX_TOKEN_BYTES / 2);
    assert!(ListParams::new(Some(&cursor)).is_ok());
    assert!(ListParams::new(Some(&(cursor + "a"))).is_err());
}

#[test]
fn decodes_all_five_push_notifications_and_preserves_rpc_id_type() {
    let active = notification(
        ACTIVE,
        json!({"_meta": {ID: 7}, "heartbeatMs": 1000, "truncated": true}),
    )
    .unwrap();
    assert_eq!(active.subscription_id, SubscriptionId::Integer(7));
    assert!(matches!(
        active.message,
        PushMessage::Active(Active {
            heartbeat_ms: 1000,
            cursor: None,
            truncated: true
        })
    ));
    let event = notification(EVENT, json!({
        "_meta": {ID: 7, "futureKey": true}, "eventId": "e-1", "name": "issue.updated",
        "timestamp": "2026-07-28T14:05+01:00", "data": {"values": [null, false]}, "cursor": "after-e-1"
    })).unwrap();
    let PushMessage::Event(event) = event.message else {
        panic!("expected event")
    };
    assert_eq!(event.event_id, "e-1");
    assert_eq!(event.timestamp, "2026-07-28T14:05+01:00");
    assert_eq!(event.cursor.as_deref(), Some("after-e-1"));
    assert_eq!(event.data["values"], json!([null, false]));
    let heartbeat =
        notification(HEARTBEAT, json!({"_meta": {ID: "7"}, "cursor": null})).unwrap();
    assert_eq!(
        heartbeat.subscription_id,
        SubscriptionId::String("7".into())
    );
    assert_ne!(heartbeat.subscription_id, active.subscription_id);
    assert!(matches!(
        heartbeat.message,
        PushMessage::Heartbeat(Heartbeat { cursor: None })
    ));
    let failure = json!({"_meta": {ID: 7}, "error": {"code": -32024, "message": "denied", "data": {"reason": "access"}}});
    assert!(matches!(
        notification(ERROR, failure.clone()).unwrap().message,
        PushMessage::Error(EventError { code: -32024, .. })
    ));
    assert!(matches!(
        notification(TERMINATED, failure).unwrap().message,
        PushMessage::Terminated(EventError { code: -32024, .. })
    ));
}

#[test]
fn push_decoder_rejects_bad_correlation_types_payloads_and_other_methods() {
    for method in [
        LIST_CHANGED_METHOD,
        "notifications/message",
        "experimental.mcp.event",
    ] {
        assert!(matches!(
            notification(method, json!({})),
            Err(InvalidWire::UnsupportedMethod)
        ));
    }
    for bad in [
        json!(1.5),
        json!(null),
        json!(true),
        json!(MAX_SAFE_INTEGER + 1),
    ] {
        assert!(notification(HEARTBEAT, json!({"_meta": {ID: bad}})).is_err());
    }
    assert!(notification(HEARTBEAT, json!({"subscriptionId": 1})).is_err());
    assert!(notification(HEARTBEAT, json!({"_meta": [1]})).is_err());
    assert!(notification(
        HEARTBEAT,
        json!({"_meta": {ID: 1}, "cursor": "x".repeat(MAX_TOKEN_BYTES + 1)})
    )
    .is_err());
    assert!(notification(ACTIVE, json!({"_meta": {ID: 1}, "heartbeatMs": 0})).is_err());
    assert!(notification(
        ACTIVE,
        json!({"_meta": {ID: 1}, "heartbeatMs": 1000, "truncated": null})
    )
    .is_err());
    assert!(
        notification(ERROR, json!({"_meta": {ID: 1}, "error": [-1, "bad"]})).is_err()
    );
    assert!(notification(
        ERROR,
        json!({"_meta": {ID: 1}, "error": {"code": -1, "message": "bad", "data": []}})
    )
    .is_err());
    let mut event = json!({"_meta": {ID: 1}, "eventId": "e-1", "name": "issue.updated", "timestamp": "2026-02-29T12:00:00Z", "data": {}});
    assert!(notification(EVENT, event.clone()).is_err());
    event["timestamp"] = json!("2024-02-29T12:00:00.123Z");
    assert!(notification(EVENT, event.clone()).is_ok());
    event["data"] = json!([1]);
    assert!(notification(EVENT, event).is_err());
    assert!(matches!(
        PushNotification::decode(EVENT, &vec![b' '; MAX_JSON_BYTES + 1]),
        Err(InvalidWire::TooLarge)
    ));
}
