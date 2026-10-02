use std::collections::{BTreeMap, BTreeSet};

use neoism_agent_core::{
    MessageInfo, MessageWithParts, Part, SessionInfo, SessionUndoCursor, SessionUndoNode,
    SessionUndoPart, SessionUndoRevert, SessionUndoSnapshotSummary, SessionUndoStatus,
    SessionUndoTree, ToolState,
};
use serde_json::Value;

use crate::error::ApiError;
use crate::snapshot;
use crate::{message_id_of, title_from_parts};

#[derive(Clone, Debug)]
pub(super) struct PersistedRevert {
    pub(super) message_id: String,
    pub(super) part_id: Option<String>,
    pub(super) time: Option<u64>,
    pub(super) messages: Vec<MessageWithParts>,
    pub(super) parts: Vec<Part>,
}

pub(super) fn build_session_undo_tree(
    info: &SessionInfo,
    current_messages: Vec<MessageWithParts>,
) -> Result<SessionUndoTree, ApiError> {
    let revert = decode_persisted_revert(info.extra.get("revert"))?;
    let mut messages = current_messages;

    if let Some(revert) = &revert {
        if !revert.parts.is_empty() {
            if let Some(message) = messages
                .iter_mut()
                .find(|message| message_id_of(message) == revert.message_id)
            {
                message.parts.extend(revert.parts.clone());
            }
        }
        messages.extend(revert.messages.clone());
    }

    let message_order = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message_id_of(message), index))
        .collect::<BTreeMap<_, _>>();
    let revert_index = revert
        .as_ref()
        .and_then(|revert| message_order.get(&revert.message_id).copied());
    let groups = group_undo_messages(messages);
    let mut previous_id = None;
    let mut nodes = Vec::new();

    for group in groups {
        let message_ids = group.iter().map(message_id_of).collect::<Vec<_>>();
        let message_id = undo_group_message_id(&group);
        let parts = group
            .iter()
            .flat_map(|message| {
                let message_id = message_id_of(message);
                message
                    .parts
                    .iter()
                    .map(move |part| undo_part_summary(&message_id, part))
            })
            .collect::<Vec<_>>();
        let status = undo_group_status(
            &message_ids,
            &message_order,
            revert.as_ref(),
            revert_index,
        );
        let snapshots = undo_snapshot_summary(&group, &[]);
        let title = undo_group_title(&group).unwrap_or_else(|| message_id.clone());
        let time = undo_group_time(&group);
        let node = SessionUndoNode {
            id: message_id.clone(),
            message_id: message_id.clone(),
            parent_id: previous_id.clone(),
            status,
            title,
            time,
            message_ids,
            parts,
            snapshots,
        };
        previous_id = Some(message_id);
        nodes.push(node);
    }

    let cursor = nodes
        .iter()
        .rev()
        .find(|node| node.status == SessionUndoStatus::Applied)
        .map(|node| SessionUndoCursor {
            message_id: node.message_id.clone(),
            part_id: None,
        });
    let revert = revert.map(|revert| {
        let snapshots = undo_snapshot_summary(&revert.messages, &revert.parts);
        SessionUndoRevert {
            message_id: revert.message_id,
            part_id: revert.part_id,
            time: revert.time,
            messages: revert.messages.len(),
            parts: revert.parts.len(),
            snapshots: snapshots.files + snapshots.refs.len(),
            files: snapshots.paths,
            snapshot_refs: snapshots.refs,
        }
    });

    Ok(SessionUndoTree {
        session_id: info.id.to_string(),
        cursor,
        revert,
        nodes,
    })
}

pub(super) fn decode_persisted_revert(
    value: Option<&Value>,
) -> Result<Option<PersistedRevert>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(message_id) = value
        .get("messageID")
        .or_else(|| value.get("messageId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let messages = value
        .get("messages")
        .cloned()
        .map(serde_json::from_value::<Vec<MessageWithParts>>)
        .transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?
        .unwrap_or_default();
    let parts = value
        .get("parts")
        .cloned()
        .map(serde_json::from_value::<Vec<Part>>)
        .transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?
        .unwrap_or_default();
    Ok(Some(PersistedRevert {
        message_id: message_id.to_string(),
        part_id: value
            .get("partID")
            .or_else(|| value.get("partId"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        time: value.get("time").and_then(Value::as_u64),
        messages,
        parts,
    }))
}

fn group_undo_messages(messages: Vec<MessageWithParts>) -> Vec<Vec<MessageWithParts>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for message in messages {
        if matches!(message.info, MessageInfo::User(_)) && !current.is_empty() {
            groups.push(current);
            current = Vec::new();
        }
        current.push(message);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn undo_group_message_id(group: &[MessageWithParts]) -> String {
    group
        .iter()
        .find(|message| matches!(message.info, MessageInfo::User(_)))
        .or_else(|| group.first())
        .map(message_id_of)
        .unwrap_or_default()
}

fn undo_group_title(group: &[MessageWithParts]) -> Option<String> {
    group
        .iter()
        .find(|message| matches!(message.info, MessageInfo::User(_)))
        .and_then(|message| title_from_parts(&message.parts))
        .or_else(|| {
            group
                .iter()
                .find_map(|message| title_from_parts(&message.parts))
        })
}

fn undo_group_time(group: &[MessageWithParts]) -> u64 {
    group
        .iter()
        .find(|message| matches!(message.info, MessageInfo::User(_)))
        .or_else(|| group.first())
        .map(message_created_at)
        .unwrap_or_default()
}

fn undo_group_status(
    message_ids: &[String],
    message_order: &BTreeMap<String, usize>,
    revert: Option<&PersistedRevert>,
    revert_index: Option<usize>,
) -> SessionUndoStatus {
    let Some(revert_index) = revert_index else {
        return SessionUndoStatus::Applied;
    };
    if revert
        .filter(|revert| revert.part_id.is_some())
        .is_some_and(|revert| message_ids.iter().any(|id| id == &revert.message_id))
    {
        return SessionUndoStatus::Partial;
    }
    let mut indices = message_ids
        .iter()
        .filter_map(|id| message_order.get(id).copied())
        .collect::<Vec<_>>();
    indices.sort_unstable();
    let Some(start) = indices.first().copied() else {
        return SessionUndoStatus::Applied;
    };
    let end = indices.last().copied().unwrap_or(start);
    if end < revert_index {
        SessionUndoStatus::Applied
    } else if start >= revert_index {
        SessionUndoStatus::Reverted
    } else {
        SessionUndoStatus::Partial
    }
}

fn undo_part_summary(message_id: &str, part: &Part) -> SessionUndoPart {
    let (part_id, kind, tool, title) = match part {
        Part::Text(part) => (part.id.to_string(), "text".to_string(), None, None),
        Part::Compaction(part) => (
            part.id.to_string(),
            "compaction".to_string(),
            None,
            Some(part.reason.clone()),
        ),
        Part::Agent(part) => (
            part.id.to_string(),
            "agent".to_string(),
            None,
            Some(part.name.clone()),
        ),
        Part::Subtask(part) => (
            part.id.to_string(),
            "subtask".to_string(),
            Some("task".to_string()),
            Some(part.description.clone()),
        ),
        Part::Reasoning(part) => {
            (part.id.to_string(), "reasoning".to_string(), None, None)
        }
        Part::Tool(part) => (
            part.id.to_string(),
            "tool".to_string(),
            Some(part.tool.clone()),
            match &part.state {
                ToolState::Completed { title, .. } => Some(title.clone()),
                ToolState::Pending { .. }
                | ToolState::Running { .. }
                | ToolState::Error { .. } => None,
            },
        ),
        Part::StepStart(part) => {
            (part.id.to_string(), "step-start".to_string(), None, None)
        }
        Part::StepFinish(part) => {
            (part.id.to_string(), "step-finish".to_string(), None, None)
        }
        Part::File(part) => (
            part.id.to_string(),
            "file".to_string(),
            None,
            part.filename.clone(),
        ),
    };
    SessionUndoPart {
        message_id: message_id.to_string(),
        part_id,
        kind,
        tool,
        title,
        snapshots: undo_snapshot_summary(&[], std::slice::from_ref(part)),
    }
}

fn undo_snapshot_summary(
    messages: &[MessageWithParts],
    parts: &[Part],
) -> SessionUndoSnapshotSummary {
    let mut paths = BTreeSet::new();
    let mut refs = BTreeSet::new();
    paths.extend(snapshot::snapshot_paths_from_revert_items(messages, parts));
    for message in messages {
        for part in &message.parts {
            collect_snapshot_ref(part, &mut refs);
        }
    }
    for part in parts {
        collect_snapshot_ref(part, &mut refs);
    }
    let paths = paths.into_iter().collect::<Vec<_>>();
    SessionUndoSnapshotSummary {
        files: paths.len(),
        paths,
        refs: refs.into_iter().collect(),
    }
}

fn collect_snapshot_ref(part: &Part, refs: &mut BTreeSet<String>) {
    match part {
        Part::StepStart(part) => {
            if let Some(snapshot) = &part.snapshot {
                refs.insert(snapshot.clone());
            }
        }
        Part::StepFinish(part) => {
            if let Some(snapshot) = &part.snapshot {
                refs.insert(snapshot.clone());
            }
        }
        Part::Text(_)
        | Part::Compaction(_)
        | Part::Agent(_)
        | Part::Subtask(_)
        | Part::Reasoning(_)
        | Part::Tool(_)
        | Part::File(_) => {}
    }
}

fn message_created_at(message: &MessageWithParts) -> u64 {
    match &message.info {
        MessageInfo::User(message) => message.time.created,
        MessageInfo::Assistant(message) => message.time.created,
    }
}
