use super::*;

#[wasm_bindgen]
impl ChromeBridge {
    // -------- agent pane ----------------------------------------
    //
    // The web frontend has no host-side agent process — the
    // workspace daemon proxies the Neoism Agent vocabulary across
    // its WebSocket. The bridge owns the composer / timeline /
    // permission state behind these methods; JS reads them per
    // frame to paint an `AgentPane`-equivalent and pushes inbound
    // `AgentServerMessage`s in via `agent_event`. Outbound
    // `AgentClientMessage`s flow through the JS callback installed
    // by `set_agent_send`.

    /// Toggle the agent UI surface. Web frontend uses this from
    /// the command palette / status-line shortcut to flip between
    /// "show agent pane" and "hide agent pane" with the same
    /// semantics as `toggle_file_tree` (open + focus on first
    /// press, hide when already focused, focus when visible-but-
    /// unfocused). The shared chrome doesn't yet host a dedicated
    /// agent panel, so we use the existing "open neoism agent
    /// buffer tab" path — JS picks up the queued open via
    /// `drain_agent_tab_opens` and switches its bookkeeping.
    pub fn toggle_agent_pane(&mut self) {
        self.queue_agent_tab_open();
    }

    /// Ingest one inbound `AgentServerMessage` envelope. The JSON
    /// shape is the externally-tagged variant set defined in
    /// `neoism_protocol::agent`.
    ///
    /// Each variant is dispatched directly to a matching method on
    /// the shared `NeoismAgentPane`, mirroring the desktop pane's
    /// `drain_server_updates` arm-by-arm so the web and desktop
    /// paint the same data.
    ///
    /// Returns `Ok(())` on success or `Err(JsValue)` carrying the
    /// parse error so the host can log + recover.
    pub fn agent_event(&mut self, event_json: &str) -> Result<(), JsValue> {
        use neoism_protocol::agent::AgentServerMessage;

        let parsed: AgentServerMessage = serde_json::from_str(event_json)
            .map_err(|e| JsValue::from_str(&format!("agent_event parse: {e}")))?;

        if !self.should_apply_agent_event(&parsed) {
            return Ok(());
        }

        // Mirror a tiny bit of state on the bridge for the
        // JS-callable getters (`agent_session_id`, `agent_is_streaming`,
        // `agent_has_pending_permission`) before handing off to the
        // pane. Done up-front so both paths stay consistent even
        // when the pane isn't installed yet.
        self.mirror_agent_event_to_bridge(&parsed);

        if let Some(pane) = self.chrome.agent_pane_mut() {
            apply_agent_event_to_pane(pane, parsed);
        }
        self.flush_pending_agent_prompt();
        Ok(())
    }

    pub(crate) fn should_apply_agent_event(
        &self,
        parsed: &neoism_protocol::agent::AgentServerMessage,
    ) -> bool {
        use neoism_protocol::agent::AgentServerMessage as M;
        let Some(event_session_id) = agent_event_session_id(parsed) else {
            return true;
        };
        if self.agent_state.thread_create_inflight
            || self.agent_state.suppress_stale_session_events
        {
            // A fresh thread is being created (or the user just
            // reset to a fresh chat) and the local session id is
            // already cleared. Without this gate, streaming events
            // from the PREVIOUS session (still live on the daemon)
            // sail through the `None == anything` fallback below
            // and repaint the conversation we just reset.
            return matches!(
                parsed,
                M::ThreadCreated { .. }
                    | M::ThreadSwitched { .. }
                    | M::HistoryChunk { .. }
                    | M::ThreadDeleted { .. }
            );
        }
        match self.agent_state.requested_session_id.as_deref() {
            Some(requested) => event_session_id == requested,
            None => self
                .agent_state
                .session_id
                .as_deref()
                .map(|active| active == event_session_id)
                .unwrap_or(true),
        }
    }

    /// Update the bridge-side scratch state (`agent_state`) for the
    /// handful of variants that gate JS-side callbacks. The pane
    /// is the source of truth for everything user-visible; this
    /// only feeds the legacy `agent_*` getters.
    pub(crate) fn mirror_agent_event_to_bridge(
        &mut self,
        parsed: &neoism_protocol::agent::AgentServerMessage,
    ) {
        use neoism_protocol::agent::AgentServerMessage;
        match parsed {
            AgentServerMessage::MessageStart { .. } => {
                self.agent_state.streaming = true;
            }
            AgentServerMessage::MessageEnd { .. }
            | AgentServerMessage::SessionIdle { .. } => {
                self.agent_state.streaming = false;
            }
            AgentServerMessage::Disabled { .. } => {
                self.agent_state.streaming = false;
                self.agent_state.thread_create_inflight = false;
            }
            AgentServerMessage::PermissionRequest { request_id, .. } => {
                self.agent_state.pending_permission = Some(AgentPendingPermission {
                    legacy_request_id: Some(*request_id),
                    tool_request_id: None,
                    session_id: self.agent_state.session_id.clone(),
                    selection: 0,
                });
            }
            AgentServerMessage::ToolUseRequest {
                request_id,
                session_id,
                ..
            } => {
                self.agent_state.pending_permission = Some(AgentPendingPermission {
                    legacy_request_id: None,
                    tool_request_id: Some(request_id.clone()),
                    session_id: Some(session_id.clone()),
                    selection: 0,
                });
            }
            AgentServerMessage::ToolUseResult { tool_use_id, .. } => {
                if let Some(perm) = self.agent_state.pending_permission.as_ref() {
                    if perm.tool_request_id.as_deref() == Some(tool_use_id.as_str()) {
                        self.agent_state.pending_permission = None;
                    }
                }
            }
            AgentServerMessage::ThreadCreated { session_id, .. }
            | AgentServerMessage::ThreadSwitched { session_id }
            | AgentServerMessage::HistoryChunk { session_id, .. } => {
                self.agent_state.thread_create_inflight = false;
                self.agent_state.suppress_stale_session_events = false;
                self.agent_state.session_id = Some(session_id.clone());
                if self.agent_state.requested_session_id.as_deref()
                    == Some(session_id.as_str())
                {
                    self.agent_state.requested_session_id = None;
                }
            }
            AgentServerMessage::Error { .. } => {
                // A failed CreateThread must not wedge auto-create
                // forever; the next prompt retries.
                self.agent_state.thread_create_inflight = false;
            }
            AgentServerMessage::ThreadDeleted { session_id } => {
                if self.agent_state.session_id.as_deref() == Some(session_id.as_str()) {
                    self.agent_state.session_id = None;
                }
            }
            AgentServerMessage::ConfigDefaults {
                agent,
                model,
                thinking,
            } => {
                self.agent_state.default_agent = agent.clone();
                self.agent_state.default_model = model.clone();
                self.agent_state.default_thinking = thinking.clone();
            }
            // Fan agent-level Notice events into the chrome's
            // global toast stack. The agent pane already stores
            // its own per-session notice list (via
            // `pane.push_notice_event`, called downstream in
            // `apply_agent_event_to_pane`); the global stack
            // mirrors them so the user sees the toast regardless
            // of which tab is focused. We render `title — body` so
            // the toast carries both fields the daemon emits.
            AgentServerMessage::Notice {
                title, body, level, ..
            } => {
                use neoism_protocol::agent::NoticeLevel;
                use neoism_ui::panels::notifications::NotificationLevel;
                let panel_level = match level {
                    NoticeLevel::Error => NotificationLevel::Error,
                    NoticeLevel::Warn => NotificationLevel::Warn,
                    NoticeLevel::Info => NotificationLevel::Info,
                };
                let message = if title.is_empty() {
                    body.clone()
                } else if body.is_empty() {
                    title.clone()
                } else {
                    format!("{title} — {body}")
                };
                self.chrome.notifications.push(message, panel_level);
            }
            _ => {}
        }
    }

    pub(crate) fn flush_pending_agent_prompt(&mut self) {
        use neoism_protocol::agent::AgentClientMessage;

        let Some(session_id) = self.agent_state.session_id.clone() else {
            return;
        };
        let Some(prompt) = self.agent_state.pending_prompt.take() else {
            return;
        };
        self.send_agent_envelope(&AgentClientMessage::SubmitPrompt {
            session_id,
            text: prompt.text,
            attachments: prompt.attachments,
            mode: prompt.mode,
            model: prompt.model,
            thinking: prompt.thinking,
        });
    }

    pub(crate) fn agent_prompt_defaults(
        &self,
    ) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.agent_state.default_agent.clone(),
            self.agent_state.default_model.clone(),
            self.agent_state.default_thinking.clone(),
        )
    }

    pub(crate) fn create_agent_thread_with_defaults(&mut self) {
        use neoism_protocol::agent::AgentClientMessage;

        // Single-flight: EnsureSession and the pending-prompt arm
        // can both land in one drain before `ThreadCreated` has a
        // chance to stamp a session id.
        if self.agent_state.thread_create_inflight {
            return;
        }
        self.agent_state.thread_create_inflight = true;
        self.send_agent_envelope(&AgentClientMessage::CreateThread {
            title: None,
            directory: self.agent_state.default_directory.clone(),
            agent: self.agent_state.default_agent.clone(),
            model: self.agent_state.default_model.clone(),
        });
    }

    /// Set the composer input text. JS pushes this on every
    /// keystroke; `clear_terminal_input`-equivalent.
    pub fn agent_set_input(&mut self, text: &str) {
        self.agent_state.input = text.to_string();
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.replace_input(text);
        }
        // Stepping into a freshly-edited input drops any stashed
        // live draft so `agent_history_step` starts fresh from the
        // bottom on the next press.
        self.agent_state.history_cursor = None;
        self.agent_state.history_pending_live = None;
    }

    /// Current composer input.
    pub fn agent_input(&self) -> String {
        self.agent_state.input.clone()
    }

    /// Clear the composer input.
    pub fn agent_clear_input(&mut self) {
        self.agent_state.input.clear();
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.clear_or_abort();
        }
        self.agent_state.history_cursor = None;
        self.agent_state.history_pending_live = None;
    }

    /// Route one browser key event through the shared desktop
    /// Neoism Agent key policy and pane state. This keeps web from
    /// inventing separate slash-picker, history, and submit rules.
    pub fn agent_handle_key(
        &mut self,
        key: &str,
        code: &str,
        text: &str,
        shift: bool,
        control: bool,
        alt: bool,
        super_key: bool,
    ) -> bool {
        use neoism_ui::panels::agent_pane::bridge_policy::{
            agent_key_decision, AgentBridgeElementState, AgentBridgeKeyEvent,
            AgentBridgeModifiers, AgentKeyContext, AgentKeyIntent, AgentPermissionReply,
        };
        use neoism_ui::panels::agent_pane::state::NeoismAgentPermissionChoice;

        let mods = AgentBridgeModifiers {
            shift,
            control,
            alt,
            super_key,
        };
        let logical_key =
            agent_bridge_key_from_web(if key.is_empty() { text } else { key });
        let event = AgentBridgeKeyEvent {
            state: AgentBridgeElementState::Pressed,
            logical_key,
            key_without_modifiers: agent_bridge_key_from_web(if text.is_empty() {
                key
            } else {
                text
            }),
            physical_key: agent_bridge_physical_key_from_web(code),
            text: text.to_string(),
        };
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        let ctx = AgentKeyContext {
            side_panel_focused: pane.side_panel().is_focused(),
            pending_permission: pane.pending_permission().is_some(),
            picker_open: pane.picker().is_some(),
        };
        let decision = agent_key_decision(&event, mods, ctx);
        if !decision.handled {
            return false;
        }
        if decision
            .intents
            .iter()
            .any(|intent| matches!(intent, AgentKeyIntent::Paste))
        {
            return false;
        }

        for intent in decision.intents {
            match intent {
                AgentKeyIntent::Backspace => pane.backspace(),
                AgentKeyIntent::ClearOrAbort => pane.clear_or_abort(),
                AgentKeyIntent::ClosePicker => pane.close_picker(),
                AgentKeyIntent::InsertNewline => pane.insert_newline(),
                AgentKeyIntent::InsertText(value) => pane.insert_text(&value),
                AgentKeyIntent::MoveInputDownOrHistory => {
                    pane.move_input_down_or_history()
                }
                AgentKeyIntent::MoveInputEnd => pane.move_input_end(),
                AgentKeyIntent::MoveInputHome => pane.move_input_home(),
                AgentKeyIntent::MoveInputLeft => pane.move_input_left(),
                AgentKeyIntent::MoveInputRight => pane.move_input_right(),
                AgentKeyIntent::MoveInputUpOrHistory => pane.move_input_up_or_history(),
                AgentKeyIntent::MovePermissionSelection(delta) => {
                    let _ = pane.move_permission_selection(delta);
                }
                AgentKeyIntent::MovePickerSelection(delta) => {
                    let _ = pane.move_picker_selection(delta);
                }
                AgentKeyIntent::RespondPendingPermission(reply) => {
                    let choice = match reply {
                        AgentPermissionReply::Once => NeoismAgentPermissionChoice::Once,
                        AgentPermissionReply::Always => {
                            NeoismAgentPermissionChoice::Always
                        }
                        AgentPermissionReply::Reject => {
                            NeoismAgentPermissionChoice::Reject
                        }
                    };
                    let _ = pane.respond_pending_permission(choice);
                }
                AgentKeyIntent::ScrollTimelineHalfPageDown => {
                    pane.scroll_timeline_half_page(false);
                }
                AgentKeyIntent::ScrollTimelineHalfPageUp => {
                    pane.scroll_timeline_half_page(true);
                }
                AgentKeyIntent::SidePanelActivateSelection => {
                    let _ = pane.activate_side_panel_selection()
                        || pane.activate_side_panel_subagent();
                }
                AgentKeyIntent::SidePanelBlur => {
                    pane.side_panel_mut().set_focused(false);
                }
                AgentKeyIntent::SidePanelSelectNext => {
                    pane.side_panel_mut().select_next();
                }
                AgentKeyIntent::SidePanelSelectPrev => {
                    pane.side_panel_mut().select_prev();
                }
                AgentKeyIntent::Submit => {
                    let _ = pane.submit();
                }
                AgentKeyIntent::SubmitPendingPermission => {
                    let _ = pane.submit_pending_permission();
                }
                AgentKeyIntent::ToggleMode => pane.toggle_mode(),
                AgentKeyIntent::ToggleSidePanel => {
                    pane.toggle_side_panel();
                }
                AgentKeyIntent::Paste => {}
            }
        }
        self.agent_state.input = pane.input().to_string();
        self.agent_state.history_cursor = None;
        self.agent_state.history_pending_live = None;
        let _ = self.drain_agent_outbound();
        true
    }

    /// Step through input history. `delta < 0` walks back in time
    /// (older entries); `delta > 0` walks forward toward the live
    /// edit. Returns the resulting input text so JS can mirror it
    /// into its DOM composer in one step.
    pub fn agent_history_step(&mut self, delta: i32) -> String {
        if self.agent_state.history.is_empty() || delta == 0 {
            return self.agent_state.input.clone();
        }
        let len = self.agent_state.history.len();
        let cursor = match self.agent_state.history_cursor {
            Some(c) => c as i32,
            None => {
                // Stash whatever's in the composer so stepping
                // forward off the end can restore it.
                self.agent_state.history_pending_live =
                    Some(self.agent_state.input.clone());
                len as i32
            }
        };
        let next = (cursor + delta).clamp(0, len as i32);
        if next == len as i32 {
            // Past the newest entry — back to the live draft.
            self.agent_state.history_cursor = None;
            if let Some(live) = self.agent_state.history_pending_live.take() {
                self.agent_state.input = live;
            }
        } else {
            let idx = next.max(0) as usize;
            self.agent_state.history_cursor = Some(idx);
            self.agent_state.input = self.agent_state.history[idx].clone();
        }
        self.agent_state.input.clone()
    }

    /// Rect of the agent pane's prompt input in chrome-logical
    /// pixels (`[x, y, w, h]` as a JS array), or `null` when no
    /// agent pane is installed. Mirrors the view's own layout
    /// (side-panel carve + home vs chat placement) so the mobile
    /// tap-to-summon-keyboard hit-test lands on the real input box
    /// — the home screen centers it mid-pane, not in the bottom
    /// band the conversation view docks to.
    pub fn agent_input_rect_json(&mut self) -> JsValue {
        use neoism_ui::panels::agent_pane::view::{layout as agent_layout, side_panel};

        let terminal_rect = self.chrome.layout().terminal;
        let scale = self.chrome.chrome_scale().clamp(0.5, 3.0);
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return JsValue::NULL;
        };
        let pane_rect = [
            terminal_rect.x,
            terminal_rect.y,
            terminal_rect.w,
            terminal_rect.h,
        ];
        let main_rect = match side_panel::carve_panel_rect(pane, pane_rect, scale) {
            Some((main, _panel)) => main,
            None => pane_rect,
        };
        let input = if pane.has_conversation() {
            agent_layout::chat_input_rect(pane, main_rect, scale)
        } else {
            agent_layout::home_input_rect(pane, main_rect, scale)
        };
        serde_wasm_bindgen::to_value(&input).unwrap_or(JsValue::NULL)
    }

    /// Scroll the agent timeline by `delta_pixels`. Returns `true`
    /// if the shared pane moved, so the host can request a redraw.
    pub fn agent_scroll_timeline(&mut self, delta_pixels: f32) -> bool {
        self.chrome
            .agent_pane_mut()
            .map(|pane| pane.scroll_timeline_pixels(delta_pixels))
            .unwrap_or(false)
    }

    /// 1:1 touch drag on the agent timeline — no velocity
    /// injection, the content tracks the finger exactly. Pair with
    /// `agent_fling_timeline` on touch release.
    pub fn agent_drag_timeline(&mut self, delta_pixels: f32) -> bool {
        self.chrome
            .agent_pane_mut()
            .map(|pane| pane.drag_timeline_pixels(delta_pixels))
            .unwrap_or(false)
    }

    /// Launch (non-zero) or stop (zero) a kinetic glide on the
    /// agent timeline. Returns `true` if the timeline was gliding
    /// before the call so the host can swallow glide-stopping taps.
    pub fn agent_fling_timeline(&mut self, velocity_px_s: f32) -> bool {
        self.chrome
            .agent_pane_mut()
            .map(|pane| pane.fling_timeline(velocity_px_s))
            .unwrap_or(false)
    }

    /// True when the agent pane is showing a conversation (vs the
    /// home screen). Hosts use this to decide whether re-invoking
    /// "Neoism Agent" should spin up a fresh thread.
    pub fn agent_has_conversation(&self) -> bool {
        self.chrome
            .agent_pane()
            .is_some_and(|pane| pane.has_conversation())
    }

    /// True if a tool / permission request is awaiting the user's
    /// decision. JS uses this to gate the permission-picker UI.
    pub fn agent_has_pending_permission(&self) -> bool {
        self.agent_state.pending_permission.is_some()
    }

    /// True while a daemon-side turn is in flight (between
    /// `MessageStart` and `MessageEnd`).
    pub fn agent_is_streaming(&self) -> bool {
        self.agent_state.streaming
    }

    /// Move the permission-picker selection by `delta`. The
    /// picker has three slots — Yes / Always / No — so the
    /// selection wraps modulo 3. Returns `true` if a permission
    /// was actually pending (so JS can short-circuit redraws when
    /// the keystroke went nowhere).
    pub fn agent_move_permission_selection(&mut self, delta: i32) -> bool {
        let Some(perm) = self.agent_state.pending_permission.as_mut() else {
            return false;
        };
        let next = perm.selection.rem_euclid(3) + delta;
        perm.selection = next.rem_euclid(3);
        true
    }

    /// Submit the currently-highlighted permission choice. Maps
    /// the picker's selection index (0 / 1 / 2) to
    /// `Yes` / `Always` / `No` and routes through the same path
    /// as `agent_reply_permission`. Returns `true` when a pending
    /// permission existed (and a callback fired); `false`
    /// otherwise.
    pub fn agent_submit_permission(&mut self) -> bool {
        let Some(selection) = self
            .agent_state
            .pending_permission
            .as_ref()
            .map(|p| p.selection.rem_euclid(3))
        else {
            return false;
        };
        let decision = match selection {
            0 => "Yes",
            1 => "Always",
            _ => "No",
        };
        self.agent_reply_permission(decision)
    }

    /// Reply to the pending permission request with `decision`.
    /// Accepts the wire spelling (`"Yes" | "Always" | "No"`) plus
    /// a handful of friendlier aliases (`"approve"`, `"deny"`,
    /// `"approve_once"`, `"deny_session"`, `"always"`,
    /// `"reject"`). Unknown values fall back to `No` so a typo on
    /// the JS side doesn't accidentally green-light a tool. Fires
    /// `ApproveTool` / `DenyTool` when the request came from the
    /// agent-server (string id present) and falls back to
    /// `ReplyPermission` otherwise. Returns `true` when an
    /// envelope was actually sent.
    pub fn agent_reply_permission(&mut self, decision: &str) -> bool {
        use neoism_protocol::agent::{AgentClientMessage, PermissionDecision};

        let Some(pending) = self.agent_state.pending_permission.take() else {
            return false;
        };
        let normalized = decision.trim().to_ascii_lowercase();
        let mapped = match normalized.as_str() {
            "yes" | "approve" | "approve_once" => PermissionDecision::Yes,
            "always" | "approve_always" => PermissionDecision::Always,
            _ => PermissionDecision::No,
        };
        let envelope = if let (Some(req_id), Some(session_id)) =
            (pending.tool_request_id.clone(), pending.session_id.clone())
        {
            if matches!(mapped, PermissionDecision::No) {
                AgentClientMessage::DenyTool {
                    request_id: req_id,
                    session_id,
                }
            } else {
                AgentClientMessage::ApproveTool {
                    request_id: req_id,
                    session_id,
                    decision: mapped,
                }
            }
        } else if let Some(req_id) = pending.legacy_request_id {
            AgentClientMessage::ReplyPermission {
                request_id: req_id,
                decision: mapped,
            }
        } else {
            // Pending was malformed (no id of either flavour). We
            // already consumed it via `.take()` — nothing else to
            // do.
            return false;
        };
        self.send_agent_envelope(&envelope)
    }

    /// Install the JS callback the bridge fires to forward
    /// outbound `AgentClientMessage` envelopes. Signature:
    /// `(request_id: number, envelope_json: string) => void`. The
    /// JS host wraps the envelope in a `ServiceClientMessage`
    /// frame and ships it to the daemon over the existing
    /// WebSocket.
    pub fn set_agent_send(&mut self, cb: js_sys::Function) {
        self.agent_state.send_cb = Some(cb);
    }

    /// Convenience: build a `SubmitPrompt` (when a session is
    /// active) or create a session and queue the prompt. This goes
    /// through the shared agent pane submit path so slash commands,
    /// picker commits, config defaults, and runtime side effects
    /// match desktop.
    pub fn agent_send_message(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.agent_state.history.last().map(String::as_str) != Some(trimmed) {
            self.agent_state.history.push(trimmed.to_string());
        }
        if let Some(pane) = self.chrome.agent_pane_mut() {
            if pane.input() != text {
                pane.replace_input(text);
            }
            let _ = pane.submit();
            self.agent_state.input = pane.input().to_string();
            self.agent_state.history_cursor = None;
            self.agent_state.history_pending_live = None;
            let _ = self.drain_agent_outbound();
            return;
        }

        let (mode, model, thinking) = self.agent_prompt_defaults();
        self.agent_state.input.clear();
        self.agent_state.history_cursor = None;
        self.agent_state.history_pending_live = None;
        self.agent_state.pending_prompt = Some(PendingAgentPrompt {
            text: trimmed.to_string(),
            attachments: Vec::new(),
            mode,
            model,
            thinking,
        });
        self.create_agent_thread_with_defaults();
    }

    /// Same submit path as `agent_send_message`, but with
    /// structured attachments supplied by the JS host. Used for
    /// clipboard image paste: JS reads `ClipboardEvent` files,
    /// serializes them as protocol `Attachment` records, and the
    /// bridge stamps the current agent session id before emitting
    /// the outbound envelope.
    pub fn agent_send_message_with_attachments(
        &mut self,
        text: &str,
        attachments_json: &str,
    ) -> Result<(), JsValue> {
        use neoism_protocol::agent::{AgentClientMessage, Attachment};

        let attachments: Vec<Attachment> = serde_json::from_str(attachments_json)
            .map_err(|e| JsValue::from_str(&format!("agent attachments parse: {e}")))?;
        let trimmed = text.trim();
        if trimmed.is_empty() && attachments.is_empty() {
            return Ok(());
        }

        if !trimmed.is_empty()
            && self.agent_state.history.last().map(String::as_str) != Some(trimmed)
        {
            self.agent_state.history.push(trimmed.to_string());
        }
        self.agent_state.input.clear();
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.replace_input("");
        }
        self.agent_state.history_cursor = None;
        self.agent_state.history_pending_live = None;

        let text = if trimmed.is_empty() {
            "Please analyze the pasted image.".to_string()
        } else {
            trimmed.to_string()
        };
        let (mode, model, thinking) = self.agent_prompt_defaults();
        if let Some(session_id) = self.agent_state.session_id.clone() {
            self.send_agent_envelope(&AgentClientMessage::SubmitPrompt {
                session_id,
                text,
                attachments,
                mode,
                model,
                thinking,
            });
        } else {
            self.agent_state.pending_prompt = Some(PendingAgentPrompt {
                text,
                attachments,
                mode,
                model,
                thinking,
            });
            self.create_agent_thread_with_defaults();
        }
        Ok(())
    }

    /// Fire a `Cancel` (legacy) or `CancelInflight` (session)
    /// envelope.
    pub fn agent_cancel(&mut self) {
        use neoism_protocol::agent::AgentClientMessage;

        let envelope = if let Some(session_id) = self.agent_state.session_id.clone() {
            AgentClientMessage::CancelInflight { session_id }
        } else {
            AgentClientMessage::Cancel
        };
        self.send_agent_envelope(&envelope);
    }

    /// Wake/attach to the daemon-backed agent-server without
    /// creating a new session. Mirrors desktop's agent-pane open:
    /// start/connect the server and load session/provider catalogs;
    /// actual session creation waits until first prompt.
    pub fn agent_attach(&mut self, directory: Option<String>) {
        use neoism_protocol::agent::AgentClientMessage;

        self.agent_state.default_directory = directory
            .as_ref()
            .filter(|dir| !dir.trim().is_empty())
            .cloned();
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.set_directory(self.agent_state.default_directory.clone());
        }
        self.send_agent_envelope(&AgentClientMessage::ListThreads {
            directory: self.agent_state.default_directory.clone(),
            limit: Some(50),
        });
        self.send_agent_envelope(&AgentClientMessage::GetConfigDefaults {
            directory: self.agent_state.default_directory.clone(),
        });
        self.send_agent_envelope(&AgentClientMessage::ListProviders);
        self.send_agent_envelope(&AgentClientMessage::ListAgents {
            directory: self.agent_state.default_directory.clone(),
        });
        self.send_agent_envelope(&AgentClientMessage::ListSkills {
            directory: self.agent_state.default_directory.clone(),
        });
    }

    /// Fire a `CreateThread` envelope to spin up a fresh
    /// agent-server session. The daemon replies with
    /// `ThreadCreated`; on ingestion the bridge stamps the new
    /// `session_id` and subsequent prompts route through it.
    pub fn agent_new_thread(&mut self, directory: Option<String>) {
        // Drop the local view of the session so the next prompt
        // creates a fresh one via the EnsureSession path in
        // `drain_agent_outbound`. NO eager CreateThread here —
        // creating before the user actually says anything littered
        // the /sessions catalog with empty "New Session" rows.
        self.agent_state.session_id = None;
        self.agent_state.pending_prompt = None;
        // Reset the pane to its fresh-chat state immediately —
        // waiting on a server ack left the old conversation on
        // screen, which reads as "it just took me back".
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.start_new_conversation();
        }
        // Gate out the old session's still-streaming events until
        // the next thread announces itself (see
        // `should_apply_agent_event`). Deliberately NOT
        // `thread_create_inflight` — that would dead-lock the
        // single-flight guard in `create_agent_thread_with_defaults`
        // since no CreateThread is actually in flight.
        self.agent_state.suppress_stale_session_events = true;
        self.agent_state.default_directory = directory
            .as_ref()
            .filter(|dir| !dir.trim().is_empty())
            .cloned();
    }

    /// Fire one `AgentClientMessage` through the JS-installed
    /// `set_agent_send` callback. Returns `true` when an envelope
    /// was actually delivered; `false` when no callback was
    /// installed or JSON serialisation failed (in practice
    /// neither happens for our PODs, but the guard keeps the
    /// surface honest).
    pub(crate) fn send_agent_envelope(
        &mut self,
        envelope: &neoism_protocol::agent::AgentClientMessage,
    ) -> bool {
        let Some(cb) = self.agent_state.send_cb.clone() else {
            return false;
        };
        let Ok(json) = serde_json::to_string(envelope) else {
            return false;
        };
        let id = self.agent_state.next_request_id.wrapping_add(1).max(1);
        self.agent_state.next_request_id = id;
        let _ = cb.call2(
            &JsValue::NULL,
            &JsValue::from_f64(id as f64),
            &JsValue::from_str(&json),
        );
        true
    }

    /// Drain `pending_outbound` off the shared `NeoismAgentPane`
    /// and turn each `OutboundAgentCommand` into the matching
    /// `AgentClientMessage`, then ship it through `set_agent_send`.
    /// Should be called after every pane-mutating bridge entrypoint
    /// (event handler, agent input setter, …) so user-initiated
    /// state changes always make it onto the wire.
    ///
    /// Returns the number of envelopes successfully forwarded.
    /// Variants the daemon-side has no native equivalent for yet
    /// are surfaced as pane system messages instead of being
    /// silently dropped.
    pub fn drain_agent_outbound(&mut self) -> u32 {
        use neoism_ui::panels::agent_pane::protocol_mapping::{
            map_outbound_command, AgentProtocolMapping, AgentProtocolMappingContext,
        };

        let commands = match self.chrome.agent_pane_mut() {
            Some(pane) if pane.has_pending_outbound() => pane.drain_pending_outbound(),
            _ => return 0,
        };
        let mut delivered = 0u32;
        for command in commands {
            let context = AgentProtocolMappingContext {
                active_session_id: self.agent_state.session_id.clone(),
                default_directory: self.agent_state.default_directory.clone(),
                default_agent: self.agent_state.default_agent.clone(),
                default_model: self.agent_state.default_model.clone(),
                default_thinking: self.agent_state.default_thinking.clone(),
            };
            match map_outbound_command(command, &context) {
                AgentProtocolMapping::EnsureSession => {
                    if self.agent_state.session_id.is_none() {
                        self.create_agent_thread_with_defaults();
                        delivered = delivered.saturating_add(1);
                    }
                }
                AgentProtocolMapping::PendingPrompt(prompt) => {
                    self.agent_state.pending_prompt = Some(PendingAgentPrompt {
                        text: prompt.text,
                        attachments: Vec::new(),
                        mode: prompt.mode,
                        model: prompt.model,
                        thinking: prompt.thinking,
                    });
                    if self.agent_state.session_id.is_none() {
                        self.create_agent_thread_with_defaults();
                        delivered = delivered.saturating_add(1);
                    }
                }
                AgentProtocolMapping::Messages(messages) => {
                    for envelope in messages {
                        if let neoism_protocol::agent::AgentClientMessage::SwitchThread {
                                session_id,
                            } = &envelope
                            {
                                self.agent_state.session_id = Some(session_id.clone());
                                self.agent_state.requested_session_id = Some(session_id.clone());
                            }
                        if self.send_agent_envelope(&envelope) {
                            delivered = delivered.saturating_add(1);
                        }
                    }
                }
                AgentProtocolMapping::Unsupported(reason) => {
                    if let Some(pane) = self.chrome.agent_pane_mut() {
                        pane.system_message("Web agent", reason.to_string());
                    }
                }
            }
        }
        delivered
    }

    /// Route a pointer-down on the agent pane through the same
    /// priority chain desktop uses (`handle_neoism_agent_click`):
    /// picker rows → side-panel toggle/rows → permission buttons →
    /// links → tool-card expand → wordmark. Returns JSON
    /// `{ handled, copy, link }` — `copy` carries code-block text
    /// the host should put on the clipboard, `link` a link target
    /// the host should open.
    pub fn agent_pointer_down(&mut self, x: f32, y: f32) -> JsValue {
        #[derive(serde::Serialize, Default)]
        struct ClickResult {
            handled: bool,
            copy: Option<String>,
            link: Option<String>,
        }
        let mut result = ClickResult::default();
        let mut relayout = false;
        // The full-width chrome top bar paints above the agent pane.
        // Let clicks in its row fall through to the chrome event path
        // (top-bar panel toggles) instead of being eaten by the
        // timeline / wordmark here — otherwise the right-edge toggle
        // would close the panel but never re-open it.
        let in_top_bar = self
            .chrome
            .layout()
            .top_bar
            .is_some_and(|r| y >= r.y && y < r.y + r.h);
        'chain: {
            if in_top_bar {
                break 'chain;
            }
            let Some(pane) = self.chrome.agent_pane_mut() else {
                break 'chain;
            };
            // /sessions, /model, slash-command pickers overlay the
            // timeline — a row tap commits the picker first, and
            // ANY press inside the card is consumed so it can't
            // fall through to whatever sits underneath (the card
            // must be solid).
            if pane.pick_at(x, y) {
                result.handled = true;
                break 'chain;
            }
            if pane.picker_contains_point(x, y) {
                result.handled = true;
                break 'chain;
            }
            if pane.side_panel().toggle_button_contains(x, y) {
                pane.side_panel_mut().toggle_visibility();
                result.handled = true;
                relayout = true;
                break 'chain;
            }
            if pane.side_panel().contains_point(x, y) {
                pane.side_panel_mut().set_focused(true);
                if let Some(rect) = pane.side_panel().last_panel_rect() {
                    if let Some(row) = pane.side_panel().hit_test_row(x, y, rect) {
                        pane.side_panel_mut().set_selected(row);
                        let activated = if pane.has_conversation() {
                            pane.activate_side_panel_subagent()
                        } else {
                            pane.activate_side_panel_selection()
                        };
                        if activated {
                            pane.side_panel_mut().set_focused(false);
                        }
                    }
                }
                result.handled = true;
                break 'chain;
            } else if pane.side_panel().is_focused() {
                pane.side_panel_mut().set_focused(false);
            }
            if pane.respond_permission_at(x, y) {
                result.handled = true;
                break 'chain;
            }
            if let Some(chip) = pane.status_chip_at(x, y) {
                pane.open_status_chip_picker(chip);
                result.handled = true;
                break 'chain;
            }
            if let Some(link) = pane.link_at(x, y) {
                if let Some(key) =
                        neoism_ui::panels::agent_pane::view::markdown::mermaid_toggle_key_from_link_target(&link)
                    {
                        pane.toggle_mermaid_raw_mode(key);
                    } else if let Some(text) =
                        neoism_ui::panels::agent_pane::view::markdown::copied_code_from_link_target(&link)
                    {
                        let chars = text.chars().count();
                        pane.push_copied_notice(chars);
                        result.copy = Some(text);
                    } else {
                        result.link = Some(link);
                    }
                result.handled = true;
                break 'chain;
            }
            if pane.toggle_tool_at(x, y) || pane.pop_wordmark_click(x, y) {
                result.handled = true;
                break 'chain;
            }
        }
        if relayout {
            self.relayout_chrome();
        }
        if result.handled {
            // Picker commits / permission replies queue outbound
            // agent messages — flush them to the daemon now.
            let _ = self.drain_agent_outbound();
        }
        serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL)
    }

    /// Wheel routing over the agent pane with the desktop priority:
    /// picker overlay → side panel → diff-card body under the
    /// cursor → timeline. `delta_pixels` uses the timeline sign
    /// convention (positive scrolls up into history).
    pub fn agent_scroll_at(&mut self, x: f32, y: f32, delta_pixels: f32) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        if pane.picker_contains_point(x, y) {
            pane.scroll_picker_pixels(delta_pixels);
            return true;
        }
        if pane.side_panel().contains_point(x, y) {
            let rows = pane.side_panel().last_panel_height_rows();
            pane.side_panel_mut().scroll_pixels(delta_pixels, rows);
            return true;
        }
        if pane.timeline_contains_point(x, y) {
            // Diff/code cards scroll internally when the cursor is
            // over them; their sign is flipped vs the timeline
            // (mirrors desktop's `scroll_diff_at` call).
            if let Some(scrolled) = pane.scroll_diff_at(x, y, -delta_pixels) {
                return scrolled;
            }
            return pane.scroll_timeline_pixels(delta_pixels);
        }
        false
    }

    /// Touch-drag routing over the agent pane. Returns which
    /// surface consumed the drag: 0 = none, 1 = overlay/diff card
    /// (no fling on release), 2 = timeline (host may fling).
    pub fn agent_drag_at(&mut self, x: f32, y: f32, dy_pixels: f32) -> i32 {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return 0;
        };
        if pane.picker_contains_point(x, y) {
            pane.scroll_picker_pixels(dy_pixels);
            return 1;
        }
        if pane.side_panel().contains_point(x, y) {
            let rows = pane.side_panel().last_panel_height_rows();
            pane.side_panel_mut().scroll_pixels(dy_pixels, rows);
            return 1;
        }
        if pane.timeline_contains_point(x, y) {
            if pane.scroll_diff_at(x, y, -dy_pixels).is_some() {
                return 1;
            }
            pane.drag_timeline_pixels(dy_pixels);
            return 2;
        }
        0
    }

    pub fn agent_wordmark_click(&mut self, x: f32, y: f32) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        pane.pop_wordmark_click(x, y)
    }
}
