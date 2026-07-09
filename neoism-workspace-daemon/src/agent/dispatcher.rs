use super::*;

/// Handle a single inbound `AgentClientMessage` envelope from the
/// WebSocket router. Routes layer-1 envelopes to the legacy direct
/// proxy and layer-2 envelopes to the embedded agent-server.
pub fn dispatch(session: &AgentSession, msg: AgentClientMessage) {
    // Make sure the embedded agent-server has been kicked off before
    // we issue the first envelope. Cheap on subsequent calls (one
    // atomic load).
    ensure_agent_server_started();
    let inner = session.inner.clone();
    match msg {
        // -- Layer 1: direct Claude proxy --------------------------
        AgentClientMessage::SendMessage { text, attachments } => {
            session.send_message(text, attachments);
        }
        AgentClientMessage::Cancel => session.cancel(),
        AgentClientMessage::NewThread => session.new_thread(),
        AgentClientMessage::ReplyPermission { .. } => {
            // Layer-1 permission gating remains unimplemented — the
            // chrome surfaces the request, but Anthropic's API
            // doesn't tunnel a tool-use loop back through the
            // streaming response, so there's no decision to forward.
            // Drop silently rather than half-implement.
        }

        // -- Layer 2: agent-server proxy ---------------------------
        AgentClientMessage::CreateThread {
            title,
            directory,
            agent,
            model,
        } => {
            tokio::spawn(async move {
                handle_create_thread(inner, title, directory, agent, model).await;
            });
        }
        AgentClientMessage::SwitchThread { session_id } => {
            tokio::spawn(async move {
                handle_switch_thread(inner, session_id).await;
            });
        }
        AgentClientMessage::DeleteThread { session_id } => {
            tokio::spawn(async move {
                handle_delete_thread(inner, session_id).await;
            });
        }
        AgentClientMessage::ListThreads { directory, limit } => {
            tokio::spawn(async move {
                handle_list_threads(inner, directory, limit).await;
            });
        }
        AgentClientMessage::GetHistory {
            session_id,
            cursor,
            limit,
        } => {
            tokio::spawn(async move {
                handle_get_history(inner, session_id, cursor, limit).await;
            });
        }
        AgentClientMessage::ResumeStream { session_id } => {
            start_event_stream(&inner, &session_id);
        }
        AgentClientMessage::StopStream { session_id } => {
            stop_event_stream(&inner, &session_id);
        }
        AgentClientMessage::SubmitPrompt {
            session_id,
            text,
            attachments: _,
            mode,
            model,
            thinking,
        } => {
            spawn_inflight(&inner, session_id.clone(), move |inner| {
                handle_submit_prompt(inner, session_id, text, mode, model, thinking)
            });
        }
        AgentClientMessage::CancelInflight { session_id } => {
            cancel_inflight(&inner, &session_id);
            let inner = inner.clone();
            tokio::spawn(async move {
                let _ =
                    post_no_body(&inner, &format!("/session/{session_id}/abort")).await;
            });
        }
        AgentClientMessage::EnqueuePrompt { session_id, text } => {
            tokio::spawn(async move {
                handle_enqueue_prompt(inner, session_id, text).await;
            });
        }
        AgentClientMessage::ClearQueue { session_id } => {
            tokio::spawn(async move {
                handle_clear_queue(inner, session_id).await;
            });
        }
        AgentClientMessage::RetryLast { session_id } => {
            tokio::spawn(async move {
                handle_retry_last(inner, session_id).await;
            });
        }
        AgentClientMessage::UndoSession { session_id } => {
            tokio::spawn(async move {
                handle_session_history(inner, session_id, "undo", "undo").await;
            });
        }
        AgentClientMessage::RedoSession { session_id } => {
            tokio::spawn(async move {
                handle_session_history(inner, session_id, "redo", "redo").await;
            });
        }
        AgentClientMessage::ApproveTool {
            request_id,
            session_id,
            decision,
        } => {
            tokio::spawn(async move {
                handle_permission_reply(inner, session_id, request_id, decision).await;
            });
        }
        AgentClientMessage::DenyTool {
            request_id,
            session_id,
        } => {
            tokio::spawn(async move {
                handle_permission_reply(
                    inner,
                    session_id,
                    request_id,
                    PermissionDecision::No,
                )
                .await;
            });
        }
        AgentClientMessage::ApplyEdit {
            session_id,
            edit_id,
        } => {
            // The agent-server doesn't yet expose a typed "apply
            // proposed edit" endpoint distinct from the tool's own
            // permission gate; bridge the apply decision through the
            // matching permission once it lands. Until then forward
            // it as a "once" approval keyed on the edit id so simple
            // gated edits resolve.
            tokio::spawn(async move {
                handle_permission_reply(
                    inner,
                    session_id,
                    edit_id,
                    PermissionDecision::Yes,
                )
                .await;
            });
        }
        AgentClientMessage::RejectEdit {
            session_id,
            edit_id,
        } => {
            tokio::spawn(async move {
                handle_permission_reply(
                    inner,
                    session_id,
                    edit_id,
                    PermissionDecision::No,
                )
                .await;
            });
        }
        AgentClientMessage::SetProvider {
            session_id,
            provider_id,
        } => {
            tokio::spawn(async move {
                handle_set_provider(inner, session_id, provider_id).await;
            });
        }
        AgentClientMessage::SetModel {
            session_id,
            model,
            thinking,
        } => {
            tokio::spawn(async move {
                handle_set_model(inner, session_id, model, thinking).await;
            });
        }
        AgentClientMessage::SetAgent { session_id, agent } => {
            tokio::spawn(async move {
                handle_set_agent(inner, session_id, agent).await;
            });
        }
        AgentClientMessage::SetThinkingMode {
            session_id,
            thinking,
        } => {
            tokio::spawn(async move {
                handle_set_thinking(inner, session_id, thinking).await;
            });
        }
        AgentClientMessage::ListProviders => {
            tokio::spawn(async move {
                handle_list_providers(inner).await;
            });
        }
        AgentClientMessage::GetConfigDefaults { directory } => {
            tokio::spawn(async move {
                handle_get_config_defaults(inner, directory).await;
            });
        }
        AgentClientMessage::ListAgents { directory } => {
            tokio::spawn(async move {
                handle_list_agents(inner, directory).await;
            });
        }
        AgentClientMessage::ListSkills { directory } => {
            tokio::spawn(async move {
                handle_list_skills(inner, directory).await;
            });
        }
        AgentClientMessage::ShowMcp { directory } => {
            tokio::spawn(async move {
                handle_show_mcp(inner, directory).await;
            });
        }
        AgentClientMessage::ShowPermissions { session_id } => {
            tokio::spawn(async move {
                handle_show_permissions(inner, session_id).await;
            });
        }
        AgentClientMessage::ShowQuestions { session_id } => {
            tokio::spawn(async move {
                handle_show_questions(inner, session_id).await;
            });
        }
        AgentClientMessage::StartSubagent {
            session_id,
            agent,
            prompt,
        } => {
            tokio::spawn(async move {
                handle_start_subagent(inner, session_id, agent, prompt).await;
            });
        }
        AgentClientMessage::Compact { session_id } => {
            tokio::spawn(async move {
                handle_compact(inner, session_id).await;
            });
        }
        AgentClientMessage::SlashCommand { session_id, text } => {
            tokio::spawn(async move {
                handle_slash_command(inner, session_id, text).await;
            });
        }
        AgentClientMessage::HandleQueue { session_id, action } => {
            tokio::spawn(async move {
                handle_queue(inner, session_id, action).await;
            });
        }
        AgentClientMessage::HandlePermit {
            session_id,
            reply,
            request_id,
        } => {
            tokio::spawn(async move {
                handle_permit(inner, session_id, reply, request_id).await;
            });
        }
        AgentClientMessage::HandleAnswer { session_id, answer } => {
            tokio::spawn(async move {
                handle_answer(inner, session_id, answer).await;
            });
        }
        AgentClientMessage::HandleReject {
            session_id,
            request_id,
        } => {
            tokio::spawn(async move {
                handle_reject(inner, session_id, request_id).await;
            });
        }
        AgentClientMessage::SetTitle { session_id, title } => {
            tokio::spawn(async move {
                handle_set_title(inner, session_id, title).await;
            });
        }

        // -- Provider connect / auth flow (`/connect` picker) ------
        AgentClientMessage::ConnectListProviders { directory } => {
            tokio::spawn(async move {
                handle_connect_list_providers(inner, directory).await;
            });
        }
        AgentClientMessage::ConnectStoreApiKey { provider_id, key } => {
            tokio::spawn(async move {
                handle_connect_store_api_key(inner, provider_id, key).await;
            });
        }
        AgentClientMessage::ConnectDisconnect { provider_id } => {
            tokio::spawn(async move {
                handle_connect_disconnect(inner, provider_id).await;
            });
        }
        AgentClientMessage::ConnectOauthAuthorize {
            provider_id,
            method_index,
        } => {
            tokio::spawn(async move {
                handle_connect_oauth_authorize(inner, provider_id, method_index).await;
            });
        }
        AgentClientMessage::ConnectOauthCallback {
            provider_id,
            method_index,
            code,
        } => {
            tokio::spawn(async move {
                handle_connect_oauth_callback(inner, provider_id, method_index, code)
                    .await;
            });
        }

        AgentClientMessage::Ping => {
            let _ = inner.tx.send(AgentServerMessage::Pong);
        }
    }
}

