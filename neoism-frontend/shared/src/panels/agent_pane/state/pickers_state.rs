use super::*;

impl NeoismAgentPane {
    pub fn open_agent_picker(&mut self) {
        self.push_outbound(OutboundAgentCommand::RefreshAgents {
            directory: self.directory.clone(),
        });
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Agent,
            "Agents",
            self.agent_options.clone(),
            0,
        ));
    }

    pub fn open_model_picker(&mut self) {
        self.push_outbound(OutboundAgentCommand::RefreshModels);
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
    }

    pub fn open_mcp_picker(&mut self) {
        self.push_outbound(OutboundAgentCommand::RefreshMcp {
            directory: self.directory.clone(),
        });
        let mut picker = NeoismAgentPicker::new(
            NeoismAgentPickerKind::Mcp,
            "MCP servers",
            Vec::new(),
            0,
        );
        picker.loading = true;
        self.picker = Some(picker);
    }

    pub fn set_mcp_status(&mut self, status: Value) {
        let options = mcp_options_from_status(&status);
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Mcp)
        {
            picker.loading = false;
            picker.replace_options(options);
        }
    }

    pub fn open_mcp_actions(&mut self, value: &str) {
        let Ok(entry) = serde_json::from_str::<Value>(value) else {
            return;
        };
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("MCP");
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::McpActions,
            &format!("{name} actions"),
            mcp_action_options(&entry),
            0,
        ));
    }

    pub fn apply_mcp_oauth_url(&mut self, name: String, url: String) {
        let link = format!("[{url}]({url})");
        self.system_message(
            name,
            format!("Authorize this MCP server in your browser:\n\n{link}"),
        );
    }

    pub fn open_thinking_picker(&mut self) {
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
        self.push_outbound(OutboundAgentCommand::RefreshSessions {
            directory: self.directory.clone(),
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
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Agent)
        {
            picker.replace_options(options);
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
        // The `/connect` flow is multi-stage: ESC steps back one screen (like
        // the per-screen "esc" affordance) rather than dismissing everything.
        if let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) {
            match kind {
                NeoismAgentPickerKind::ConnectSecret => {
                    if let Some(provider_id) =
                        self.connect.as_ref().and_then(|flow| flow.provider_id())
                    {
                        self.enter_connect_auth(&provider_id);
                        return;
                    }
                    self.close_connect();
                    return;
                }
                NeoismAgentPickerKind::ConnectAuth => {
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

    pub fn toggle_side_panel(&mut self) {
        self.side_panel.toggle_visibility();
        if !self.side_panel.user_hidden() {
            self.push_outbound(OutboundAgentCommand::RefreshSessions {
                directory: self.directory.clone(),
            });
        }
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
            return self
                .session_options
                .iter()
                .map(|opt| {
                    let mut o = opt.clone();
                    o.is_current = current_id.is_some_and(|id| id == opt.value);
                    o
                })
                .collect();
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
            "No saved sessions for this workspace",
            "empty",
            "",
        )]
    }

    pub fn scroll_picker_pixels(&mut self, delta_pixels: f32) -> bool {
        self.picker
            .as_mut()
            .is_some_and(|picker| picker.scroll_pixels(delta_pixels))
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

    pub fn pop_wordmark_click(&mut self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.wordmark.rect else {
            return false;
        };
        if x < rx || x > rx + rw || y < ry || y > ry + rh {
            return false;
        }
        self.wordmark.click_started = Some(Instant::now());
        self.wordmark.click_pos = Some((x, y));
        true
    }

    pub fn is_animating(&self) -> bool {
        self.animation_reason().is_some()
    }

    pub fn animation_reason(&self) -> Option<&'static str> {
        if self.wordmark_click_is_animating() {
            return Some("wordmark");
        }
        if self.code_copy_feedback_is_animating() {
            return Some("code_copy_feedback");
        }
        if !self.has_conversation() {
            return Some("agent_home_wordmark");
        }
        if self.visible_user_orb {
            return Some("visible_user_orb");
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
        // The derived display state includes background tasks. Those update
        // through events and must not own the continuous redraw loop.
        if self.streaming_state != NeoismAgentStreamingState::Idle {
            return Some("streaming");
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

    pub(crate) fn begin_visible_animation_frame(&mut self) {
        self.visible_user_orb = false;
    }

    pub(crate) fn mark_visible_user_orb(&mut self) {
        self.visible_user_orb = true;
    }

    pub(in crate::panels::agent_pane::state) fn wordmark_click_is_animating(
        &self,
    ) -> bool {
        self.wordmark.click_started.is_some_and(|started| {
            Instant::now().saturating_duration_since(started) <= WORDMARK_CLICK_ANIMATION
        })
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
            let value = json!({
                "name": name,
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
        options.push(if enabled {
            action(
                "Disable",
                "Persist enabled: false and stop the runtime",
                "config",
                "disable",
            )
        } else {
            action(
                "Enable",
                "Persist enabled: true in the owning config",
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
    fn mcp_picker_opens_actions_then_runs_selected_action() {
        let mut pane = NeoismAgentPane::default();
        pane.directory = Some("/tmp/project".to_string());
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
        ));
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
