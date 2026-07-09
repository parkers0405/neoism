//! Mapping between the shared agent-pane outbound queue and the daemon
//! WebSocket protocol.
//!
//! The pane records host-agnostic [`OutboundAgentCommand`] values. Web and
//! desktop remotes should not each invent their own translation table; this
//! module is the shared contract for the daemon-backed agent runtime.

use neoism_protocol::agent::{AgentClientMessage, PermissionDecision};
use serde_json::Value;

use crate::panels::agent_pane::outbound::OutboundAgentCommand;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProtocolMappingContext {
    pub active_session_id: Option<String>,
    pub default_directory: Option<String>,
    pub default_agent: Option<String>,
    pub default_model: Option<String>,
    pub default_thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentProtocolMapping {
    Messages(Vec<AgentClientMessage>),
    PendingPrompt(PendingAgentProtocolPrompt),
    EnsureSession,
    Unsupported(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAgentProtocolPrompt {
    pub text: String,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
}

pub fn map_outbound_command(
    command: OutboundAgentCommand,
    context: &AgentProtocolMappingContext,
) -> AgentProtocolMapping {
    use AgentClientMessage as Msg;
    use AgentProtocolMapping as Mapping;
    use OutboundAgentCommand as Cmd;

    match command {
        Cmd::EnsureSession => Mapping::EnsureSession,
        Cmd::SendPrompt {
            text,
            parts: _,
            system: _,
            agent,
            model,
            thinking,
            transcript_echo: _,
        } => {
            let mode = agent.or_else(|| context.default_agent.clone());
            let model = non_empty(model).or_else(|| context.default_model.clone());
            let thinking = thinking.or_else(|| context.default_thinking.clone());
            match context.active_session_id.clone() {
                Some(session_id) => Mapping::Messages(vec![Msg::SubmitPrompt {
                    session_id,
                    text,
                    attachments: Vec::new(),
                    mode,
                    model,
                    thinking,
                }]),
                None => Mapping::PendingPrompt(PendingAgentProtocolPrompt {
                    text,
                    mode,
                    model,
                    thinking,
                }),
            }
        }
        Cmd::SwitchSession { session_id } => Mapping::Messages(vec![
            Msg::SwitchThread {
                session_id: session_id.clone(),
            },
            Msg::GetHistory {
                session_id: session_id.clone(),
                cursor: None,
                limit: Some(200),
            },
            Msg::ResumeStream { session_id },
        ]),
        Cmd::AbortSession => context
            .active_session_id
            .clone()
            .map(|session_id| vec![Msg::CancelInflight { session_id }])
            .map(Mapping::Messages)
            .unwrap_or_else(|| Mapping::Messages(vec![Msg::Cancel])),
        Cmd::CompactSession => context
            .active_session_id
            .clone()
            .map(|session_id| vec![Msg::Compact { session_id }])
            .map(Mapping::Messages)
            .unwrap_or(Mapping::Unsupported("compact requires an active session")),
        Cmd::UndoSession => context
            .active_session_id
            .clone()
            .map(|session_id| vec![Msg::UndoSession { session_id }])
            .map(Mapping::Messages)
            .unwrap_or(Mapping::Unsupported("undo requires an active session")),
        Cmd::RedoSession => context
            .active_session_id
            .clone()
            .map(|session_id| vec![Msg::RedoSession { session_id }])
            .map(Mapping::Messages)
            .unwrap_or(Mapping::Unsupported("redo requires an active session")),
        Cmd::ApplyConfigDefaults => Mapping::Messages(vec![Msg::GetConfigDefaults {
            directory: context.default_directory.clone(),
        }]),
        Cmd::RefreshModelContextLimit | Cmd::RefreshModels => {
            Mapping::Messages(vec![Msg::ListProviders])
        }
        Cmd::RefreshSessions { directory } => Mapping::Messages(vec![Msg::ListThreads {
            directory: directory.or_else(|| context.default_directory.clone()),
            limit: Some(50),
        }]),
        Cmd::LoadOlderTimeline {
            session_id,
            before,
            limit,
        } => Mapping::Messages(vec![Msg::GetHistory {
            session_id,
            cursor: before,
            limit: Some(limit.min(u32::MAX as usize) as u32),
        }]),
        Cmd::RefreshAgents { directory } => Mapping::Messages(vec![Msg::ListAgents {
            directory: directory.or_else(|| context.default_directory.clone()),
        }]),
        Cmd::RefreshSkills { directory } | Cmd::ShowSkills { directory } => {
            Mapping::Messages(vec![Msg::ListSkills {
                directory: directory.or_else(|| context.default_directory.clone()),
            }])
        }
        Cmd::ReplyPermission { id, reply } => context
            .active_session_id
            .clone()
            .map(|session_id| permission_reply_messages(session_id, id, reply))
            .map(Mapping::Messages)
            .unwrap_or(Mapping::Unsupported(
                "permission reply requires an active session",
            )),
        Cmd::ApplyAgent { session_id, agent } => {
            Mapping::Messages(vec![Msg::SetAgent { session_id, agent }])
        }
        Cmd::ApplyModel { session_id, model } => {
            let model = model_value_to_wire_model(&model);
            Mapping::Messages(vec![Msg::SetModel {
                session_id,
                model,
                thinking: context.default_thinking.clone(),
            }])
        }
        Cmd::ApplyThinking {
            session_id,
            model,
            thinking,
        } => Mapping::Messages(vec![Msg::SetModel {
            session_id,
            model,
            thinking,
        }]),
        Cmd::SlashCommand { name, args } => context
            .active_session_id
            .clone()
            .map(|session_id| {
                let text = if args.trim().is_empty() {
                    format!("/{name}")
                } else {
                    format!("/{name} {}", args.trim())
                };
                vec![Msg::SlashCommand { session_id, text }]
            })
            .map(Mapping::Messages)
            .unwrap_or(Mapping::Unsupported(
                "slash command requires an active session",
            )),
        Cmd::ShowMcp { directory } => Mapping::Messages(vec![Msg::ShowMcp {
            directory: directory.or_else(|| context.default_directory.clone()),
        }]),
        Cmd::ShowPermissions { session_id } => {
            Mapping::Messages(vec![Msg::ShowPermissions { session_id }])
        }
        Cmd::ShowQuestions { session_id } => {
            Mapping::Messages(vec![Msg::ShowQuestions { session_id }])
        }
        Cmd::HandleQueue { session_id, action } => {
            Mapping::Messages(vec![Msg::HandleQueue { session_id, action }])
        }
        Cmd::HandlePermit {
            session_id,
            reply,
            id,
        } => Mapping::Messages(vec![Msg::HandlePermit {
            session_id,
            reply,
            request_id: id,
        }]),
        Cmd::HandleAnswer { session_id, answer } => {
            Mapping::Messages(vec![Msg::HandleAnswer { session_id, answer }])
        }
        Cmd::HandleReject { session_id, id } => {
            Mapping::Messages(vec![Msg::HandleReject {
                session_id,
                request_id: id,
            }])
        }
        Cmd::SetTitle { session_id, title } => {
            Mapping::Messages(vec![Msg::SetTitle { session_id, title }])
        }
        // The `/connect` provider-auth flow maps onto the daemon's
        // provider-auth WebSocket variants, which the daemon proxies to the
        // agent-server's HTTP surface (`GET /provider`, `GET /provider/auth`,
        // `PUT`/`DELETE /auth/:id`, `POST /provider/:id/oauth/…`). Replies
        // flow back through the pane's `apply_connect_catalog` /
        // `apply_connect_oauth_url` / `note_connect_finished` /
        // `note_connect_failed` setters.
        Cmd::RefreshConnectProviders { directory } => {
            Mapping::Messages(vec![Msg::ConnectListProviders {
                directory: directory.or_else(|| context.default_directory.clone()),
            }])
        }
        Cmd::ConnectStoreApiKey { provider_id, key } => {
            Mapping::Messages(vec![Msg::ConnectStoreApiKey { provider_id, key }])
        }
        Cmd::ConnectDisconnect { provider_id } => {
            Mapping::Messages(vec![Msg::ConnectDisconnect { provider_id }])
        }
        Cmd::ConnectOauthAuthorize {
            provider_id,
            method_index,
        } => Mapping::Messages(vec![Msg::ConnectOauthAuthorize {
            provider_id,
            method_index,
        }]),
        Cmd::ConnectOauthCallback {
            provider_id,
            method_index,
            code,
        } => Mapping::Messages(vec![Msg::ConnectOauthCallback {
            provider_id,
            method_index,
            code,
        }]),
    }
}

pub fn followup_after_thread_created(session_id: String) -> Vec<AgentClientMessage> {
    vec![
        AgentClientMessage::GetHistory {
            session_id: session_id.clone(),
            cursor: None,
            limit: Some(200),
        },
        AgentClientMessage::ResumeStream { session_id },
    ]
}

fn permission_reply_messages(
    session_id: String,
    id: String,
    reply: String,
) -> Vec<AgentClientMessage> {
    match reply.as_str() {
        "reject" | "no" | "deny" => vec![AgentClientMessage::DenyTool {
            request_id: id,
            session_id,
        }],
        "always" => vec![AgentClientMessage::ApproveTool {
            request_id: id,
            session_id,
            decision: PermissionDecision::Always,
        }],
        _ => vec![AgentClientMessage::ApproveTool {
            request_id: id,
            session_id,
            decision: PermissionDecision::Yes,
        }],
    }
}

fn model_value_to_wire_model(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            let provider = value.get("providerId")?.as_str()?;
            let model = value.get("modelId")?.as_str()?;
            Some(format!("{provider}/{model}"))
        })
        .unwrap_or_default()
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_session_requests_session_history_and_stream() {
        let mapped = map_outbound_command(
            OutboundAgentCommand::SwitchSession {
                session_id: "s1".to_string(),
            },
            &AgentProtocolMappingContext::default(),
        );
        let AgentProtocolMapping::Messages(messages) = mapped else {
            panic!("expected messages");
        };
        assert!(matches!(
            messages[0],
            AgentClientMessage::SwitchThread { .. }
        ));
        assert!(matches!(messages[1], AgentClientMessage::GetHistory { .. }));
        assert!(matches!(
            messages[2],
            AgentClientMessage::ResumeStream { .. }
        ));
    }

    #[test]
    fn send_prompt_without_session_becomes_pending_prompt() {
        let mapped = map_outbound_command(
            OutboundAgentCommand::SendPrompt {
                text: "hello".to_string(),
                parts: Vec::new(),
                system: None,
                agent: None,
                model: String::new(),
                thinking: None,
                transcript_echo: true,
            },
            &AgentProtocolMappingContext::default(),
        );
        assert!(matches!(mapped, AgentProtocolMapping::PendingPrompt(_)));
    }

    #[test]
    fn undo_redo_map_to_session_protocol_messages() {
        let context = AgentProtocolMappingContext {
            active_session_id: Some("s1".to_string()),
            ..AgentProtocolMappingContext::default()
        };

        let undo = map_outbound_command(OutboundAgentCommand::UndoSession, &context);
        let AgentProtocolMapping::Messages(messages) = undo else {
            panic!("expected undo message");
        };
        assert!(matches!(
            &messages[0],
            AgentClientMessage::UndoSession { session_id } if session_id == "s1"
        ));

        let redo = map_outbound_command(OutboundAgentCommand::RedoSession, &context);
        let AgentProtocolMapping::Messages(messages) = redo else {
            panic!("expected redo message");
        };
        assert!(matches!(
            &messages[0],
            AgentClientMessage::RedoSession { session_id } if session_id == "s1"
        ));
    }

    #[test]
    fn undo_redo_require_active_session() {
        assert!(matches!(
            map_outbound_command(
                OutboundAgentCommand::UndoSession,
                &AgentProtocolMappingContext::default()
            ),
            AgentProtocolMapping::Unsupported(_)
        ));
        assert!(matches!(
            map_outbound_command(
                OutboundAgentCommand::RedoSession,
                &AgentProtocolMappingContext::default()
            ),
            AgentProtocolMapping::Unsupported(_)
        ));
    }
}
