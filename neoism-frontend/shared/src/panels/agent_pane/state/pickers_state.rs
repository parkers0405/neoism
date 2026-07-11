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
                "Use Neoism Agent default",
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
                "Fetching from Neoism Agent",
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
        if self.is_streaming() {
            return Some("streaming");
        }
        if self.active_subagent_count() > 0 {
            return Some("subagents");
        }
        if self.running_background_task_count() > 0 {
            return Some("background_tasks");
        }
        if self.side_panel.is_animating() {
            return Some("side_panel");
        }
        None
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
