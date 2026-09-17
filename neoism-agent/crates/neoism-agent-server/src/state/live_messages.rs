use std::collections::BTreeMap;

use neoism_agent_core::{event_type, EventPayload};
use serde_json::{json, Value};

/// Unfinished messages and active execution families in broadcast order.
/// Completed transcript bodies belong to durable history, not this projection.
#[derive(Default)]
pub(super) struct LiveMessages {
    messages: BTreeMap<String, (Value, BTreeMap<String, Value>)>,
    runtimes: BTreeMap<String, Value>,
    runtime_revisions: BTreeMap<String, (String, u64, u64)>,
}

impl LiveMessages {
    pub(super) fn observe(&mut self, event: &EventPayload) {
        let p = &event.properties;
        match event.kind.as_str() {
            event_type::SESSION_EXECUTION_UPDATED => {
                let Some(session) = p["sessionID"].as_str() else {
                    return;
                };
                let runtime = &p["runtime"];
                let Some(family_revision) = runtime["familyRevision"]
                    .as_u64()
                    .or_else(|| runtime["revision"].as_u64())
                else {
                    return;
                };
                let revision = (
                    runtime["execution"]["executionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    runtime["execution"]["revision"]
                        .as_u64()
                        .unwrap_or_default(),
                    family_revision,
                );
                if self
                    .runtime_revisions
                    .get(session)
                    .is_some_and(|seen| seen > &revision)
                {
                    return;
                }
                // Keep only the revision after completion so a delayed active
                // snapshot cannot resurrect a settled family for a new viewer.
                self.runtime_revisions.insert(session.to_owned(), revision);
                let working = runtime["execution"]["finished"] == false
                    || runtime["branches"].as_array().is_some_and(|branches| {
                        branches
                            .iter()
                            .any(|branch| branch["status"] == "outstanding")
                    });
                if working {
                    self.runtimes.insert(session.to_owned(), p.clone());
                } else {
                    self.runtimes.remove(session);
                }
            }
            event_type::MESSAGE_UPDATED => {
                let info = &p["info"];
                let Some(id) = info["id"].as_str() else {
                    return;
                };
                if info["role"] != "assistant" {
                    return;
                }
                if info["time"]["completed"].is_number() {
                    self.messages.remove(id);
                } else {
                    self.messages.entry(id.to_owned()).or_default().0 = info.clone();
                }
            }
            event_type::MESSAGE_PART_UPDATED => {
                let part = &p["part"];
                let Some(message) = part["messageID"]
                    .as_str()
                    .or_else(|| part["messageId"].as_str())
                else {
                    return;
                };
                let Some((_, parts)) = self.messages.get_mut(message) else {
                    return;
                };
                if let Some(id) = part["id"].as_str() {
                    // Retrying reuses the message ID but seeds a new step-start.
                    // Its old reasoning/tool fragments are no longer live.
                    if part["type"] == "step-start" {
                        parts.clear();
                    }
                    parts.insert(id.to_owned(), part.clone());
                }
            }
            event_type::MESSAGE_PART_DELTA => {
                let Some(message) = p["messageID"].as_str() else {
                    return;
                };
                let Some((_, parts)) = self.messages.get_mut(message) else {
                    return;
                };
                let (Some(id), Some(delta)) = (p["partID"].as_str(), p["delta"].as_str())
                else {
                    return;
                };
                if p["field"] != "text" {
                    return;
                }
                if let Some(Value::String(text)) =
                    parts.get_mut(id).and_then(|part| part.get_mut("text"))
                {
                    text.push_str(delta);
                }
            }
            event_type::MESSAGE_PART_REMOVED => {
                if let (Some(message), Some(part)) =
                    (p["messageID"].as_str(), p["partID"].as_str())
                {
                    if let Some((_, parts)) = self.messages.get_mut(message) {
                        parts.remove(part);
                    }
                }
            }
            event_type::MESSAGE_REMOVED => {
                if let Some(message) = p["messageID"].as_str() {
                    self.messages.remove(message);
                }
            }
            event_type::SESSION_DELETED => {
                let session = p["sessionID"].as_str();
                self.messages
                    .retain(|_, (info, _)| session != session_id(info));
                if let Some(session) = session {
                    self.runtimes.remove(session);
                    self.runtime_revisions.remove(session);
                }
            }
            _ => {}
        }
    }

    pub(super) fn snapshot(&self) -> Vec<EventPayload> {
        let mut events: Vec<_> = self
            .runtimes
            .values()
            .map(|properties| {
                EventPayload::new(
                    event_type::SESSION_EXECUTION_UPDATED,
                    properties.clone(),
                )
            })
            .collect();
        for (info, parts) in self.messages.values() {
            let Some(session) = session_id(info) else {
                continue;
            };
            // Fresh IDs make a reconnect snapshot authoritative even when the
            // transport has already deduplicated the original part-start event.
            events.push(EventPayload::new(
                event_type::MESSAGE_UPDATED,
                json!({"sessionID": session, "info": info}),
            ));
            for part in parts.values() {
                events.push(EventPayload::new(event_type::MESSAGE_PART_UPDATED,
                    json!({"sessionID": session, "part": part, "time": crate::now_millis()})));
            }
        }
        events
    }
}

fn session_id(info: &Value) -> Option<&str> {
    info["sessionID"]
        .as_str()
        .or_else(|| info["sessionId"].as_str())
}
