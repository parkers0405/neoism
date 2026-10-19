//! Desktop-only request ownership. Never perform request I/O in a frame/input handler.
use super::*;

impl NeoismAgentPane {
    pub(in crate::neoism::agent) fn request_interaction_reply(
        &mut self,
        id: String,
        permission_reply: Option<String>,
        path: String,
        body: Option<serde_json::Value>,
    ) {
        let server = self.server.clone();
        let tx = self.background_sender();
        let failed_id = id.clone();
        let permission = permission_reply.is_some();
        if let Err(error) = std::thread::Builder::new()
            .name("neoism-agent-interaction".into())
            .spawn(move || {
                let result = super::super::api::api_request_json(
                    &server,
                    "POST",
                    &path,
                    body.as_ref(),
                )
                .map(|_| ());
                let _ = tx.send(NeoismAgentBackgroundUpdate::InteractionReplyCompleted {
                    server,
                    id,
                    permission_reply,
                    result,
                });
            })
        {
            if permission {
                self.permission_reply_failed(&failed_id, error.to_string());
            } else {
                self.question_reply_failed(&failed_id, error.to_string());
            }
        }
    }

    /// Keep the last authoritative row until success (no speculative delete or
    /// pin rollback can overwrite a newer SSE update). Repeated clicks on the
    /// same session are single-flight, independent of the prompt FIFO.
    pub(crate) fn request_session_mutation(
        &mut self,
        server: String,
        session_id: String,
        mutation: AgentSessionMutation,
    ) {
        let key = (server.clone(), session_id.clone());
        if !self.session_mutations_in_flight.insert(key.clone()) {
            return;
        }
        let tx = self.background_sender();
        let result = std::thread::Builder::new()
            .name("neoism-agent-session-mutation".into())
            .spawn(move || {
                let result = match &mutation {
                    AgentSessionMutation::Pin(pinned) => {
                        super::super::api::set_session_pinned(
                            &server,
                            &session_id,
                            *pinned,
                        )
                        .map(|_| ())
                    }
                    AgentSessionMutation::Rename(title) => {
                        super::super::api::rename_session(&server, &session_id, title)
                    }
                    AgentSessionMutation::Delete => {
                        super::super::api::delete_session(&server, &session_id)
                    }
                };
                let _ = tx.send(NeoismAgentBackgroundUpdate::SessionMutationCompleted {
                    server,
                    session_id,
                    mutation,
                    result,
                });
            });
        if let Err(error) = result {
            self.session_mutations_in_flight.remove(&key);
            self.push_notice(
                format!("Could not start conversation request: {error}"),
                NeoismAgentNoticeLevel::Error,
            );
        }
    }

    /// Explicit retry retains the original session, endpoint, parts and message
    /// ID. The server's messageID admission deduplication is the write barrier;
    /// this is deliberately not a generic HTTP/POST retry policy.
    pub(in crate::neoism::agent) fn retry_uncertain_prompt(&mut self) {
        let index = self.uncertain_prompts.iter().rposition(|request| {
            request.server == self.server
                && match &request.origin_session_id {
                    Some(id) => self.session_id.as_ref() == Some(id),
                    None => {
                        self.session_id.is_none()
                            && request.origin_draft_id == self.prompt_draft_id
                    }
                }
        });
        let Some(index) = index else {
            self.push_notice(
                "No uncertain prompt in this conversation",
                NeoismAgentNoticeLevel::Info,
            );
            return;
        };
        if self.uncertain_prompts[index].origin_session_id.is_none() {
            self.push_notice("Session creation could not be confirmed. Reopen the conversation from the sidebar; retry will not create another session.", NeoismAgentNoticeLevel::Warn);
            return;
        }
        let mut request = self.uncertain_prompts.remove(index).unwrap();
        request.reconcile_before_send = true;
        // Do not increment the optimistic queue counter twice or create a
        // second transcript bubble for the same stable message identity.
        self.pending_prompt_dispatches.push_back(request);
        self.start_next_prompt_dispatch();
    }

    /// For patches with no response payload. Capture server/session before
    /// spawning; completion errors must not be attached to a different chat.
    pub(in crate::neoism::agent) fn request_patch(
        &mut self,
        session_id: String,
        title: &str,
        body: serde_json::Value,
    ) {
        let server = self.server.clone();
        let tx = self.background_sender();
        let title = title.to_string();
        if let Err(error) = std::thread::Builder::new()
            .name("neoism-agent-session-patch".into())
            .spawn(move || {
                if let Err(error) = super::super::api::api_request_json(
                    &server,
                    "PATCH",
                    &format!("/v2/sessions/{session_id}"),
                    Some(&body),
                ) {
                    let _ = tx.send(NeoismAgentBackgroundUpdate::RequestFailed {
                        server,
                        session_id: Some(session_id),
                        title,
                        error,
                    });
                }
            })
        {
            self.push_notice(
                format!("Could not start request: {error}"),
                NeoismAgentNoticeLevel::Error,
            );
        }
    }
}
