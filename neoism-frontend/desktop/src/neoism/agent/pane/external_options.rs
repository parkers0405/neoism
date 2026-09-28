use super::*;
use neoism_ui::panels::agent_pane::state::external_options::ExternalOptions;

impl NeoismAgentPane {
    pub(crate) fn external_option_pending(&self) -> bool {
        self.external_options_pending.is_some() || self.external_options_post_request.is_some()
    }

    pub(crate) fn external_options(&self) -> Option<&ExternalOptions> {
        self.external_options.as_ref().filter(|options| {
            self.conversation_source().provider() == Some(options.provider.as_str())
        })
    }

    pub(crate) fn reset_external_options(&mut self) {
        self.external_options_generation = self.external_options_generation.wrapping_add(1);
        self.external_options = None;
        self.draft_external_selections.clear();
        self.external_options_request = None;
        self.external_options_pending = None;
        self.external_options_post_request = None;
        self.external_options_refresh_after_request = false;
        self.external_options_next_refresh = None;
        self.external_options_dirty = true;
        self.pending_external_model_picker = false;
        self.external_options_error = None;
        self.external_picker_option_id = None;
        if self.picker.as_ref().is_some_and(|picker|             picker.kind == NeoismAgentPickerKind::ExternalOption || picker.kind == NeoismAgentPickerKind::ExternalOptionMenu || picker.kind == NeoismAgentPickerKind::Slash) {
            self.picker = None;
        }
    }

    pub(crate) fn invalidate_external_options(&mut self) {
        if self.conversation_source().provider().is_some() {
            // A GET can race a different client's update. Finish the current
            // request, then take one cached follow-up snapshot instead of
            // competing for the ACP lock with a second live request.
            if self.external_options_request.is_some() {
                self.external_options_refresh_after_request = true;
                return;
            }
            self.external_options_dirty = true;
            self.external_options_next_refresh = None;
        }
    }

    pub(crate) fn maybe_refresh_external_options(&mut self) {
        if self.session_id.is_none() {
            let Some(provider) = self.new_chat_source.provider() else { return; };
            if !self.external_options_dirty || self.external_options_request.is_some()
                || self.external_options_next_refresh.is_some_and(|next| Instant::now() < next) { return; }
            let (provider, directory, server, selections) = (provider.to_string(), self.directory.clone(), self.server.clone(), self.draft_external_selections.clone());
            let generation = self.external_options_generation;
            self.external_options_request = Some(generation);
            let tx = self.background_tx.clone();
            let wake = self.event_wake.clone();
            std::thread::spawn(move || {
                let result = super::super::api::fetch_external_options_preview(&server, directory.as_deref(), &provider, &selections);
                let _ = tx.send(NeoismAgentBackgroundUpdate::ExternalOptionsFetched { server, session_id: String::new(), generation, result });
                if let Some(wake) = wake { wake.wake(); }
            });
            return;
        }
        let (Some(provider), Some(session_id)) = (self.conversation_source().provider(), self.session_id.as_deref()) else { return; };
        if self.external_options_request.is_some() || self.external_options_post_request.is_some()
            || self.external_options_next_refresh.is_some_and(|next| Instant::now() < next) { return; }
        if !self.external_options_dirty {
            if self.external_options_pending.is_some() { self.dispatch_external_option(); }
            return;
        }
        let (provider, session_id, server) = (provider.to_string(), session_id.to_string(), self.server.clone());
        let generation = self.external_options_generation;
        self.external_options_request = Some(generation);
        let tx = self.background_tx.clone();
        let wake = self.event_wake.clone();
        std::thread::spawn(move || {
            let result = super::super::api::fetch_external_options(&server, &session_id, &provider);
            let _ = tx.send(NeoismAgentBackgroundUpdate::ExternalOptionsFetched { server, session_id, generation, result });
            if let Some(wake) = wake { wake.wake(); }
        });
    }

    fn dispatch_external_option(&mut self) {
        let (Some(provider), Some(session_id), Some((config_id, value))) = (
            self.conversation_source().provider(), self.session_id.as_deref(), self.external_options_pending.clone()
        ) else { return; };
        let (provider, session_id, server) = (provider.to_string(), session_id.to_string(), self.server.clone());
        self.external_options_generation = self.external_options_generation.wrapping_add(1);
        let generation = self.external_options_generation;
        self.external_options_post_request = Some(generation);
        self.external_options_error = None;
        self.external_options_dirty = false;
        let tx = self.background_tx.clone();
        let wake = self.event_wake.clone();
        std::thread::spawn(move || {
            let result = super::super::api::set_external_option(&server, &session_id, &provider, &config_id, &value);
            let _ = tx.send(NeoismAgentBackgroundUpdate::ExternalOptionSet { server, session_id, generation, result });
            if let Some(wake) = wake { wake.wake(); }
        });
    }

    pub(crate) fn select_external_option(&mut self, config_id: String, value: String) {
        if self.session_id.is_none() {
            let Some(option) = self.external_options.as_mut().filter(|options| self.new_chat_source.provider() == Some(options.provider.as_str()))
                .and_then(|options| options.options.iter_mut().find(|option| option.id == config_id && option.choices.iter().any(|choice| choice.value == value))) else { return; };
            let changed = option.current_value != value;
            let model_changed = option.category == "model" && changed;
            option.current_value = value.clone();
            if model_changed {
                // Model-dependent selectors are unverified until the draft catalog refreshes.
                if let Some(options) = self.external_options.as_mut() {
                    options.options.retain(|option| option.category == "model");
                }
                self.draft_external_selections.clear();
            }
            self.draft_external_selections.insert(config_id, value);
            if changed {
                self.external_options_dirty = true;
                self.external_options_next_refresh = None;
                self.maybe_refresh_external_options();
            }
            return;
        }
        if self.conversation_source().provider().is_none() { return; }
        let Some(option) = self.external_options().and_then(|options| options.options.iter().find(|option| option.id == config_id)) else { return; };
        if self.external_options_pending.is_some() || (option.current_value == value && self.external_options().is_some_and(|options| options.replay_error.is_none())) || !option.choices.iter().any(|choice| choice.value == value) { return; }
        self.external_options_pending = Some((config_id, value));
        self.external_options_next_refresh = None;
        self.maybe_refresh_external_options();
    }

    pub(crate) fn apply_external_options_update(&mut self, server: String, session_id: String, generation: u64, result: Result<ExternalOptions, String>, is_post: bool) -> bool {
        if server != self.server || self.session_id.as_deref() != (if session_id.is_empty() { None } else { Some(session_id.as_str()) }) || generation != self.external_options_generation { return false; }
        let needs_followup = if is_post { self.external_options_dirty } else { std::mem::take(&mut self.external_options_refresh_after_request) };
        if is_post {
            if self.external_options_post_request.take() != Some(generation) || self.external_options_pending.is_none() { return false; }
        } else if self.external_options_request.take() != Some(generation) { return false; }
        match result {
            Ok(mut options) if self.conversation_source().provider() == Some(options.provider.as_str()) => {
                if session_id.is_empty() {
                    if self.draft_external_selections.iter().any(|(id, value)| {
                        options.options.iter().find(|option| &option.id == id)
                            .is_none_or(|option| &option.current_value != value)
                    }) {
                        // The model changed while this request was in flight. Keep the draft
                        // selection and request its catalog rather than showing stale traits.
                        self.external_options_dirty = true;
                        self.external_options_next_refresh = None;
                        self.maybe_refresh_external_options();
                        return true;
                    }
                    self.draft_external_selections.retain(|id, value| {
                        if let Some(option) = options.options.iter_mut().find(|option| &option.id == id && option.choices.iter().any(|choice| &choice.value == value)) {
                            option.current_value = value.clone();
                            true
                        } else { false }
                    });
                    self.external_options = Some(options);
                    self.external_options_error = None;
                    self.external_options_dirty = false;
                    self.external_options_next_refresh = None;
                    return true;
                }
                if is_post && self.external_options.as_ref().is_some_and(|previous| previous.external_session_id.is_some()
                    && previous.external_session_id != options.external_session_id) {
                    self.external_options_dirty = true;
                    self.push_notice("Provider session changed while selecting an option; refreshing controls", NeoismAgentNoticeLevel::Warn);
                    return true;
                }
                if is_post && self.external_options_pending.as_ref().is_some_and(|(id, value)| {
                    options.options.iter().find(|option| &option.id == id).is_none_or(|option| &option.current_value != value)
                }) {
                    self.external_options_dirty = true;
                    self.external_options_next_refresh = Some(Instant::now() + Duration::from_secs(2));
                    self.push_notice("Provider did not confirm the selected option; checking its current value", NeoismAgentNoticeLevel::Warn);
                    return true;
                }
                let first_binding = self.external_options.as_ref().and_then(|old| old.external_session_id.as_ref()) != options.external_session_id.as_ref()
                    && options.external_session_id.is_some();
                self.external_root_bound_refresh |= first_binding;
                if is_post {
                    self.external_options_pending = None;
                } else if let Some((id, value)) = self.external_options_pending.as_ref() {
                    match options.options.iter().find(|option| &option.id == id) {
                        Some(option) if options.replay_error.is_none() && &option.current_value == value => { self.external_options_pending = None; }
                        Some(option) if option.choices.iter().any(|choice| &choice.value == value) => {}
                        _ => {
                            self.external_options_pending = None;
                            self.push_notice("Provider no longer offers the selected option", NeoismAgentNoticeLevel::Warn);
                        }
                    }
                }
                let new_error = options.replay_error.as_deref() != self.external_options_error.as_deref();
                self.external_options_error = options.replay_error.clone();
                if new_error {
                    if let Some(error) = options.replay_error.as_deref() {
                        self.push_notice(format!("Provider options need a new selection: {error}"), NeoismAgentNoticeLevel::Warn);
                    }
                }
                self.external_options = Some(options);
                if self.pending_external_model_picker {
                    if self.input.trim() == "/model" && self.open_external_model_picker_from_slash() {
                        self.external_options_dirty = needs_followup;
                        self.external_options_next_refresh = needs_followup.then(|| Instant::now() + Duration::from_millis(100));
                        self.maybe_refresh_external_options();
                        return true;
                    }
                    self.pending_external_model_picker = false;
                    if self.input.trim() == "/model" {
                        self.push_notice("Provider does not advertise a model selector", NeoismAgentNoticeLevel::Warn);
                    }
                }
                if self.picker.as_ref().is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash) {
                    self.picker = None;
                    self.sync_slash_picker();
                }
                self.external_options_dirty = needs_followup;
                self.external_options_next_refresh = needs_followup.then(|| Instant::now() + Duration::from_millis(100));
                self.maybe_refresh_external_options();
            }
            Ok(options) => {
                self.external_options_error = Some(format!("Provider options returned {} instead of the active provider", options.provider));
                self.external_options_pending = None;
                self.external_options_dirty = needs_followup;
                self.external_options_next_refresh = needs_followup.then(|| Instant::now() + Duration::from_secs(2));
            },
            Err(error) => {
                let busy = error.contains("409");
                let indeterminate = is_post && (error.to_ascii_lowercase().contains("timed out") || error.to_ascii_lowercase().contains("timeout"));
                let new_error = self.external_options_error.as_deref() != Some(error.as_str());
                self.external_options_error = Some(error.clone());
                if is_post && !busy && !indeterminate { self.external_options_pending = None; }
                self.external_options_dirty = needs_followup || indeterminate || (!is_post && busy) || (is_post && !busy && !indeterminate);
                self.external_options_next_refresh = (busy || indeterminate || self.external_options_dirty)
                    .then(|| Instant::now() + Duration::from_secs(2));
                if new_error && (!busy || !is_post) {
                    if neoism_ui::panels::agent_pane::state::external_options::auth_required(&error) {
                        let instruction = if self.conversation_source().provider() == Some("codex") {
                            "Run `codex login` in a terminal, then retry options".to_string()
                        } else {
                            format!("Sign in to {} with its CLI, then retry options", self.conversation_source().label())
                        };
                        self.push_notice(instruction, NeoismAgentNoticeLevel::Warn);
                    } else {
                        self.push_notice(format!("Provider options: {error}"), NeoismAgentNoticeLevel::Warn);
                    }
                }
            }
        }
        if self.picker.as_ref().is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash) { self.sync_slash_picker(); }
        true
    }

    pub(crate) fn open_external_model_picker_from_slash(&mut self) -> bool {
        if self.external_options_pending.is_some() {
            self.pending_external_model_picker = true;
            return true;
        }
        let Some(index) = self.external_options().and_then(|snapshot| snapshot.options.iter().position(|option| option.category == "model")) else { return false; };
        self.pending_external_model_picker = false;
        self.input.clear();
        self.cursor_byte = 0;
        self.picker = None;
        self.open_status_chip_picker(index + 1 + usize::from(!self.has_conversation()));
        true
    }

    pub(crate) fn retry_external_options(&mut self) {
        self.external_options_error = None;
        self.external_options_dirty = true;
        self.external_options_next_refresh = None;
        self.maybe_refresh_external_options();
        self.sync_input_pickers();
    }

    pub(crate) fn take_external_root_bound_refresh(&mut self) -> bool {
        std::mem::take(&mut self.external_root_bound_refresh)
    }

    pub fn external_options_error(&self) -> Option<&str> {
        self.external_options_error.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;

    fn snapshot(value: &str) -> ExternalOptions {
        ExternalOptions::parse(&serde_json::json!({"provider":"claude","externalSessionId":"provider-123","modeFallback":false,"selectedOptions":{},"configOptions":[
            {"id":"provider-model","name":"Model","category":"model","type":"select","currentValue":value,
             "options":[{"value":"a","name":"A"},{"value":"b","name":"B"}]}
        ]}), "claude").unwrap()
    }

    #[test]
    fn model_enter_waits_for_current_provider_options_without_submitting_prompt() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.external_options_generation = 4;
        pane.external_options_request = Some(4);
        pane.input = "/model".into();
        pane.sync_slash_picker();
        assert!(pane.submit());
        assert_eq!(pane.input, "/model");
        assert!(pane.pending_external_model_picker);
        assert!(pane.drain_pending_outbound().is_empty());
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 4, Ok(snapshot("a")), false));
        assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ExternalOption);
        assert_eq!(pane.external_picker_option_id.as_deref(), Some("provider-model"));
        assert_eq!(pane.input, "");
    }

    #[test]
    fn lazy_root_pending_slash_hydrates_without_dropping_draft() {
        let mut pane = NeoismAgentPane::default();
        pane.create_new_chat_from(ConversationSource::ClaudeCode);
        assert!(pane.drain_pending_outbound().is_empty());
        pane.insert_text("/rev");
        assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::Slash);
        assert!(pane.picker.as_ref().unwrap().selected_option().is_none());
        pane.session_id = Some("root".into());
        pane.reset_external_options();
        pane.sync_input_pickers();
        assert_eq!(pane.input, "/rev");
        pane.external_options_generation = 1;
        pane.external_options_request = Some(1);
        let mut loaded = snapshot("a");
        loaded.available_commands.push(neoism_ui::panels::agent_pane::state::external_options::ExternalCommand {
            name: "review".into(), description: "Review files".into(), input_hint: None,
        });
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 1, Ok(loaded), false));
        assert_eq!(pane.input, "/rev");
        assert_eq!(pane.picker.as_ref().unwrap().selected_option().unwrap().value, "review");
        assert!(pane.take_external_root_bound_refresh());
        assert!(!pane.take_external_root_bound_refresh());
    }

    #[test]
    fn failed_get_keeps_slash_retry_visible_and_blank_retry_draft_only() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::Codex;
        pane.input = "/unknown".into();
        pane.cursor_byte = pane.input.len();
        pane.sync_input_pickers();
        assert!(pane.picker.as_ref().unwrap().selected_option().is_none());
        pane.session_id = Some("root".into());
        pane.external_options_dirty = true;
        pane.external_options_request = Some(2);
        pane.external_options_generation = 2;
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 2, Err("offline".into()), false));
        assert_eq!(pane.picker.as_ref().unwrap().selected_option().unwrap().value, "__retry_external_options");
        assert!(!pane.external_options_dirty); // no frame-by-frame retry
        pane.commit_picker();
        assert_eq!(pane.input, "/unknown");
        assert!(pane.external_options_request.is_some()); // explicit retry dispatched
        pane.session_id = None;
        pane.external_options_request = None;
        pane.external_options_error = Some("root failed".into());
        pane.retry_external_options();
        assert!(pane.external_options_request.is_some());
        assert!(pane.drain_pending_outbound().is_empty());
        assert!(pane.session_id.is_none());
    }

    #[test]
    fn codex_auth_failure_stays_in_draft_and_retry_does_not_create_chat() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::Codex;
        pane.input = "keep this draft".into();
        pane.external_options_request = Some(pane.external_options_generation);
        let error = crate::neoism::agent::api::http_error(400, "Bad Request", r#"{"code":"request.invalid","message":"Codex ACP authentication required for session/new: Authentication required. Sign in with the provider's CLI, then retry."}"#);
        assert!(pane.apply_external_options_update(pane.server.clone(), String::new(), pane.external_options_generation, Err(error), false));
        assert!(neoism_ui::panels::agent_pane::state::external_options::auth_required(pane.external_options_error().unwrap()));
        assert!(!pane.external_options_error().unwrap().contains("\"code\""));
        assert!(pane.ui_events.iter().any(|event| matches!(event, NeoismAgentUiEvent::Notice { message, .. } if message.contains("`codex login`"))));
        pane.retry_external_options();
        assert_eq!(pane.input, "keep this draft");
        assert!(pane.session_id.is_none());
        assert!(pane.external_options_request.is_some());
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn identical_provider_snapshot_has_same_draft_and_live_picker_values() {
        let options = ExternalOptions::parse(&serde_json::json!({
            "provider":"claude","externalSessionId":null,"modeFallback":false,"selectedOptions":{},
            "configOptions":[
                {"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]},
                {"id":"model","name":"Model","category":"model","type":"select","currentValue":"sonnet","options":[{"value":"sonnet","name":"Sonnet"},{"value":"opus","name":"Opus"}]}
            ]
        }), "claude").unwrap();
        let mut draft = NeoismAgentPane::default();
        draft.new_chat_source = ConversationSource::ClaudeCode;
        draft.external_options = Some(options.clone());
        let mut live = NeoismAgentPane::default();
        live.new_chat_source = ConversationSource::ClaudeCode;
        live.session_id = Some("root".into());
        let mut bound = options;
        bound.external_session_id = Some("provider-session".into());
        live.external_options = Some(bound);
        assert_eq!(draft.external_options().unwrap().display_order(), live.external_options().unwrap().display_order());
        for option_index in [1, 0] {
            draft.open_status_chip_picker(option_index + 2);
            live.open_status_chip_picker(option_index + 1);
            assert_eq!(draft.external_picker_option_id, live.external_picker_option_id);
            assert_eq!(draft.picker.as_ref().unwrap().selected_option().unwrap().value, live.picker.as_ref().unwrap().selected_option().unwrap().value);
        }
    }

    #[test]
    fn options_fetch_survives_its_own_session_updates() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.external_options_generation = 4;
        pane.external_options_request = Some(4);
        pane.external_options_dirty = false;
        pane.invalidate_external_options(); // binding, config, and command updates from GET
        assert_eq!(pane.external_options_request, Some(4));
        assert_eq!(pane.external_options_generation, 4);
        assert!(!pane.external_options_dirty);
        assert!(pane.external_options_refresh_after_request);
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 4, Ok(snapshot("a")), false));
        assert!(pane.external_options().is_some());
        assert!(pane.external_options_dirty);
        assert!(pane.external_options_next_refresh.is_some());
    }

    #[test]
    fn stale_fetch_cannot_change_switched_session_or_override_post() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("first".into());
        pane.external_options_generation = 2;
        pane.external_options_request = Some(2);
        pane.reset_external_options();
        pane.session_id = Some("second".into());
        assert!(!pane.apply_external_options_update(pane.server.clone(), "first".into(), 2, Ok(snapshot("a")), false));
        pane.external_options_request = Some(3);
        pane.external_options_generation = 4;
        pane.external_options_pending = Some(("provider-model".into(), "b".into()));
        pane.external_options_post_request = Some(4);
        assert!(!pane.apply_external_options_update(pane.server.clone(), "second".into(), 3, Ok(snapshot("a")), false));
        assert!(pane.apply_external_options_update(pane.server.clone(), "second".into(), 4, Ok(snapshot("b")), true));
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "B");
    }

    #[test]
    fn send_waits_for_active_provider_option_confirmation_without_losing_draft() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::OpenCode;
        pane.session_id = Some("root".into());
        pane.input = "use the selected model".into();
        pane.external_options_pending = Some(("model".into(), "chosen".into()));
        assert!(pane.submit());
        assert_eq!(pane.input, "use the selected model");
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn prechat_model_change_replaces_conditional_selectors_from_confirmed_catalog() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.external_options = Some(snapshot("a"));
        pane.external_options_request = Some(pane.external_options_generation);
        pane.select_external_option("provider-model".into(), "b".into());
        assert_eq!(pane.external_options().unwrap().options.len(), 1);
        assert_eq!(pane.draft_external_selections.get("provider-model").map(String::as_str), Some("b"));
        let refreshed = ExternalOptions::parse(&serde_json::json!({"provider":"claude","externalSessionId":null,"modeFallback":false,
            "selectedOptions":{"provider-model":"b"},"configOptions":[
                {"id":"provider-model","name":"Model","category":"model","type":"select","currentValue":"b",
                 "options":[{"value":"a","name":"A"},{"value":"b","name":"B"}]},
                {"id":"effort","name":"Effort","category":"thought_level","type":"select","currentValue":"low",
                 "options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]}
            ]}), "claude").unwrap();
        assert!(pane.apply_external_options_update(pane.server.clone(), String::new(), pane.external_options_generation, Ok(refreshed), false));
        assert_eq!(pane.external_options().unwrap().options.len(), 2);
        assert_eq!(pane.external_options().unwrap().options[0].current_value, "b");
        assert!(pane.session_id.is_none());
    }

    #[test]
    fn prechat_options_are_draft_only_and_stale_provider_updates_are_ignored() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.external_options_request = Some(pane.external_options_generation);
        let mut preview = snapshot("a");
        preview.external_session_id = None;
        assert!(pane.apply_external_options_update(pane.server.clone(), String::new(), pane.external_options_generation, Ok(preview.clone()), false));
        assert!(pane.session_id.is_none());
        pane.select_external_option("provider-model".into(), "b".into());
        assert_eq!(pane.draft_external_selections.get("provider-model").map(String::as_str), Some("b"));
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "B");
        let old_generation = pane.external_options_generation;
        pane.set_conversation_source(ConversationSource::Codex);
        assert!(pane.draft_external_selections.is_empty());
        assert!(!pane.apply_external_options_update(pane.server.clone(), String::new(), old_generation, Ok(preview), false));
        assert!(pane.session_id.is_none());
    }

    #[test]
    fn reordered_footer_and_overflow_target_original_provider_option_ids() {
        use neoism_ui::panels::agent_pane::state::external_options::{ExternalChoice, ExternalOption};
        let mut pane = NeoismAgentPane::default();
        pane.session_id = Some("root".into());
        pane.new_chat_source = ConversationSource::Codex;
        let mut options = snapshot("a");
        options.provider = "codex".into();
        options.options.push(ExternalOption {
            id: "cache".into(), name: "Cache".into(), category: "model_config".into(), current_value: "on".into(),
            choices: vec![ExternalChoice { value: "on".into(), name: "On".into(), group: None }],
        });
        options.options.push(ExternalOption {
            id: "mode".into(), name: "Mode".into(), category: "mode".into(), current_value: "code".into(),
            choices: vec![ExternalChoice { value: "code".into(), name: "Code".into(), group: None }],
        });
        pane.external_options = Some(options);
        pane.open_status_chip_picker(3); // visually first mode, original index 2
        assert_eq!(pane.external_picker_option_id.as_deref(), Some("mode"));
        pane.open_status_chip_picker(4); // overflow menu
        assert_eq!(pane.picker.as_ref().unwrap().selected_option().unwrap().value, "mode");
        pane.picker.as_mut().unwrap().move_selection(1);
        assert!(pane.commit_picker());
        assert_eq!(pane.external_picker_option_id.as_deref(), Some("provider-model"));
    }

    #[test]
    fn overflow_menu_reaches_later_provider_choices_without_touching_draft() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.input = "unfinished prompt".into();
        let mut options = snapshot("a");
        options.options[0].name = "A very long model label".into();
        options.options.push(neoism_ui::panels::agent_pane::state::external_options::ExternalOption {
            id: "thought".into(), name: "Thinking".into(), category: "".into(), current_value: "low".into(),
            choices: vec![neoism_ui::panels::agent_pane::state::external_options::ExternalChoice { value: "low".into(), name: "Low".into(), group: None }],
        });
        pane.external_options = Some(options);
        pane.register_status_chip_rect(3, [10.0, 10.0, 32.0, 20.0]);
        let hit = pane.status_chip_at(15.0, 15.0).unwrap();
        pane.open_status_chip_picker(hit); // two provider options + overflow
        assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ExternalOptionMenu);
        assert_eq!(pane.picker.as_ref().unwrap().selected_option().unwrap().value, "provider-model");
        pane.picker.as_mut().unwrap().move_selection(1);
        assert!(pane.commit_picker());
        assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::ExternalOption);
        assert_eq!(pane.external_picker_option_id.as_deref(), Some("thought"));
        assert_eq!(pane.picker.as_ref().unwrap().selected_option().unwrap().title, "Low");
        assert_eq!(pane.input, "unfinished prompt");
    }

    #[test]
    fn source_change_closes_provider_command_picker_and_rejects_old_snapshot() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        let mut confirmed = snapshot("a");
        confirmed.available_commands.push(neoism_ui::panels::agent_pane::state::external_options::ExternalCommand {
            name: "compact".into(), description: "Claude compact".into(), input_hint: None,
        });
        pane.external_options = Some(confirmed);
        pane.input = "/comp".into();
        pane.sync_input_pickers();
        assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::Slash);
        let old_generation = pane.external_options_generation;
        pane.set_conversation_source(ConversationSource::Codex);
        assert!(pane.picker.is_none());
        assert!(pane.external_options().is_none());
        assert!(!pane.apply_external_options_update(pane.server.clone(), "root".into(), old_generation, Ok(snapshot("b")), false));
        pane.sync_input_pickers();
        assert_eq!(pane.picker.as_ref().unwrap().kind, NeoismAgentPickerKind::Slash);
        assert!(pane.picker.as_ref().unwrap().options().iter().all(|row| row.value.is_empty()));
    }

    #[test]
    fn post_from_another_provider_session_cannot_replace_current_commands() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.external_options = Some(snapshot("a"));
        pane.external_options_generation = 7;
        pane.external_options_pending = Some(("provider-model".into(), "b".into()));
        pane.external_options_post_request = Some(7);
        let mut other = snapshot("b");
        other.external_session_id = Some("different-process".into());
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 7, Ok(other), true));
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "A");
        assert!(pane.external_options_dirty);
    }

    #[test]
    fn selection_waits_for_inflight_get_then_applies_confirmed_choice() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.external_options = Some(snapshot("a"));
        pane.external_options_generation = 8;
        pane.external_options_request = Some(8);
        pane.external_options_dirty = false;
        pane.select_external_option("provider-model".into(), "b".into());
        assert_eq!(pane.external_options_request, Some(8));
        assert_eq!(pane.external_options_post_request, None);
        assert_eq!(pane.external_options_pending.as_ref().unwrap().1, "b");
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 8, Ok(snapshot("a")), false));
        let post = pane.external_options_post_request.expect("POST follows GET");
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), post, Ok(snapshot("b")), true));
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "B");
        assert!(pane.external_options_pending.is_none());
    }

    #[test]
    fn timed_out_post_reconciles_confirmed_value_before_retry() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.external_options = Some(snapshot("a"));
        pane.external_options_generation = 3;
        pane.external_options_post_request = Some(3);
        pane.external_options_pending = Some(("provider-model".into(), "b".into()));
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 3, Err("request timed out".into()), true));
        assert_eq!(pane.external_options_pending.as_ref().unwrap().1, "b");
        assert!(pane.external_options_dirty);
        pane.external_options_next_refresh = None;
        pane.maybe_refresh_external_options();
        assert_eq!(pane.external_options_request, Some(3));
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 3, Ok(snapshot("b")), false));
        assert!(pane.external_options_pending.is_none());
        assert!(pane.external_options_post_request.is_none());
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "B");
    }

    #[test]
    fn switched_chat_ignores_old_post_and_reloads_provider_confirmation() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("first".into());
        pane.external_options = Some(snapshot("a"));
        pane.external_options_generation = 5;
        pane.external_options_pending = Some(("provider-model".into(), "b".into()));
        pane.external_options_post_request = Some(5);
        pane.reset_external_options();
        pane.session_id = Some("second".into());
        assert!(!pane.apply_external_options_update(pane.server.clone(), "first".into(), 5, Ok(snapshot("b")), true));
        pane.reset_external_options();
        pane.session_id = Some("first".into());
        pane.external_options_dirty = false;
        let generation = pane.external_options_generation;
        pane.external_options_request = Some(generation);
        assert!(pane.apply_external_options_update(pane.server.clone(), "first".into(), generation, Ok(snapshot("b")), false));
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "B");
        assert!(pane.external_options_pending.is_none());
    }

    #[test]
    fn failed_replay_still_offers_model_recovery_without_claiming_it_is_applied() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::OpenCode;
        pane.session_id = Some("root".into());
        pane.external_options_generation = 2;
        pane.external_options_request = Some(2);
        let mut stale = snapshot("a");
        stale.provider = "opencode".into();
        stale.replay_error = Some("saved model is unavailable".into());
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 2, Ok(stale), false));
        assert_eq!(pane.external_options_error(), Some("saved model is unavailable"));
        pane.input = "/model".into();
        assert!(pane.open_external_model_picker_from_slash());
        assert_eq!(pane.external_picker_option_id.as_deref(), Some("provider-model"));
        pane.select_external_option("provider-model".into(), "a".into());
        let request = pane.external_options_post_request.expect("same displayed value must repair stale selection");
        let mut confirmed = snapshot("a");
        confirmed.provider = "opencode".into();
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), request, Ok(confirmed), true));
        assert!(pane.external_options_error().is_none());
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "A");
    }

    #[test]
    fn failed_selection_keeps_confirmed_value_and_allows_retry() {
        let mut pane = NeoismAgentPane::default();
        pane.new_chat_source = ConversationSource::ClaudeCode;
        pane.session_id = Some("root".into());
        pane.external_options = Some(snapshot("a"));
        pane.external_options_generation = 1;
        pane.external_options_pending = Some(("provider-model".into(), "b".into()));
        pane.external_options_post_request = Some(1);
        assert!(pane.apply_external_options_update(pane.server.clone(), "root".into(), 1, Err("409 busy".into()), true));
        assert_eq!(pane.external_options().unwrap().options[0].selected_label(), "A");
        assert_eq!(pane.external_options_pending.as_ref().unwrap().1, "b");
        assert!(pane.external_options_post_request.is_none());
        assert!(!pane.external_options_dirty);
        pane.external_options_next_refresh = None;
        pane.maybe_refresh_external_options();
        assert!(pane.external_options_post_request.is_some());
    }
}
