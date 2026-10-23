use super::*;

impl NeoismAgentPane {
    pub(super) fn hydrate_session_images(
        &mut self,
        session_id: &str,
        messages: &[NeoismAgentMessage],
    ) {
        let server = self.server.clone();
        let progress_server = server.clone();
        let owner = session_id.to_owned();
        let tx = self.background_sender();
        let progress_tx = tx.clone();
        let admitted = super::super::api::generated_images::hydrate_page(
            &self.server,
            messages,
            move |patch| {
                let _ = tx.send(NeoismAgentBackgroundUpdate::SessionImagesHydrated {
                    server: server.clone(),
                    session_id: owner.clone(),
                    patch,
                });
            },
            move || {
                let _ = progress_tx.send(
                    NeoismAgentBackgroundUpdate::ImageHydrationProgress {
                        server: progress_server.clone(),
                    },
                );
            },
        );
        if admitted {
            self.image_hydration_pending.remove(session_id);
        } else {
            self.image_hydration_pending.insert(session_id.to_owned());
        }
    }

    pub(super) fn retry_session_images(&mut self) {
        let pending = std::mem::take(&mut self.image_hydration_pending);
        for session_id in pending {
            let messages = if self.session_id.as_deref() == Some(session_id.as_str()) {
                Some(&self.messages)
            } else {
                self.session_cache
                    .get(&session_id)
                    .map(|cached| &cached.messages)
            };
            let Some(messages) = messages else {
                continue;
            };
            // Retry only lightweight descriptors, not copies of tool output or
            // transcript text. Successful patches no longer have artifact URLs.
            let descriptors: Vec<_> = messages
                .iter()
                .filter(|message| {
                    message
                        .images
                        .iter()
                        .any(|image| image.url.starts_with("/v2/artifacts/"))
                })
                .map(|message| {
                    let mut descriptor =
                        NeoismAgentMessage::assistant("").with_id(message.id.clone());
                    descriptor.images = message.images.clone();
                    descriptor
                })
                .collect();
            self.hydrate_session_images(&session_id, &descriptors);
        }
    }

    pub(super) fn apply_session_image_patch(
        &mut self,
        session_id: &str,
        patch: &super::super::api::generated_images::ImagePatch,
    ) -> bool {
        if self.session_id.as_deref() == Some(session_id) {
            let mut dirty = Vec::new();
            for (index, message) in self.messages.iter_mut().enumerate() {
                if message.id == patch.message_id
                    && patch.apply(std::slice::from_mut(message))
                {
                    dirty.push(index);
                }
            }
            let changed = !dirty.is_empty();
            for index in dirty {
                self.mark_timeline_message_dirty_at(index);
            }
            changed
        } else if let Some(cached) = self.session_cache.get_mut(session_id) {
            if patch.apply(&mut cached.messages) {
                cached.invalidate_timeline_layout();
                return true;
            }
            false
        } else {
            false
        }
    }

    pub(super) fn discard_initial_session_preview(&mut self, session_id: &str) {
        if self.session_id.as_deref() == Some(session_id) {
            if let Some(preview) = self.initial_history_preview.take() {
                self.timeline_history.oldest_loaded_cursor = None;
                super::super::api::conversation_cache::discard_unchanged_preview(
                    &mut self.messages,
                    Some(preview),
                );
                self.invalidate_timeline_layout();
            }
        }
        if let Some(cached) = self.session_cache.get_mut(session_id) {
            if let Some(preview) = cached.initial_history_preview.take() {
                cached.timeline_history.oldest_loaded_cursor = None;
                super::super::api::conversation_cache::discard_unchanged_preview(
                    &mut cached.messages,
                    Some(preview),
                );
                cached.invalidate_timeline_layout();
            }
        }
    }

    pub(super) fn apply_preloaded_session_metadata(
        &mut self,
        session_id: &str,
        state: SessionState,
    ) {
        self.session_sources
            .insert(session_id.to_owned(), state.source);
        if let Some(history) = state.imported_history.as_ref() {
            self.session_histories
                .insert(session_id.to_owned(), history.clone());
        } else {
            self.session_histories.remove(session_id);
        }
        // A persisted clear is authoritative even when an earlier live plan
        // event is still retained in the pane.
        if state.plan_todos.as_ref().is_some_and(Vec::is_empty)
            || (state.source.provider().is_some() && state.plan_todos.is_none())
        {
            self.plan_todo_events.remove(session_id);
        }
        let plan = self
            .plan_todo_events
            .get(session_id)
            .cloned()
            .or_else(|| state.plan_todos.as_deref().map(plan_from_shared));
        if self.session_id.as_deref() != Some(session_id) {
            let cached = self
                .session_cache
                .entry(session_id.to_owned())
                .or_insert_with(CachedAgentSession::live_only);
            let changed = cached.state.source != state.source
                || cached.state.plan_todos != state.plan_todos;
            cached.state = state;
            if changed {
                ingest::apply_authoritative_plan(
                    &mut cached.messages,
                    plan.as_ref(),
                    cached.state.source.provider().is_some(),
                );
                cached.invalidate_timeline_layout();
            }
            return;
        }
        let source_changed = self.new_chat_source != state.source;
        if source_changed {
            self.reset_external_options();
        }
        self.new_chat_source = state.source;
        self.session_title = state.title;
        self.parent_session_id = state.parent_id;
        if let Some(directory) = state.directory {
            if self.directory.as_ref() != Some(&directory) {
                self.directory = Some(directory);
                self.invalidate_skill_options();
            }
        }
        self.refresh_provider_readiness();
        if let Some(agent) = state.agent {
            self.agent = Some(agent);
        }
        let model_changed = state
            .model
            .as_ref()
            .is_some_and(|model| self.model != *model);
        if let Some(model) = state.model {
            self.model = model;
        }
        if model_changed {
            self.model_context_limit = None;
        }
        if model_changed || self.model_context_limit.is_none() {
            self.execute_refresh_model_context_limit_command();
        }
        self.connection_id = state.connection_id;
        self.thinking = state.thinking;
        if source_changed
            || self.current_plan_todos != plan
            || (state.source.provider().is_some()
                && !self
                    .messages
                    .iter()
                    .any(|message| message.id == "todos-snapshot"))
        {
            ingest::apply_authoritative_plan(
                &mut self.messages,
                plan.as_ref(),
                state.source.provider().is_some(),
            );
            self.current_plan_todos = plan;
            self.invalidate_timeline_layout();
        }
    }
}
