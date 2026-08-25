use neoism_agent_core::EventPayload;
use serde_json::Value;

/// Resolve the durable aggregate owning an internally published event.
pub(crate) fn aggregate_id(payload: &EventPayload) -> String {
    payload
        .properties
        .get("aggregateID")
        .or_else(|| payload.properties.get("aggregateId"))
        .or_else(|| payload.properties.get("sessionID"))
        .or_else(|| payload.properties.get("sessionId"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| payload.id.to_string())
}