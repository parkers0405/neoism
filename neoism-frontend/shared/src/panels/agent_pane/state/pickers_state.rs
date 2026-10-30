use super::*;

#[cfg(test)]
#[path = "model_hydration_tests.rs"]
mod model_hydration_tests;

const MCP_CATALOG_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Default)]
pub(super) struct McpCatalogRefresh {
    in_flight: bool,
    started_at: Option<Instant>,
    invalidated: bool,
    completed_at: Option<Instant>,
    directory: Option<String>,
}

impl McpCatalogRefresh {
    fn request_expired(&self) -> bool {
        self.in_flight && self.started_at.is_none_or(|at| at.elapsed() >= MCP_CATALOG_REQUEST_TIMEOUT)
    }
}

impl NeoismAgentPane {
    pub fn open_agent_picker(&mut self) {
        if self.new_chat_source.provider().is_some() {
            return;
        }
        self.status_chip_activated = Some((0, Instant::now()));
        self.push_outbound(OutboundAgentCommand::RefreshAgents {
            directory: self.directory.clone(),
        });
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Agent,
            "Agents",
            self.agent_options.clone(),
            self.agent_options
                .iter()
                .position(|option| {
                    option.value == self.agent.as_deref().unwrap_or("build")
                })
                .unwrap_or(0),
        ));
    }

    pub fn open_model_picker(&mut self) {
        if self.new_chat_source.provider().is_some() {
            return;
        }
        self.status_chip_activated = Some((1, Instant::now()));
        let options = self.model_picker_options();
        let selected = options
            .iter()
            .position(|option| option.is_selectable() && option.value == self.model)
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Model,
            "Select model",
            options,
            selected,
        ));
        self.refresh_visible_model_catalog();
    }

    pub fn refresh_visible_model_catalog(&mut self) {
        let Some(picker) = self.picker.as_mut().filter(|p| p.kind == NeoismAgentPickerKind::Model) else { return; };
        if picker.model_refresh_started.is_some() { return; }
        static NEXT_REQUEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let request_id = format!("models-{}", NEXT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        picker.model_request_id = Some(request_id.clone());
        picker.model_request_directory = self.directory.clone();
        picker.begin_model_catalog_request();
        picker.set_loading(true);
        self.push_outbound(OutboundAgentCommand::RefreshModelCatalog { request_id, directory: self.directory.clone() });
    }

    pub fn apply_model_catalog(&mut self, options: Vec<NeoismAgentPickerOption>, status: Option<&Value>, request_id: Option<&str>) -> bool {
        if let Some(picker) = self.picker.as_mut() { picker.expire_model_catalog_request(); }
        // Uncorrelated legacy reads can update metadata but must not finish a
        // newer picker request. Closing/replacing the picker cancels ownership.
        if let Some(id) = request_id {
            if !self.picker.as_ref().is_some_and(|p| p.kind == NeoismAgentPickerKind::Model && p.model_request_id.as_deref() == Some(id) && p.model_request_directory == self.directory) { return false; }
        } else if self.picker.as_ref().is_some_and(|p| p.model_request_id.is_some()) { return false; }
        let failed = status.and_then(|s| s.get("state")).and_then(Value::as_str) == Some("retryable-error");
        if !failed || self.model_options.is_empty() { self.set_model_options(options); }
        if let Some(picker) = self.picker.as_mut().filter(|p| p.kind == NeoismAgentPickerKind::Model) {
            picker.apply_model_catalog_status(status);
        }
        true
    }

    pub fn open_mcp_picker(&mut self) {
        let mut picker = NeoismAgentPicker::new(
            NeoismAgentPickerKind::Mcp,
            "MCP servers",
            Vec::new(),
            0,
        );
        picker.loading = true;
        self.picker = Some(picker);
        self.refresh_mcp_if_visible();
    }

    /// Hosts that resolve an unset pane directory against their workspace default
    /// record that effective scope before dispatching the catalog command.
    pub fn set_mcp_catalog_request_directory(&mut self, directory: Option<String>) {
        self.mcp_catalog_refresh.directory = directory;
    }

    pub fn set_mcp_status_for_directory(&mut self, directory: Option<String>, status: Value) {
        let expected = self.directory.as_ref().or(self.mcp_catalog_refresh.directory.as_ref());
        if directory.as_ref() != expected {
            if self.mcp_catalog_refresh.in_flight && self.mcp_catalog_refresh.directory == directory {
                self.mcp_catalog_refresh.in_flight = false;
                self.mcp_catalog_refresh.invalidated = false;
                self.refresh_mcp_if_visible();
            }
            return;
        }
        self.set_mcp_status(status);
    }

    pub fn set_mcp_status(&mut self, status: Value) {
        self.mcp_catalog_refresh.in_flight = false;
        self.mcp_catalog_refresh.started_at = None;
        self.mcp_catalog_refresh.completed_at = Some(Instant::now());
        // An event/action invalidated this read while it was running. Do not
        // briefly expose stale action controls; retain loading/confirmed rows
        // until the single coalesced follow-up has authoritative data.
        if std::mem::take(&mut self.mcp_catalog_refresh.invalidated) {
            self.refresh_mcp_if_visible();
            return;
        }
        if let Some(picker) = self.picker.as_mut() {
            picker.apply_mcp_status(&status);
        }
    }

    pub fn mcp_picker_visible(&self) -> bool {
        self.picker.as_ref().is_some_and(|picker| matches!(
            picker.kind,
            NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions
        ))
    }

    /// Passive catalog polling; hosts drain this through their existing async IO.
    /// A missing response releases the latch after a bounded deadline. Only GET
    /// catalog reads are retried; this never makes runtime connections.
    pub(super) fn mcp_catalog_poll_due(&self) -> bool {
        self.mcp_picker_visible()
            && (self.mcp_catalog_refresh.request_expired()
                || (!self.mcp_catalog_refresh.in_flight
                    && self.mcp_catalog_refresh.completed_at.is_none_or(|at| {
                        at.elapsed() >= std::time::Duration::from_secs(2)
                    })))
    }

    pub fn refresh_mcp_if_visible(&mut self) {
        if !self.mcp_picker_visible() {
            return;
        }
        if self.mcp_catalog_refresh.in_flight && !self.mcp_catalog_refresh.request_expired() {
            // One trailing read ensures an event arriving during a stale read
            // cannot be lost, without allowing notification bursts to fan out.
            self.mcp_catalog_refresh.invalidated = true;
            return;
        }
        // The replacement read also satisfies any coalesced invalidation.
        self.mcp_catalog_refresh.invalidated = false;
        self.mcp_catalog_refresh.in_flight = true;
        self.mcp_catalog_refresh.started_at = Some(Instant::now());
        self.mcp_catalog_refresh.directory = self.directory.clone();
        self.push_outbound(OutboundAgentCommand::RefreshMcp {
            directory: self.directory.clone(),
        });
    }

    /// A transport rejection is not an MCP server error. Retain confirmed rows
    /// and retry on the normal poll cadence without adding transcript messages.
    pub fn mcp_catalog_send_failed(&mut self) {
        self.mcp_catalog_refresh.in_flight = false;
        self.mcp_catalog_refresh.started_at = None;
        self.mcp_catalog_refresh.invalidated = false;
        self.mcp_catalog_refresh.completed_at = Some(Instant::now());
        if let Some(picker) = self.picker.as_mut().filter(|picker| matches!(
            picker.kind,
            NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions
        )) {
            picker.set_loading(false);
        }
    }

    pub fn apply_mcp_failure(&mut self, name: Option<String>, error: String) {
        if name.is_none() {
            self.mcp_catalog_refresh.in_flight = false;
            self.mcp_catalog_refresh.invalidated = false;
            self.mcp_catalog_refresh.completed_at = Some(Instant::now());
        }
        if let Some(picker) = self.picker.as_mut().filter(|picker| {
            matches!(
                picker.kind,
                NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions
            )
        }) {
            picker.set_loading(false);
        }
        // Retain confirmed state, never optimistically flip a failed toggle.
        self.system_message(name.unwrap_or_else(|| "MCP".to_string()), error);
    }

    pub fn open_mcp_actions(&mut self, value: &str) {
        let Ok(entry) = serde_json::from_str::<Value>(value) else {
            return;
        };
        self.picker = Some(NeoismAgentPicker::mcp_actions(&entry, self.picker.as_ref()));
    }

    pub fn apply_mcp_oauth_url(&mut self, name: String, url: String) {
        let link = format!("[{url}]({url})");
        self.system_message(
            name,
            format!("Authorize this MCP server in your browser:\n\n{link}"),
        );
    }

    pub fn open_thinking_picker(&mut self) {
        if self.new_chat_source.provider().is_some() {
            return;
        }
        self.status_chip_activated = Some((2, Instant::now()));
        let mut options = vec![
            NeoismAgentPickerOption::new(
                "none",
                "Use model default reasoning",
                "default",
                "",
            ),
            NeoismAgentPickerOption::new("low", "Fastest reasoning", "think", "low"),
            NeoismAgentPickerOption::new(
                "medium",
                "Balanced reasoning",
                "think",
                "medium",
            ),
            NeoismAgentPickerOption::new("high", "More reasoning", "think", "high"),
            NeoismAgentPickerOption::new("xhigh", "Maximum reasoning", "think", "xhigh"),
        ];
        // GPT-5.6's ultra mode (Responses API multi-agent orchestration) —
        // only offered where the server can actually enable it.
        if self.model.contains("gpt-5.6") {
            options.push(NeoismAgentPickerOption::new(
                "ultra",
                "Multi-agent reasoning (GPT-5.6)",
                "think",
                "ultra",
            ));
        }
        let selected = options
            .iter()
            .position(|option| option.value == self.thinking.as_deref().unwrap_or(""))
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Thinking,
            "Reasoning",
            options,
            selected,
        ));
    }

    pub fn open_sessions_picker(&mut self) {
        self.session_picker_requested_cursor = None;
        self.push_outbound(OutboundAgentCommand::RefreshSessions {
            directory: self.side_panel.catalog_directory().map(str::to_owned),
            cursor: None,
        });
        let selected = self
            .session_options
            .iter()
            .position(|option| Some(option.value.as_str()) == self.session_id.as_deref())
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Session,
            "Sessions",
            self.session_picker_options_for_display(true),
            selected,
        ));
    }

    pub fn open_directory_picker(&mut self) {
        let mut options = Vec::new();
        if let Some(directory) = self.directory.as_deref() {
            let mut current = NeoismAgentPickerOption::new(
                directory,
                "Current session directory",
                "current",
                directory,
            );
            current.is_current = true;
            options.push(current);
        }
        options.push(NeoismAgentPickerOption::new(
            "~/",
            "Home directory",
            "directory",
            "~",
        ));
        let mut picker = NeoismAgentPicker::new(
            NeoismAgentPickerKind::Directory,
            "Change directory",
            options,
            0,
        );
        picker.search_placeholder = Some("Path or fuzzy directory".to_string());
        self.picker = Some(picker);
    }

    pub fn open_skill_picker(&mut self) {
        self.push_outbound(OutboundAgentCommand::RefreshSkills {
            directory: self.directory.clone(),
        });
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Skill,
            "Skills",
            self.skill_options.clone(),
            0,
        ));
    }

    pub fn open_subagent_picker(&mut self) {
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Subagent,
            "Subagents",
            self.subagent_options.clone(),
            0,
        ));
    }

    pub fn set_model_options(&mut self, options: Vec<NeoismAgentPickerOption>) {
        self.model_options = options;
        let options = self.model_picker_options();
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Model)
        {
            picker.replace_options(options);
        }
    }

    pub fn set_agent_options(&mut self, options: Vec<NeoismAgentPickerOption>) {
        self.agent_options = options;
        let options = self.agent_options.clone();
        let selected = options
            .iter()
            .position(|option| option.value == self.agent.as_deref().unwrap_or("build"));
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Agent)
        {
            picker.replace_options(options);
            if picker.query.is_empty() {
                if let Some(selected) = selected {
                    picker.selected = selected;
                }
            }
        }
    }

    pub fn set_skill_options(&mut self, options: Vec<NeoismAgentPickerOption>) {
        self.skill_options = options;
        let options = self.skill_options.clone();
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Skill)
        {
            picker.replace_options(options);
        }
    }

    /// Apply a paged all-project picker catalog. Hosts must pass continuation
    /// cursors instead of dropping picker rows after the first 50 sessions.
    pub fn set_session_options_page(
        &mut self,
        options: Vec<NeoismAgentPickerOption>,
        requested_cursor: Option<&str>,
        next_cursor: Option<String>,
    ) {
        if requested_cursor.is_some()
            && requested_cursor != self.session_picker_requested_cursor.as_deref()
        {
            return;
        }
        self.session_picker_requested_cursor = None;
        self.session_picker_next_cursor = next_cursor;
        let options = if requested_cursor.is_some() {
            let mut merged = self.session_options.clone();
            for option in options {
                if option.is_header
                    || !merged
                        .iter()
                        .any(|old| !old.is_header && old.value == option.value)
                {
                    merged.push(option);
                }
            }
            merged
        } else {
            options
        };
        self.set_session_options(options);
    }

    pub fn session_picker_page_failed(&mut self) {
        self.session_picker_requested_cursor = None;
        self.set_session_options(self.session_options.clone());
    }

    pub fn set_session_options(&mut self, options: Vec<NeoismAgentPickerOption>) {
        self.session_options = options;
        let options = self.session_picker_options_for_display(false);
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Session)
        {
            picker.replace_options(options);
        }
    }

    pub fn set_subagent_options(&mut self, options: Vec<NeoismAgentPickerOption>) {
        self.subagent_options = options;
    }

    pub fn close_picker(&mut self) {
        if self.pending_connect.is_some() {
            self.close_connect();
            return;
        }
        // The `/connect` flow is multi-stage: ESC steps back one screen (like
        // the per-screen "esc" affordance) rather than dismissing everything.
        if let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) {
            match kind {
                NeoismAgentPickerKind::ConnectSecret => {
                    if self
                        .connect
                        .as_ref()
                        .and_then(|flow| flow.provider_id())
                        .is_some()
                    {
                        self.open_connect_auth_methods();
                        return;
                    }
                    self.close_connect();
                    return;
                }
                NeoismAgentPickerKind::ConnectLabel
                | NeoismAgentPickerKind::ConnectAccountActions
                | NeoismAgentPickerKind::ConnectConfirm => {
                    if let Some((provider_id, connections)) =
                        self.connect.as_ref().and_then(|flow| {
                            flow.provider_id().map(|id| (id, flow.connections.clone()))
                        })
                    {
                        self.open_account_picker(
                            NeoismAgentPickerKind::ConnectAccount,
                            &provider_id,
                            &connections,
                        );
                        return;
                    }
                }
                NeoismAgentPickerKind::ConnectAccount => {
                    self.reopen_connect_provider_picker();
                    return;
                }
                NeoismAgentPickerKind::ModelAccount => {
                    self.close_connect();
                    self.pending_account_model = None;
                    return;
                }
                NeoismAgentPickerKind::ConnectAuth => {
                    if let Some((provider_id, connections)) =
                        self.connect.as_ref().and_then(|flow| {
                            flow.provider_id().map(|id| (id, flow.connections.clone()))
                        })
                    {
                        if let Some(flow) = self.connect.as_mut() {
                            flow.reset_account_addition();
                        }
                        self.open_account_picker(
                            NeoismAgentPickerKind::ConnectAccount,
                            &provider_id,
                            &connections,
                        );
                        return;
                    }
                    self.reopen_connect_provider_picker();
                    return;
                }
                NeoismAgentPickerKind::Connect => {
                    self.close_connect();
                    return;
                }
                NeoismAgentPickerKind::McpActions => {
                    self.open_mcp_picker();
                    return;
                }
                _ => {}
            }
        }
        self.picker = None;
        self.file_mention_anchor = None;
        if self.input == "/" {
            self.input.clear();
            self.cursor_byte = 0;
        }
    }

    pub fn move_picker_selection(&mut self, delta: isize) -> bool {
        let Some(picker) = self.picker.as_mut() else {
            return false;
        };
        picker.move_selection(delta);
        true
    }

    pub(in crate::panels::agent_pane::state) fn model_picker_options(
        &self,
    ) -> Vec<NeoismAgentPickerOption> {
        let mut options = Vec::new();
        options.push(self.current_model_picker_option(&self.model_options));
        if !self.recent_model_options.is_empty() {
            options.push(NeoismAgentPickerOption::header("Recent"));
            options.extend(self.recent_model_options.clone());
        }
        options.extend(self.model_options.clone());
        options
    }

    pub(in crate::panels::agent_pane::state) fn current_model_picker_option(
        &self,
        model_options: &[NeoismAgentPickerOption],
    ) -> NeoismAgentPickerOption {
        if self.model.trim().is_empty() {
            return NeoismAgentPickerOption::new(
                "server default",
                "Use Neoism default",
                "selected",
                "",
            );
        }
        if let Some(option) = model_options
            .iter()
            .chain(self.recent_model_options.iter())
            .find(|option| option.value == self.model && option.is_selectable())
        {
            let mut current = option.clone();
            current.is_header = false;
            if current.description.is_empty() && !current.section.is_empty() {
                current.description = current.section.clone();
            }
            current.footer = "selected".to_string();
            current.section = "Current".to_string();
            return current;
        }
        let title = self
            .model
            .split_once('/')
            .map(|(_, model)| model)
            .unwrap_or(self.model.as_str());
        let provider = self
            .model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or("");
        NeoismAgentPickerOption::new(title, provider, "selected", &self.model)
    }

    pub(in crate::panels::agent_pane::state) fn session_picker_options_for_display(
        &self,
        refreshing: bool,
    ) -> Vec<NeoismAgentPickerOption> {
        if !self.session_options.is_empty() {
            let current_id = self.session_id.as_deref();
            let mut options: Vec<_> = self
                .session_options
                .iter()
                .map(|opt| {
                    let mut o = opt.clone();
                    o.is_current = current_id.is_some_and(|id| id == opt.value);
                    o
                })
                .collect();
            if self.session_picker_next_cursor.is_some()
                && self.session_picker_requested_cursor.is_none()
            {
                options.push(NeoismAgentPickerOption::new(
                    "Load more sessions",
                    "All projects",
                    "more",
                    "__neoism_load_session_page__",
                ));
            }
            return options;
        }
        if refreshing {
            return vec![NeoismAgentPickerOption::new(
                "Loading sessions...",
                "Fetching from Neoism",
                "loading",
                "",
            )];
        }
        vec![NeoismAgentPickerOption::new(
            "No sessions",
            "No saved sessions",
            "empty",
            "",
        )]
    }

    pub fn scroll_picker_pixels(&mut self, delta_pixels: f32) -> bool {
        self.picker
            .as_mut()
            .is_some_and(|picker| picker.scroll_pixels(delta_pixels))
    }

    pub fn scroll_picker_touch_pixels(&mut self, delta_pixels: f32) -> bool {
        self.picker
            .as_mut()
            .is_some_and(|picker| picker.scroll_touch_pixels(delta_pixels))
    }

    pub fn picker_contains_point(&self, x: f32, y: f32) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|picker| picker.contains_point(x, y))
    }

    /// If the click lands on a picker row, move selection there and
    /// commit it (the conventional "single-click picks" UX). Returns true
    /// when the click was handled by the picker overlay.
    pub fn pick_at(&mut self, x: f32, y: f32) -> bool {
        if let Some(url) = self.picker.as_ref().and_then(|picker| picker.authorization_url_at(x, y)) {
            self.push_outbound(OutboundAgentCommand::OpenUrl { url });
            return true;
        }
        let Some(picker) = self.picker.as_mut() else {
            return false;
        };
        if !picker.contains_point(x, y) {
            return false;
        }
        if picker.activate_row_at(x, y) {
            // Activating a row commits the picker. Mirror the Enter
            // path so the user gets the same behaviour regardless of
            // input device.
            self.commit_picker();
            return true;
        }
        // Click was inside the popover but missed a row — absorb so the
        // tab strip / message timeline don't react.
        true
    }

    pub fn wordmark_hit(&self, x: f32, y: f32) -> bool {
        !self.has_conversation()
            && self.wordmark.rect.is_some_and(|[rx, ry, rw, rh]| {
                x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
            })
    }

    pub fn pop_wordmark_click(&mut self, x: f32, y: f32) -> bool {
        if self.has_conversation() {
            self.wordmark.animation.cancel();
            self.wordmark.rect = None;
            return false;
        }
        if !self.wordmark_hit(x, y) {
            return false;
        }
        self.wordmark.animation.replay();
        true
    }

    pub fn is_animating(&self) -> bool {
        self.animation_reason().is_some()
    }

    pub fn animation_reason(&self) -> Option<&'static str> {
        if !self.activity_status_rects.is_empty() && self.activity_animation_frame().is_some() {
            return Some("agent_activity_word");
        }
        if self
            .tool_motion
            .is_animating_for(self.session_id.as_deref())
        {
            return Some("tool_motion");
        }
        if self
            .text_reveal
            .is_animating_for(self.session_id.as_deref())
        {
            return Some("text_reveal");
        }
        if self.wordmark_click_is_animating() {
            return Some("wordmark");
        }
        if self.code_copy_feedback_is_animating() {
            return Some("code_copy_feedback");
        }
        if self
            .status_chip_activated
            .is_some_and(|(_, at)| at.elapsed().as_millis() < 280)
        {
            return Some("status_chip_activation");
        }
        if self.agent_label_changed_elapsed_ms().is_some() {
            return Some("agent_label_transition");
        }
        if self.config_chip_transition().is_some() {
            return Some("config_chip_hydration");
        }
        if !self.has_conversation() {
            return Some("agent_home_wordmark");
        }
        if self.fx_active() {
            return Some("easter_fx");
        }
        if self
            .picker
            .as_ref()
            .is_some_and(NeoismAgentPicker::is_animating)
        {
            return Some("picker");
        }
        if self.tool_expansion_is_animating() {
            return Some("tool_expansion");
        }
        if self.timeline_is_inertial() {
            return Some("timeline_inertia");
        }
        if self.visible_running_tool_active {
            return Some("running_tool");
        }
        if crate::panels::agent_pane::view::image_preview::image_decodes_pending() {
            return Some("image_decode");
        }
        if crate::syntax::highlighters_compiling() {
            return Some("syntax_compile");
        }
        if self.visible_user_orb_active() {
            return Some("user_orb");
        }
        // Interaction rects are deliberately omitted while timeline content
        // moves. Keep a bounded final repaint alive so the first settled frame
        // republishes authoritative click and wheel geometry.
        if self.timeline_interaction_settle_active() {
            return Some("timeline_interaction_settle");
        }
        // Provider time, status dots, and streamed deltas advance even across
        // transient run-idle edges. Keep draining and painting until the
        // durable execution/branch activity settles.
        if self.streaming_state != NeoismAgentStreamingState::Idle
            || self
                .execution_activity
                .as_ref()
                .is_some_and(|activity| self.execution_status_live(activity))
            || self.active_subagent_count() > 0
            || self.viewed_subagent_outstanding()
        {
            return Some("streaming");
        }
        // A transient idle gap keeps the last status label on screen
        // through the display grace hold; keep frames coming so it can
        // expire (and erase) without waiting for the next input event.
        if self.side_panel.held_status_display().is_some() {
            return Some("streaming_status_hold");
        }
        // Background task start/completion events invalidate the pane; elapsed
        // seconds alone must not keep the renderer spinning continuously.
        // Only an on-screen side panel drives the render loop. A pane
        // that has never been laid out (fresh/backgrounded) must not
        // spin redraws off its still-unloaded sessions skeleton — the
        // shimmer only needs to animate once the panel is actually
        // painted (`last_panel_rect` is stamped during render).
        if self.side_panel.last_panel_rect().is_some() && self.side_panel.is_animating() {
            return Some("side_panel");
        }
        None
    }

    pub(in crate::panels::agent_pane::state) fn wordmark_click_is_animating(
        &self,
    ) -> bool {
        !self.has_conversation() && self.wordmark.animation.is_active()
    }

    pub fn swap_detail_panel(&mut self) {
        std::mem::swap(&mut self.side_panel, &mut self.detail_panel);
    }

    pub fn prepare_detail_panel(&mut self) {
        let root = self.conversation_root_id().map(str::to_owned);
        self.detail_panel
            .sync_conversation_details_from_root(&self.side_panel, root.as_deref());
    }

    /// Owning conversation, not the exact child whose transcript is open.
    pub fn conversation_root_id(&self) -> Option<&str> {
        self.session_id.as_deref()?;
        if self.parent_session_id.is_some() {
            self.side_panel
                .subagents()
                .first()
                .map(|row| row.id.as_str())
                .or(self.parent_session_id.as_deref())
        } else {
            self.session_id.as_deref()
        }
    }

    pub fn take_catalog_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.catalog_toggle_requested)
    }

    pub fn toggle_side_panel(&mut self) {
        self.catalog_toggle_requested = true;
    }

    pub fn detail_panel(&self) -> &NeoismAgentSidePanel {
        &self.detail_panel
    }

    pub fn detail_panel_mut(&mut self) -> &mut NeoismAgentSidePanel {
        &mut self.detail_panel
    }

    pub fn side_panel(&self) -> &NeoismAgentSidePanel {
        &self.side_panel
    }

    pub fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel {
        &mut self.side_panel
    }

    pub fn session_id_str(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

impl NeoismAgentPicker {
    /// Apply authoritative data to the live picker, preserving query and selection.
    /// Shared by the native desktop and web hosts.
    pub fn apply_mcp_status(&mut self, status: &Value) {
        if !matches!(self.kind, NeoismAgentPickerKind::Mcp | NeoismAgentPickerKind::McpActions) { return; }
        if let Some(servers) = status.as_object() {
            self.mcp_icons = servers.iter().map(|(name, entry)| {
                let icon = super::super::mcp_icon::McpServiceIcon::from_entry(name, entry, self.mcp_icons.get(name));
                (name.clone(), icon)
            }).collect();
        }
        let rows = mcp_options_from_status(status);
        let options = match self.kind {
            NeoismAgentPickerKind::Mcp => rows,
            NeoismAgentPickerKind::McpActions => {
                let name = self.mcp_server.as_deref()
                    .or_else(|| self.mcp_authorization.as_ref().map(|detail| detail.name.as_str()))
                    .unwrap_or_else(|| self.title.strip_suffix(" actions").unwrap_or_default());
                rows.iter()
                    .find(|row| row.title == name)
                    .and_then(|row| serde_json::from_str::<Value>(&row.value).ok())
                    .map(|entry| mcp_action_options(&entry))
                    .unwrap_or_default()
            }
            _ => return,
        };
        self.set_loading(false);
        self.update_mcp_normal_options(options);
    }
}

pub fn mcp_options_from_status(status: &Value) -> Vec<NeoismAgentPickerOption> {
    let Some(servers) = status.as_object() else {
        return Vec::new();
    };
    servers
        .iter()
        .map(|(name, entry)| {
            let state = entry.get("status").filter(|status| status.is_object()).unwrap_or(entry);
            let oauth_capable = entry
                .get("oauthCapable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let status = state
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            let error = state
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (description, footer) = match status {
                "connected" => ("Available to Neoism".to_string(), "connected"),
                "disabled" if oauth_capable => (
                    "Click to authenticate with OAuth".to_string(),
                    "needs auth",
                ),
                "disabled" => ("Disabled in configuration".to_string(), "disabled"),
                "needs_auth" => {
                    ("Click to authenticate with OAuth".to_string(), "needs auth")
                }
                "needs_client_registration" => (
                    if error.is_empty() {
                        "Click to register and authenticate".to_string()
                    } else {
                        error.to_string()
                    },
                    "needs registration",
                ),
                "failed" if error == "MCP client runtime is not connected yet" && oauth_capable => {
                    ("OAuth configured; click to authenticate again".to_string(), "ready")
                }
                "failed" if error == "MCP client runtime is not connected yet" => {
                    ("Ready to connect".to_string(), "ready")
                }
                _ => (error.to_string(), "failed"),
            };
            let scope = mcp_scope_label(entry);
            let writable = entry.get("configWritable").and_then(Value::as_bool).unwrap_or(true);
            let description = format!("{scope}{} · {description}", if writable { "" } else { " (read-only)" });
            let value = json!({
                "name": name,
                "brand": entry.get("brand"),
                "serverInfo": entry.get("serverInfo"),
                "configScope": entry.get("configScope"),
                "enabled": entry.get("enabled").and_then(Value::as_bool).unwrap_or(status != "disabled"),
                "connected": entry.get("runtimeConnected").and_then(Value::as_bool).unwrap_or(status == "connected"),
                "oauthCapable": oauth_capable,
                "hasCredentials": entry.get("hasCredentials").and_then(Value::as_bool).unwrap_or(false),
                "configWritable": entry.get("configWritable").and_then(Value::as_bool).unwrap_or(true),
            });
            NeoismAgentPickerOption::new(name, &description, footer, &value.to_string())
        })
        .collect()
}

fn mcp_scope_label(entry: &Value) -> &'static str {
    match entry.get("configScope").and_then(Value::as_str) {
        Some("global") => "Global",
        Some("workspace") => "Workspace",
        _ => "Unknown scope",
    }
}

pub fn mcp_action_options(entry: &Value) -> Vec<NeoismAgentPickerOption> {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let enabled = entry
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let connected = entry
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let oauth = entry
        .get("oauthCapable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let credentials = entry
        .get("hasCredentials")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let writable = entry
        .get("configWritable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let action = |title: &str, description: &str, footer: &str, action: &str| {
        NeoismAgentPickerOption::new(
            title,
            description,
            footer,
            &json!({ "name": name, "action": action }).to_string(),
        )
    };
    let mut options = Vec::new();
    if writable {
        let scope = mcp_scope_label(entry);
        options.push(if enabled {
            action(
                "Disable",
                &format!("Persist enabled: false in {scope} config and stop the runtime"),
                "config",
                "disable",
            )
        } else {
            action(
                "Enable",
                &format!("Persist enabled: true in {scope} config"),
                "config",
                "enable",
            )
        });
    }
    if enabled {
        options.push(if connected {
            action(
                "Disconnect",
                "Stop this MCP runtime without disabling it",
                "runtime",
                "disconnect",
            )
        } else if oauth && !credentials {
            action(
                "Connect",
                "Authenticate with OAuth and connect this MCP",
                "OAuth",
                "authenticate",
            )
        } else {
            action(
                "Connect",
                "Start this MCP runtime now",
                "runtime",
                "connect",
            )
        });
    }
    if oauth && credentials {
        options.push(action(
            "Reauthenticate",
            "Open the MCP OAuth flow again",
            "OAuth",
            "authenticate",
        ));
    }
    if credentials {
        options.push(action(
            "Log out",
            "Remove saved OAuth credentials",
            "OAuth",
            "logout",
        ));
    }
    options
}

#[cfg(test)]
mod mcp_tests {
    use super::*;
    use crate::panels::agent_pane::state::picker::McpAuthorizationDetail;

    #[test]
    fn mcp_icons_survive_actions_authorization_refresh_and_done() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        let catalog = json!({
            "another": {"status":{"status":"connected"}},
            "webflow": {"status":{"status":"needs_auth"}, "enabled":true, "oauthCapable":true,
                "brand":"webflow", "serverInfo":{"name":"custom", "icons":[{"src":"https://example.com/icon.svg"}]}}
        });
        pane.set_mcp_status(catalog.clone());
        let picker = pane.picker.as_mut().unwrap();
        picker.move_selection(1);
        let value = picker.selected_option().unwrap().value.clone();
        assert_eq!(serde_json::from_str::<Value>(&value).unwrap()["brand"], "webflow");
        picker.apply_mcp_status(&catalog);
        assert_eq!(picker.selected_option().unwrap().title, "webflow");
        pane.open_mcp_actions(&value);
        let picker = pane.picker.as_mut().unwrap();
        assert_eq!(picker.mcp_server.as_deref(), Some("webflow"));
        assert!(picker.mcp_icons.contains_key("webflow"));
        picker.set_mcp_authorization(authorization_detail(true, "Waiting"));
        picker.apply_mcp_status(&catalog);
        assert_eq!(picker.mcp_server.as_deref(), Some("webflow"));
        assert!(picker.mcp_icons.contains_key("webflow"));
        picker.set_mcp_authorization(authorization_detail(false, "Connected"));
        assert_eq!(serde_json::from_str::<Value>(&picker.options().last().unwrap().value).unwrap()["action"], "oauth_done");
        picker.clear_mcp_authorization();
        assert_eq!(picker.mcp_server.as_deref(), Some("webflow"));
        assert!(picker.mcp_icons.contains_key("webflow"));
    }

    fn authorization_detail(busy: bool, status: &str) -> McpAuthorizationDetail {
        McpAuthorizationDetail {
            name: "webflow".into(), body: "Continue in your browser. Connection is not yet confirmed.".into(),
            status: status.into(), busy, url: Some("https://example.com/oauth?state=a)b".into()),
        }
    }

    #[test]
    fn authorization_refresh_preserves_detail_server_and_live_actions() {
        let mut picker = NeoismAgentPicker::new(NeoismAgentPickerKind::McpActions, "webflow actions", Vec::new(), 0);
        picker.apply_mcp_status(&toggle_catalog(true));
        picker.set_mcp_authorization(authorization_detail(true, "Waiting for authorization"));
        let before = picker.options().to_vec();
        picker.apply_mcp_status(&json!({
            "another-server": {"status": "connected"},
            "webflow": {"status": "disabled", "enabled": false, "oauthCapable": true}
        }));
        let detail = picker.mcp_authorization.as_ref().unwrap();
        assert_eq!(detail.name, "webflow");
        assert_eq!(detail.status, "Waiting for authorization");
        assert!(detail.busy);
        assert_eq!(picker.title, "webflow actions");
        assert_eq!(picker.options(), before);
        assert!(!picker.loading);
        assert!(picker.clear_mcp_authorization());
        assert!(!picker.clear_mcp_authorization());
        assert!(picker.options().iter().any(|row| row.title == "Enable"));
        assert!(!picker.options().iter().any(|row| row.title == "Disable"));
        assert!(picker.options().iter().all(|row| serde_json::from_str::<Value>(&row.value).unwrap()["name"] == "webflow"));
    }

    #[test]
    fn busy_authorization_links_and_back_are_actionable_and_own_redraw() {
        let mut pane = NeoismAgentPane::default();
        let mut picker = NeoismAgentPicker::new(NeoismAgentPickerKind::McpActions, "webflow actions", Vec::new(), 0);
        picker.set_mcp_authorization(authorization_detail(true, "Waiting"));
        picker.set_loading(true);
        picker.set_query("must not filter progress".into());
        assert!(!picker.loading);
        assert!(picker.query.is_empty());
        assert!(picker.is_animating());
        assert_eq!(picker.options().len(), 2);
        let link: Value = serde_json::from_str(&picker.selected_option().unwrap().value).unwrap();
        assert_eq!(link["action"], "oauth_open");
        assert_eq!(link["url"], "https://example.com/oauth?state=a)b");
        picker.move_selection(1);
        assert_eq!(serde_json::from_str::<Value>(&picker.selected_option().unwrap().value).unwrap()["action"], "oauth_back");
        pane.picker = Some(picker);
        assert!(pane.is_animating());
    }

    #[test]
    fn oauth_success_is_not_a_runtime_connection() {
        let mut picker = NeoismAgentPicker::new(NeoismAgentPickerKind::McpActions, "webflow actions", Vec::new(), 0);
        picker.set_mcp_authorization(authorization_detail(false, "Authorized · connection failed"));
        let actions = |picker: &NeoismAgentPicker| picker.options().iter().map(|row| {
            serde_json::from_str::<Value>(&row.value).unwrap()["action"].as_str().unwrap().to_owned()
        }).collect::<Vec<_>>();
        assert_eq!(actions(&picker), ["oauth_open", "authenticate", "connect", "oauth_back"]);
        picker.set_mcp_authorization(authorization_detail(false, "Unauthorized"));
        assert!(!actions(&picker).iter().any(|action| action == "connect"));
        picker.set_mcp_authorization(authorization_detail(false, "Connected"));
        assert_eq!(actions(&picker), ["oauth_open", "oauth_done"]);
        assert!(!picker.is_animating());
    }

    #[test]
    fn authorization_hit_geometry_uses_measured_header_at_every_scale() {
        use crate::widgets::inline_picker::InlinePickerRenderState;
        for s in [0.5, 1.0, 1.5, 2.0, 3.0] {
            for header in [54.0, 126.0, 234.0] {
                let mut picker = NeoismAgentPicker::new(NeoismAgentPickerKind::McpActions, "webflow actions", Vec::new(), 0);
                picker.set_mcp_authorization(authorization_detail(true, "Waiting"));
                let state = InlinePickerRenderState {
                    rect: [10.0, 20.0, 400.0 * s, (header + 92.0 + 26.0) * s],
                    header_h: header * s, footer_h: 26.0 * s,
                    first_row: 0, visible_rows: 2, selected_cursor_rect: None,
                };
                picker.set_render_geometry(state, s, 0.0);
                assert!(!picker.activate_row_at(11.0, 20.0 + header * s - s));
                for ix in 0..2 {
                    let row = state.row_rect(ix, s);
                    assert!(picker.activate_row_at(row[0] + s, row[1] + row[3] * 0.5));
                    assert_eq!(picker.selected, ix);
                }
                assert!(!picker.activate_row_at(11.0, 20.0 + (header + 92.0 + 10.0) * s));
            }
        }
    }

    #[test]
    fn invalidated_mcp_read_keeps_loading_until_follow_up() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.refresh_mcp_if_visible();
        pane.set_mcp_status(toggle_catalog(false));
        let picker = pane.picker.as_ref().unwrap();
        assert!(picker.loading);
        assert!(picker.options().is_empty());
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        pane.set_mcp_status(toggle_catalog(true));
        let picker = pane.picker.as_ref().unwrap();
        assert!(!picker.loading);
        assert_eq!(picker.options().len(), 1);
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn invalidated_mcp_read_does_not_replace_confirmed_actions() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_status(toggle_catalog(true));
        let entry = pane.picker.as_ref().unwrap().options()[0].value.clone();
        pane.open_mcp_actions(&entry);
        pane.refresh_mcp_if_visible();
        pane.drain_pending_outbound();
        pane.refresh_mcp_if_visible();
        pane.set_mcp_status(toggle_catalog(false));
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.kind, NeoismAgentPickerKind::McpActions);
        assert_eq!(picker.options()[0].title, "Disable");
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        pane.set_mcp_status(toggle_catalog(false));
        assert_eq!(pane.picker.as_ref().unwrap().options()[0].title, "Enable");
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn lost_mcp_read_expires_and_retries_once_with_coalesced_invalidation() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        pane.refresh_mcp_if_visible();
        assert!(pane.drain_pending_outbound().is_empty());
        pane.mcp_catalog_refresh.started_at = Some(
            Instant::now() - MCP_CATALOG_REQUEST_TIMEOUT - std::time::Duration::from_secs(1),
        );
        assert!(pane.has_pending_outbound());
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        assert!(!pane.mcp_catalog_refresh.invalidated);
        assert!(!pane.has_pending_outbound());
        pane.set_mcp_status(toggle_catalog(false));
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn expired_mcp_read_does_not_reopen_a_dismissed_picker() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.picker = None;
        pane.mcp_catalog_refresh.started_at = Some(
            Instant::now() - MCP_CATALOG_REQUEST_TIMEOUT - std::time::Duration::from_secs(1),
        );
        assert!(!pane.has_pending_outbound());
        assert!(pane.drain_pending_outbound().is_empty());
        assert!(pane.picker.is_none());
        // An explicit reopen recovers even if the old read never replies.
        pane.open_mcp_picker();
        assert_eq!(pane.drain_pending_outbound().len(), 1);
    }

    #[test]
    fn rejected_mcp_send_is_quiet_and_retries_after_backoff() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_status(toggle_catalog(false));
        let before = pane.picker.as_ref().unwrap().options().to_vec();
        let messages = pane.messages.len();
        pane.refresh_mcp_if_visible();
        pane.drain_pending_outbound();
        pane.refresh_mcp_if_visible();
        pane.mcp_catalog_send_failed();
        assert_eq!(pane.messages.len(), messages);
        assert_eq!(pane.picker.as_ref().unwrap().options().len(), before.len());
        assert_eq!(pane.picker.as_ref().unwrap().options()[0].value, before[0].value);
        assert!(!pane.has_pending_outbound());
        pane.mcp_catalog_refresh.completed_at = Some(
            Instant::now() - std::time::Duration::from_secs(3),
        );
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        assert!(!pane.has_pending_outbound());
        pane.picker = None;
        pane.mcp_catalog_send_failed();
        assert!(pane.picker.is_none());
    }

    #[test]
    fn expired_mcp_read_retry_keeps_directory_scope_gate() {
        let mut pane = NeoismAgentPane::default();
        pane.directory = Some("/old".into());
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.directory = Some("/new".into());
        pane.mcp_catalog_refresh.started_at = Some(
            Instant::now() - MCP_CATALOG_REQUEST_TIMEOUT - std::time::Duration::from_secs(1),
        );
        assert!(matches!(pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::RefreshMcp { directory }] if directory.as_deref() == Some("/new")
        ));
        pane.set_mcp_status_for_directory(Some("/old".into()), toggle_catalog(true));
        assert!(pane.picker.as_ref().unwrap().options().is_empty());
        assert!(pane.mcp_catalog_refresh.in_flight);
        pane.set_mcp_status_for_directory(Some("/new".into()), toggle_catalog(false));
        assert_eq!(pane.picker.as_ref().unwrap().options().len(), 1);
        assert!(!pane.has_pending_outbound());
    }

    #[test]
    fn maps_mcp_statuses_to_actionable_picker_rows() {
        let rows = mcp_options_from_status(&json!({
            "connected": { "status": { "status": "connected" }, "oauthCapable": false },
            "oauth": { "status": { "status": "needs_auth" }, "oauthCapable": true },
            "registration": { "status": { "status": "needs_client_registration", "error": "register" }, "oauthCapable": true },
            "broken": { "status": { "status": "failed", "error": "boom" }, "oauthCapable": false },
            "off": { "status": { "status": "disabled" }, "oauthCapable": false },
            "disabled_oauth": { "status": { "status": "disabled" }, "oauthCapable": true }
        }));
        let row = |name: &str| rows.iter().find(|row| row.title == name).unwrap();
        assert_eq!(row("connected").footer, "connected");
        assert_eq!(row("oauth").footer, "needs auth");
        assert_eq!(row("registration").footer, "needs registration");
        assert_eq!(row("broken").footer, "failed");
        assert_eq!(row("off").footer, "disabled");
        assert_eq!(row("disabled_oauth").footer, "needs auth");
    }

    #[test]
    fn mcp_refresh_is_single_flight_with_one_trailing_invalidation() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        for _ in 0..10 {
            pane.refresh_mcp_if_visible();
        }
        assert!(pane.drain_pending_outbound().is_empty());
        pane.set_mcp_status(toggle_catalog(false));
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        pane.set_mcp_status(toggle_catalog(true));
        assert!(pane.drain_pending_outbound().is_empty());
        assert_eq!(pane.picker.as_ref().unwrap().options()[0].footer, "ready");
    }

    #[test]
    fn mcp_visible_poll_is_bounded_and_stops_on_dismissal() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_status(toggle_catalog(false));
        assert!(!pane.has_pending_outbound());
        pane.mcp_catalog_refresh.completed_at = Some(
            Instant::now() - std::time::Duration::from_secs(3),
        );
        assert!(pane.has_pending_outbound());
        assert_eq!(pane.drain_pending_outbound().len(), 1);
        assert!(!pane.has_pending_outbound());
        pane.apply_mcp_failure(None, "temporary catalog failure".into());
        assert!(!pane.has_pending_outbound());
        pane.mcp_catalog_refresh.completed_at = None;
        pane.picker = None;
        assert!(!pane.has_pending_outbound());
    }

    #[test]
    fn mcp_late_catalog_cannot_overwrite_a_different_directory_or_picker() {
        let mut pane = NeoismAgentPane::default();
        pane.directory = Some("/old".into());
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.directory = Some("/new".into());
        pane.set_mcp_status_for_directory(Some("/old".into()), toggle_catalog(true));
        assert!(pane.picker.as_ref().unwrap().options().is_empty());
        assert!(matches!(pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::RefreshMcp { directory }] if directory.as_deref() == Some("/new")
        ));
        pane.set_mcp_status_for_directory(Some("/new".into()), toggle_catalog(false));
        let entry = pane.picker.as_ref().unwrap().options()[0].value.clone();
        pane.open_mcp_actions(&entry);
        pane.set_mcp_status_for_directory(Some("/new".into()), toggle_catalog(true));
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.kind, NeoismAgentPickerKind::McpActions);
        assert_eq!(picker.title, "webflow actions");
        assert!(picker.options().iter().any(|option| option.title == "Disable"));
        pane.picker = None;
        pane.set_mcp_status_for_directory(Some("/new".into()), toggle_catalog(false));
        assert!(pane.picker.is_none());
    }

    #[test]
    fn mcp_unset_directory_accepts_only_the_hosts_effective_request_scope() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_catalog_request_directory(Some("/workspace".into()));
        pane.set_mcp_status_for_directory(Some("/other".into()), toggle_catalog(true));
        assert!(pane.picker.as_ref().unwrap().options().is_empty());
        pane.set_mcp_status_for_directory(Some("/workspace".into()), toggle_catalog(false));
        assert_eq!(pane.picker.as_ref().unwrap().options().len(), 1);
    }

    #[test]
    fn mcp_invalidation_refreshes_only_a_visible_mcp_surface() {
        let mut pane = NeoismAgentPane::default();
        pane.directory = Some("/tmp/project".to_string());
        pane.refresh_mcp_if_visible();
        assert!(pane.drain_pending_outbound().is_empty());

        pane.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::McpActions,
            "webflow actions",
            Vec::new(),
            0,
        ));
        pane.refresh_mcp_if_visible();
        assert_eq!(
            pane.picker.as_ref().map(|picker| picker.kind),
            Some(NeoismAgentPickerKind::McpActions)
        );
        assert!(matches!(
            pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::RefreshMcp { directory }]
                if directory.as_deref() == Some("/tmp/project")
        ));
    }

    #[test]
    fn mcp_picker_exposes_owner_scope_and_read_only() {
        for scope in ["global", "workspace"] {
            for writable in [true, false] {
                let rows = mcp_options_from_status(&json!({"alpha": {
                    "status":{"status":"disabled"}, "enabled":false,
                    "configScope":scope, "configWritable":writable
                }}));
                let label = if scope == "global" {
                    "Global"
                } else {
                    "Workspace"
                };
                assert!(
                    rows[0].description.contains(label),
                    "{}",
                    rows[0].description
                );
                assert_eq!(rows[0].description.contains("read-only"), !writable);
                let mut entry: Value = serde_json::from_str(&rows[0].value).unwrap();
                for (enabled, title) in [(false, "Enable"), (true, "Disable")] {
                    entry["enabled"] = json!(enabled);
                    let actions = mcp_action_options(&entry);
                    if writable {
                        assert!(actions
                            .iter()
                            .any(|row| row.title == title
                                && row.description.contains(label)));
                    } else {
                        assert!(!actions.iter().any(|row| row.title == title));
                    }
                }
            }
        }
    }

    #[test]
    fn mcp_picker_opens_actions_then_runs_selected_action() {
        let mut pane = NeoismAgentPane::default();
        pane.directory = Some("/tmp/project".to_string());
        // The synthetic picker represents a freshly received catalog, so no
        // passive refresh should accompany the action under test.
        pane.mcp_catalog_refresh.completed_at = Some(Instant::now());
        pane.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Mcp,
            "MCP servers",
            vec![NeoismAgentPickerOption::new(
                "webflow",
                "Click to authenticate with OAuth",
                "needs auth",
                &json!({
                    "name": "webflow",
                    "enabled": false,
                    "connected": false,
                    "oauthCapable": true,
                    "hasCredentials": false,
                    "configWritable": true
                })
                .to_string(),
            )],
            0,
        ));
        assert!(pane.commit_picker());
        assert_eq!(
            pane.picker.as_ref().map(|picker| picker.kind),
            Some(NeoismAgentPickerKind::McpActions)
        );
        let actions = pane.picker.as_ref().unwrap().options();
        assert_eq!(
            actions
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Enable"]
        );
        assert!(pane.commit_picker());
        let commands = pane.drain_pending_outbound();
        assert!(matches!(
            commands.as_slice(),
            [OutboundAgentCommand::McpSetEnabled { name, enabled: true, directory }]
                if name == "webflow" && directory.as_deref() == Some("/tmp/project")
        ), "{commands:?}");
    }

    fn toggle_catalog(enabled: bool) -> Value {
        json!({"webflow": {
            "enabled": enabled,
            "runtimeConnected": false,
            "configWritable": true,
            "status": {"status": if enabled { "failed" } else { "disabled" },
                "error": "MCP client runtime is not connected yet"}
        }})
    }

    #[test]
    fn mcp_toggle_reply_refreshes_current_actions_in_both_directions() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_status(toggle_catalog(false));
        pane.commit_picker();
        for enabled in [true, false] {
            assert!(pane.commit_picker());
            assert!(matches!(pane.drain_pending_outbound().as_slice(),
                [OutboundAgentCommand::McpSetEnabled { enabled: actual, .. }] if *actual == enabled));
            let picker = pane.picker.as_ref().unwrap();
            assert_eq!(picker.kind, NeoismAgentPickerKind::McpActions);
            assert!(picker.loading);
            // A repeated Enter cannot race another toggle against the pending request.
            pane.commit_picker();
            assert!(pane.drain_pending_outbound().is_empty());
            pane.refresh_mcp_if_visible();
            assert!(matches!(
                pane.drain_pending_outbound().as_slice(),
                [OutboundAgentCommand::RefreshMcp { .. }]
            ));
            pane.set_mcp_status(toggle_catalog(enabled));
            let picker = pane.picker.as_ref().unwrap();
            assert!(!picker.loading);
            assert_eq!(picker.selected, 0);
            assert_eq!(
                picker.selected_option().unwrap().title,
                if enabled { "Disable" } else { "Enable" }
            );
        }
    }

    #[test]
    fn mcp_failed_toggle_keeps_confirmed_state_and_allows_retry() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_status(toggle_catalog(false));
        pane.commit_picker();
        pane.commit_picker();
        pane.drain_pending_outbound();
        pane.apply_mcp_failure(Some("webflow".into()), "read-only config".into());
        let picker = pane.picker.as_ref().unwrap();
        assert!(!picker.loading);
        assert_eq!(picker.selected_option().unwrap().title, "Enable");
        pane.commit_picker();
        assert!(matches!(
            pane.drain_pending_outbound().as_slice(),
            [OutboundAgentCommand::McpSetEnabled { enabled: true, .. }]
        ));
    }

    #[test]
    fn mcp_catalog_refresh_preserves_name_query_and_dismissal() {
        let mut pane = NeoismAgentPane::default();
        pane.open_mcp_picker();
        pane.drain_pending_outbound();
        pane.set_mcp_status(toggle_catalog(false));
        pane.picker.as_mut().unwrap().set_query("web".into());
        let mut status = toggle_catalog(true);
        status["a-web-server"] = status["webflow"].clone();
        pane.refresh_mcp_if_visible();
        pane.drain_pending_outbound();
        pane.set_mcp_status(status);
        let picker = pane.picker.as_ref().unwrap();
        assert_eq!(picker.query, "web");
        assert_eq!(picker.selected_option().unwrap().title, "webflow");
        assert_eq!(picker.selected_option().unwrap().footer, "ready");
        pane.picker = None;
        pane.set_mcp_status(toggle_catalog(false));
        pane.refresh_mcp_if_visible();
        assert!(pane.picker.is_none());
        assert!(pane.drain_pending_outbound().is_empty());
    }

    #[test]
    fn oauth_connect_starts_authentication_until_credentials_exist() {
        let actions = mcp_action_options(&json!({
            "name": "webflow",
            "enabled": true,
            "connected": false,
            "oauthCapable": true,
            "hasCredentials": false,
            "configWritable": true
        }));
        let connect = actions.iter().find(|row| row.title == "Connect").unwrap();
        let value: Value = serde_json::from_str(&connect.value).unwrap();
        assert_eq!(value["action"], "authenticate");
        assert_eq!(connect.footer, "OAuth");
    }
}
