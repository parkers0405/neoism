use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use neoism_agent_core::{
    event_type, AssistantMessage, AssistantPath, CompletedTime, CreatedTime,
    EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Part, PermissionAction,
    PermissionRequestInfo, PermissionRule, ProviderGenerationResponse, SessionInfo,
    TextPart, TimeInfo, TokenUsage, UserMessage, UserModel,
};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::external_acp::{
    workspace_path, AcpClient, AcpEvent, AcpRpcError, AcpServerConfig,
    AcpTerminalManager, PROMPT_TIMEOUT,
};
use crate::message_part_mutation::{
    append_text_delta, set_tool_completed, set_tool_error, set_tool_running,
};
use crate::provider_stream_message::{
    finish_provider_stream_success, finish_provider_stream_with_error,
    start_assistant_step, StartedAssistantStep,
};
use crate::session_actions::publish_background_subtask_finished;
use crate::session_run::{finish_session_run, start_session_run};
use crate::state::AppState;
use crate::{ask_permission_for_tool, now_millis, permission, slug, tool};

#[path = "external_agent/acp_run.rs"]
mod acp_run;
#[path = "external_agent/events.rs"]
mod events;
#[path = "external_agent/helpers.rs"]
mod helpers;
#[path = "external_agent/lifecycle.rs"]
mod lifecycle;
#[path = "external_agent/requests.rs"]
mod requests;
#[path = "external_agent/runtime.rs"]
mod runtime;

pub(crate) use acp_run::*;
pub(crate) use events::*;
pub(crate) use helpers::*;
pub(crate) use lifecycle::*;
pub(crate) use requests::*;
pub(crate) use runtime::*;

#[cfg(test)]
#[path = "external_agent/tests.rs"]
mod tests;
