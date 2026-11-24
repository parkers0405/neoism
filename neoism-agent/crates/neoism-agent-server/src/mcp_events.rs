//! MCP Events push wire models, as used by OpenCode v2.4.0's `packages/core/src/mcp/client.ts`.
//! This is not the internal event bus model in `packages/schema/src/mcp-event.ts` or the
//! lifecycle and retry policy in `packages/core/src/mcp/event-stream.ts`.

use chrono::DateTime;
use serde::de::{value::MapAccessDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

#[path = "mcp_events_http_stream.rs"]
mod http_stream;
pub(crate) use http_stream::{EventStreamError, HttpMcpEventStream};

pub(crate) const EXTENSION: &str = "io.modelcontextprotocol/events";
pub(crate) const PROTOCOL_REVISION: &str = "2026-07-28";
pub(crate) const DISCOVER_METHOD: &str = "server/discover";
pub(crate) const LIST_METHOD: &str = "events/list";
pub(crate) const STREAM_METHOD: &str = "events/stream";
pub(crate) const LIST_CHANGED_METHOD: &str = "notifications/events/list_changed";

// Neoism limits, not extension requirements. Transport must separately bound frames before buffering.
pub(crate) const MAX_JSON_BYTES: usize = 256 * 1024;
const MAX_EVENTS_PER_PAGE: usize = 256;
const MAX_NAME_BYTES: usize = 512;
const MAX_TOKEN_BYTES: usize = 4096;
pub(crate) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
const DEFAULT_REPLAY_AGE_MS: u64 = 60 * 60 * 1000;

type Object = Map<String, Value>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InvalidWire {
    TooLarge,
    InvalidJson,
    InvalidField(&'static str),
    UnsupportedMethod,
    MismatchedResponse,
    Rpc(i64),
    UnsupportedRevision,
}

impl std::fmt::Display for InvalidWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("MCP Events JSON exceeds the byte limit"),
            Self::InvalidJson => f.write_str("invalid MCP Events JSON or field type"),
            Self::InvalidField(field) => write!(f, "invalid MCP Events field: {field}"),
            Self::UnsupportedMethod => {
                f.write_str("not an MCP Events push notification method")
            }
            Self::MismatchedResponse => {
                f.write_str("MCP Events response ID does not match")
            }
            Self::Rpc(code) => {
                write!(f, "MCP Events request failed with JSON-RPC code {code}")
            }
            Self::UnsupportedRevision => {
                f.write_str("server does not support the MCP Events protocol revision")
            }
        }
    }
}

impl std::error::Error for InvalidWire {}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, InvalidWire> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(InvalidWire::TooLarge);
    }
    // Don't include arbitrary server-provided values in errors.
    serde_json::from_slice::<ObjectWire<T>>(bytes)
        .map(|wire| wire.0)
        .map_err(|_| InvalidWire::InvalidJson)
}

// A derived serde struct can also accept a positional array. MCP objects have named fields.
struct ObjectWire<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ObjectWire<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = ObjectWire<T>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<Self::Value, M::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(ObjectWire)
            }
        }

        d.deserialize_map(ObjectVisitor(std::marker::PhantomData))
    }
}

// Unlike Option's default deserializer, an optional non-null field must be omitted, not null.
fn optional<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

fn bounded(field: &'static str, value: &str, limit: usize) -> Result<(), InvalidWire> {
    if value.len() > limit {
        return Err(InvalidWire::InvalidField(field));
    }
    Ok(())
}

fn name(value: &str) -> Result<(), InvalidWire> {
    if value.trim().is_empty() {
        return Err(InvalidWire::InvalidField("name"));
    }
    bounded("name", value, MAX_NAME_BYTES)
}

fn cursor(value: Option<&str>) -> Result<(), InvalidWire> {
    value.map_or(Ok(()), |value| bounded("cursor", value, MAX_TOKEN_BYTES))
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventCapability {
    #[serde(default)]
    pub(crate) list_changed: bool,
}

#[derive(Deserialize)]
struct Capabilities {
    #[serde(default, deserialize_with = "optional")]
    extensions: Option<ObjectWire<Extensions>>,
}

#[derive(Deserialize)]
struct Extensions {
    #[serde(
        rename = "io.modelcontextprotocol/events",
        default,
        deserialize_with = "optional"
    )]
    events: Option<ObjectWire<EventCapability>>,
}

impl Capabilities {
    fn events(self) -> Option<EventCapability> {
        self.extensions
            .and_then(|extensions| extensions.0.events)
            .map(|wire| wire.0)
    }
}

/// Decode the server capabilities object, not the whole discovery result. Presence alone
/// is not negotiation: callers must also opt in and check the negotiated protocol era.
pub(crate) fn capability(bytes: &[u8]) -> Result<Option<EventCapability>, InvalidWire> {
    Ok(decode::<Capabilities>(bytes)?.events())
}

/// The 2026 revision has no `initialize` exchange; this envelope goes on every request.
pub(crate) fn request_meta() -> Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_REVISION,
        "io.modelcontextprotocol/clientInfo": {"name": "neoism-agent", "version": env!("CARGO_PKG_VERSION")},
        "io.modelcontextprotocol/clientCapabilities": {"extensions": {EXTENSION: {}}}
    })
}

// For these read-only methods there is no input-required flow. Like the SDK, receive
// a missing resultType as `complete`, but do not interpret other result types as data.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Complete {
    #[default]
    Complete,
}

fn decode_response<T: serde::de::DeserializeOwned>(
    id: u64,
    bytes: &[u8],
) -> Result<T, InvalidWire> {
    #[derive(Deserialize)]
    struct RpcError {
        code: i64,
        #[serde(rename = "message")]
        _message: String,
    }

    #[derive(Deserialize)]
    #[serde(bound(deserialize = "T: Deserialize<'de>"))]
    struct Response<T> {
        jsonrpc: String,
        id: u64,
        #[serde(default, deserialize_with = "optional")]
        method: Option<Value>,
        #[serde(default, deserialize_with = "optional")]
        result: Option<ObjectWire<T>>,
        #[serde(default, deserialize_with = "optional")]
        error: Option<ObjectWire<RpcError>>,
    }

    let response: Response<T> = decode(bytes)?;
    if response.jsonrpc != "2.0" {
        return Err(InvalidWire::InvalidField("jsonrpc"));
    }
    if response.method.is_some() {
        return Err(InvalidWire::InvalidField("method"));
    }
    if response.id != id {
        return Err(InvalidWire::MismatchedResponse);
    }
    match (response.result, response.error) {
        (Some(result), None) => Ok(result.0),
        (None, Some(error)) => Err(InvalidWire::Rpc(error.0.code)),
        _ => Err(InvalidWire::InvalidJson),
    }
}

pub(crate) fn decode_discovery(
    id: u64,
    bytes: &[u8],
) -> Result<Option<EventCapability>, InvalidWire> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Discovery {
        #[serde(default, rename = "resultType")]
        _result_type: Complete,
        supported_versions: Vec<String>,
        capabilities: ObjectWire<Capabilities>,
    }

    let found: Discovery = decode_response(id, bytes)?;
    if !found
        .supported_versions
        .iter()
        .any(|version| version == PROTOCOL_REVISION)
    {
        return Err(InvalidWire::UnsupportedRevision);
    }
    Ok(found.capabilities.0.events())
}

#[derive(Debug, Serialize)]
pub(crate) struct ListParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
}

impl<'a> ListParams<'a> {
    pub(crate) fn new(after: Option<&'a str>) -> Result<Self, InvalidWire> {
        cursor(after)?;
        Ok(Self { cursor: after })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventType {
    pub(crate) name: String,
    #[serde(default, deserialize_with = "optional")]
    pub(crate) description: Option<String>,
    pub(crate) delivery: Vec<String>,
    pub(crate) input_schema: Object,
    pub(crate) payload_schema: Object,
}

impl EventType {
    pub(crate) fn supports_push(&self) -> bool {
        self.delivery.iter().any(|mode| mode == "push")
    }
}

#[derive(Debug)]
pub(crate) struct ListPage {
    pub(crate) events: Vec<EventType>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default, rename = "resultType")]
    _result_type: Complete,
    events: Vec<ObjectWire<EventType>>,
    #[serde(default, deserialize_with = "optional")]
    next_cursor: Option<String>,
}

impl ListPage {
    /// Fail the page on a malformed entry rather than silently using a partial workflow catalog.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, InvalidWire> {
        Self::validate(decode(bytes)?)
    }

    pub(crate) fn decode_response(id: u64, bytes: &[u8]) -> Result<Self, InvalidWire> {
        Self::validate(decode_response(id, bytes)?)
    }

    fn validate(page: Page) -> Result<Self, InvalidWire> {
        cursor(page.next_cursor.as_deref())?;
        if page.events.len() > MAX_EVENTS_PER_PAGE {
            return Err(InvalidWire::InvalidField("events"));
        }
        let mut events = Vec::with_capacity(page.events.len());
        for ObjectWire(event) in page.events {
            name(&event.name)?;
            if event.delivery.is_empty()
                || event.delivery.iter().any(|mode| mode.trim().is_empty())
            {
                return Err(InvalidWire::InvalidField("delivery"));
            }
            events.push(event);
        }
        Ok(Self {
            events,
            next_cursor: page.next_cursor,
        })
    }
}

/// Push is an in-flight request, not a separate subscribe/unsubscribe RPC pair. The
/// request's JSON-RPC ID is the correlation key; transport owns cancellation and lifetime.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StreamParams<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<&'a Object>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_age_ms: Option<u64>,
}

impl<'a> StreamParams<'a> {
    pub(crate) fn new(
        event: &'a EventType,
        arguments: Option<&'a Object>,
        after: Option<&'a str>,
        max_age_ms: Option<u64>,
    ) -> Result<Self, InvalidWire> {
        name(&event.name)?;
        cursor(after)?;
        if !event.supports_push() {
            return Err(InvalidWire::InvalidField("delivery"));
        }
        if max_age_ms.is_some_and(|age| age > MAX_SAFE_INTEGER) {
            return Err(InvalidWire::InvalidField("maxAgeMs"));
        }
        Ok(Self {
            name: &event.name,
            arguments,
            cursor: after,
            max_age_ms,
        })
    }

    pub(crate) fn for_http(
        event: &'a EventType,
        arguments: Option<&'a Object>,
        after: Option<&'a str>,
        max_age_ms: Option<u64>,
    ) -> Result<Self, InvalidWire> {
        fn has_header(value: &Value) -> bool {
            match value {
                Value::Object(map) => {
                    map.contains_key("x-mcp-header") || map.values().any(has_header)
                }
                Value::Array(items) => items.iter().any(has_header),
                _ => false,
            }
        }
        // Do not silently send a request missing the required argument mirrors.
        if event.input_schema.contains_key("x-mcp-header")
            || event.input_schema.values().any(has_header)
        {
            return Err(InvalidWire::InvalidField(
                "unsupported x-mcp-header in inputSchema",
            ));
        }
        let age = max_age_ms.or_else(|| after.map(|_| DEFAULT_REPLAY_AGE_MS));
        Self::new(event, arguments, after, age)
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub(crate) enum SubscriptionId {
    String(String),
    Integer(i64),
}

#[derive(Debug, Deserialize)]
struct SubscriptionMeta {
    #[serde(rename = "io.modelcontextprotocol/subscriptionId")]
    id: SubscriptionId,
}

#[derive(Deserialize)]
struct StreamMessage<T> {
    #[serde(rename = "_meta")]
    subscription: ObjectWire<SubscriptionMeta>,
    #[serde(flatten)]
    message: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Active {
    pub(crate) heartbeat_ms: u64,
    pub(crate) cursor: Option<String>,
    #[serde(default)]
    pub(crate) truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    pub(crate) event_id: String,
    pub(crate) name: String,
    pub(crate) timestamp: String,
    pub(crate) data: Object,
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Heartbeat {
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EventError {
    pub(crate) code: i64,
    pub(crate) message: String,
    #[serde(default, deserialize_with = "optional")]
    pub(crate) data: Option<Object>,
}

#[derive(Deserialize)]
struct Failure {
    error: ObjectWire<EventError>,
}

#[derive(Debug)]
pub(crate) enum PushMessage {
    Active(Active),
    Event(Event),
    Heartbeat(Heartbeat),
    Error(EventError),
    Terminated(EventError),
}

#[derive(Debug)]
pub(crate) struct PushNotification {
    pub(crate) subscription_id: SubscriptionId,
    pub(crate) message: PushMessage,
}

impl PushNotification {
    /// Accept only stream-scoped Events methods. The transport must already have checked
    /// the JSON-RPC envelope; list changes are not stream-scoped and don't enter this decoder.
    pub(crate) fn decode(method: &str, params: &[u8]) -> Result<Self, InvalidWire> {
        Self::decode_at(method, params, false)
    }

    fn decode_at(
        method: &str,
        bytes: &[u8],
        envelope: bool,
    ) -> Result<Self, InvalidWire> {
        match method {
            "notifications/events/active" => {
                Self::decode_message(bytes, envelope, |active: Active| {
                    cursor(active.cursor.as_deref())?;
                    if !(1..=MAX_SAFE_INTEGER).contains(&active.heartbeat_ms) {
                        return Err(InvalidWire::InvalidField("heartbeatMs"));
                    }
                    Ok(PushMessage::Active(active))
                })
            }
            "notifications/events/event" => {
                Self::decode_message(bytes, envelope, |event: Event| {
                    name(&event.name)?;
                    cursor(event.cursor.as_deref())?;
                    bounded("eventId", &event.event_id, MAX_TOKEN_BYTES)?;
                    if event.event_id.is_empty() {
                        return Err(InvalidWire::InvalidField("eventId"));
                    }
                    validate_timestamp(&event.timestamp)?;
                    Ok(PushMessage::Event(event))
                })
            }
            "notifications/events/heartbeat" => {
                Self::decode_message(bytes, envelope, |heartbeat: Heartbeat| {
                    cursor(heartbeat.cursor.as_deref())?;
                    Ok(PushMessage::Heartbeat(heartbeat))
                })
            }
            "notifications/events/error" | "notifications/events/terminated" => {
                Self::decode_message(bytes, envelope, |failure: Failure| {
                    let error = failure.error.0;
                    if error.code.unsigned_abs() > MAX_SAFE_INTEGER {
                        return Err(InvalidWire::InvalidField("error.code"));
                    }
                    Ok(if method == "notifications/events/error" {
                        PushMessage::Error(error)
                    } else {
                        PushMessage::Terminated(error)
                    })
                })
            }
            _ => Err(InvalidWire::UnsupportedMethod),
        }
    }

    fn decode_message<T: serde::de::DeserializeOwned>(
        bytes: &[u8],
        envelope: bool,
        validate: impl FnOnce(T) -> Result<PushMessage, InvalidWire>,
    ) -> Result<Self, InvalidWire> {
        #[derive(Deserialize)]
        struct Params<T> {
            params: ObjectWire<StreamMessage<T>>,
        }
        let decoded: StreamMessage<T> = if envelope {
            decode::<Params<T>>(bytes)?.params.0
        } else {
            decode(bytes)?
        };
        let subscription_id = decoded.subscription.0.id;
        match &subscription_id {
            SubscriptionId::String(id) => bounded("subscriptionId", id, MAX_TOKEN_BYTES)?,
            SubscriptionId::Integer(id) if id.unsigned_abs() > MAX_SAFE_INTEGER => {
                return Err(InvalidWire::InvalidField("subscriptionId"));
            }
            SubscriptionId::Integer(_) => {}
        }
        Ok(Self {
            subscription_id,
            message: validate(decoded.message)?,
        })
    }
}

pub(super) enum PushFrame {
    Notification(PushMessage),
    Completed,
    Unrelated,
}

pub(super) fn decode_push_frame(id: u64, bytes: &[u8]) -> Result<PushFrame, InvalidWire> {
    #[derive(Deserialize)]
    struct Envelope {
        jsonrpc: String,
        #[serde(default, deserialize_with = "optional")]
        method: Option<String>,
        #[serde(default, deserialize_with = "optional")]
        id: Option<Value>,
        #[serde(default, deserialize_with = "optional")]
        result: Option<serde::de::IgnoredAny>,
        #[serde(default, deserialize_with = "optional")]
        error: Option<serde::de::IgnoredAny>,
    }
    #[derive(Deserialize)]
    struct Completion {
        #[serde(default, rename = "resultType")]
        _result_type: Complete,
    }
    let envelope: Envelope = decode(bytes)?;
    if envelope.jsonrpc != "2.0" {
        return Err(InvalidWire::InvalidField("jsonrpc"));
    }
    if let Some(method) = envelope.method {
        if envelope.id.is_some() || envelope.result.is_some() || envelope.error.is_some()
        {
            return Err(InvalidWire::InvalidJson);
        }
        if !method.starts_with("notifications/events/") || method == LIST_CHANGED_METHOD {
            return Ok(PushFrame::Unrelated);
        }
        let notification = PushNotification::decode_at(&method, bytes, true)?;
        if !matches!(notification.subscription_id, SubscriptionId::Integer(found) if u64::try_from(found) == Ok(id))
        {
            return Err(InvalidWire::MismatchedResponse);
        }
        return Ok(PushFrame::Notification(notification.message));
    }
    let _: Completion = decode_response(id, bytes)?;
    Ok(PushFrame::Completed)
}

fn validate_timestamp(value: &str) -> Result<(), InvalidWire> {
    bounded("timestamp", value, 128)?;
    let invalid = InvalidWire::InvalidField("timestamp");
    // OpenCode accepts minute precision as well as timestamps with seconds. Validate
    // real calendar dates without changing the timestamp delivered to the caller.
    if value.as_bytes().get(10) != Some(&b'T') {
        return Err(invalid);
    }
    let normalized;
    let value = if matches!(value.as_bytes().get(16), Some(b'Z' | b'+' | b'-')) {
        normalized = format!("{}:00{}", &value[..16], &value[16..]);
        normalized.as_str()
    } else {
        value
    };
    DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| invalid)
}

#[cfg(test)]
#[path = "mcp_events_tests.rs"]
mod tests;
