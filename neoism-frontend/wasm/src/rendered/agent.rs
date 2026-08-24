use super::*;

pub(super) fn open_agent_usage_menu(
    bridge: &mut ChromeBridge,
    lines: Vec<String>,
    x: f32,
    y: f32,
) {
    use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
    use neoism_ui::widgets::modal::ModalAction;

    if lines.is_empty() {
        return;
    }
    let mut items = lines
        .into_iter()
        .map(|line| {
            let mut item = ContextMenuItem::new(
                line,
                "",
                ContextMenuAction::Modal(ModalAction::Close.into()),
            );
            item.enabled = false;
            item
        })
        .collect::<Vec<_>>();
    items.push(ContextMenuItem::new(
        "Close",
        "Esc",
        ContextMenuAction::Modal(ModalAction::Close.into()),
    ));
    bridge.chrome.context_menu.open(
        "Context usage",
        items,
        x,
        y,
        bridge.viewport.w,
        bridge.viewport.h,
    );
}

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

        // List-level refresh triggers run BEFORE the per-session gate:
        // a rename / pin / delete can target a session other than the
        // active one (any row of the /sessions picker), and its ack
        // must still refresh the catalog.
        self.note_agent_catalog_side_effects(&parsed);

        if !self.should_apply_agent_event(&parsed) {
            // NOT dropped: cache-eligible events for non-active sessions
            // stream into the shared pane's background session cache
            // (desktop's `!stream_is_active` ingest arms), so switching
            // to that session later restores a fully-caught-up
            // conversation instantly. The bridge mirror is deliberately
            // skipped — cached events must not flip `agent_state`
            // (session id, streaming flag) for the live view.
            if agent_event_session_id(&parsed).is_some() {
                if let Some(pane) = self.chrome.agent_pane_mut() {
                    apply_agent_event_to_cache(pane, parsed);
                }
            }
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

    /// Session-catalog refresh triggers that must fire regardless of
    /// the per-session event gate: the daemon acks a rename / pin
    /// (`ThreadUpdated`) or delete (`ThreadDeleted`) only AFTER the
    /// upstream mutation landed, so re-requesting the thread list here
    /// is race-free — the desktop analogue is
    /// `refresh_sessions_after_mutation`.
    pub(crate) fn note_agent_catalog_side_effects(
        &mut self,
        parsed: &neoism_protocol::agent::AgentServerMessage,
    ) {
        use neoism_protocol::agent::{AgentClientMessage, AgentServerMessage};
        if matches!(
            parsed,
            AgentServerMessage::ThreadUpdated { .. }
                | AgentServerMessage::ThreadDeleted { .. }
        ) {
            self.send_agent_envelope(&AgentClientMessage::ListThreads {
                directory: self.agent_state.default_directory.clone(),
                limit: Some(50),
            });
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
                ..
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
            message_id: prompt.message_id,
            text: prompt.text,
            author: prompt.author,
            attachments: prompt.attachments,
            mode: prompt.mode,
            model: prompt.model,
            thinking: prompt.thinking,
            delivery: prompt.delivery,
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

    /// Reconcile the composer with a JS-side text push (tab re-open,
    /// mobile IME sync). The pane is the single source of truth for
    /// the composer — there is no bridge-side mirror — so an equal
    /// string is a no-op and a pure tail-append is applied as an
    /// incremental `insert_text` (preserving the caret, any open
    /// slash/@file picker, and composer attachment tokens). Only a
    /// genuinely divergent text falls back to `replace_input`.
    pub fn agent_set_input(&mut self, text: &str) {
        {
            let Some(pane) = self.chrome.agent_pane_mut() else {
                return;
            };
            let current = pane.input().to_string();
            if current == text {
                return;
            }
            match text.strip_prefix(current.as_str()) {
                Some(suffix)
                    if !suffix.is_empty() && pane.cursor_byte() == current.len() =>
                {
                    pane.insert_text(suffix);
                }
                _ => pane.replace_input(text),
            }
        }
        // Typing into a picker (slash / @file / $skill) can queue
        // outbound refreshes — flush like every other entrypoint.
        let _ = self.drain_agent_outbound();
    }

    /// Current composer input — read straight off the pane.
    pub fn agent_input(&self) -> String {
        self.chrome
            .agent_pane()
            .map(|pane| pane.input().to_string())
            .unwrap_or_default()
    }

    /// Clear the composer input (Esc semantics: clears a non-empty
    /// composer, aborts a live run on an empty one).
    pub fn agent_clear_input(&mut self) {
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.clear_or_abort();
        }
        let _ = self.drain_agent_outbound();
    }

    /// Route pasted clipboard text through the shared pane's paste
    /// path (`insert_paste`): picker-aware routing, `[pasted N lines]`
    /// token compaction for large pastes, and composer attachment
    /// bookkeeping — the same pipeline desktop's Ctrl+V uses. JS
    /// calls this from its ClipboardEvent handler after
    /// `agent_handle_key` declined the Ctrl+V press (the clipboard
    /// payload only exists on the async browser paste event).
    /// Returns `true` when an agent pane consumed the paste.
    pub fn agent_insert_paste(&mut self, text: &str) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        pane.insert_paste(text);
        let _ = self.drain_agent_outbound();
        true
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
            pending_question: pane.pending_question().is_some(),
            picker_open: pane.picker().is_some(),
            session_picker_open: pane.session_picker_open(),
            session_rename_active: pane.session_rename_active(),
        };
        let decision = agent_key_decision(&event, mods, ctx);
        if !decision.handled {
            return false;
        }
        // Ctrl+V: the clipboard payload is NOT part of the key event on
        // the web — it only arrives on the browser's asynchronous
        // ClipboardEvent. Returning `false` here (before any pane
        // mutation) leaves the keypress unconsumed so that event fires;
        // the JS paste handler then routes the text back through
        // `agent_insert_paste` → the pane's `insert_paste` (desktop's
        // exact paste pipeline: picker-aware, `[pasted N lines]`
        // compaction, attachment tokens).
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
                AgentKeyIntent::MoveQuestionSelection(delta) => {
                    let _ = pane.move_question_selection(delta);
                }
                AgentKeyIntent::QuestionBackspace => {
                    let _ = pane.question_backspace();
                }
                AgentKeyIntent::QuestionInput(value) => {
                    let _ = pane.question_type_str(&value);
                }
                AgentKeyIntent::SubmitPendingQuestion => {
                    let _ = pane.submit_pending_question();
                }
                AgentKeyIntent::RejectPendingQuestion => {
                    let _ = pane.reject_pending_question();
                }
                AgentKeyIntent::ToggleSelectedSessionPin => {
                    let _ = pane.toggle_selected_session_pin();
                }
                AgentKeyIntent::DeleteSelectedSession => {
                    let _ = pane.delete_selected_session();
                }
                AgentKeyIntent::BeginSelectedSessionRename => {
                    let _ = pane.begin_selected_session_rename();
                }
                AgentKeyIntent::SessionRenameInput(value) => {
                    pane.push_session_rename(&value);
                }
                AgentKeyIntent::SessionRenameBackspace => {
                    pane.backspace_session_rename();
                }
                AgentKeyIntent::SessionRenameCommit => {
                    let _ = pane.commit_session_rename();
                }
                AgentKeyIntent::SessionRenameCancel => {
                    pane.cancel_session_rename();
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
                    if pane.side_panel().back_focused() {
                        // Enter on "← Back" flips the home-override view
                        // without touching the live conversation.
                        pane.side_panel_mut().trigger_back_scramble();
                        pane.side_panel_mut().toggle_home_override();
                        pane.side_panel_mut().focus_back();
                    } else {
                        let showing_sessions = !pane.has_conversation()
                            || pane.side_panel().show_home_override();
                        let mut activated = if showing_sessions {
                            pane.activate_side_panel_selection()
                        } else {
                            pane.activate_side_panel_subagent()
                        };
                        if !activated
                            && pane.side_panel().show_home_override()
                            && pane.selected_side_panel_session_is_current()
                        {
                            activated = true;
                        }
                        if activated {
                            pane.side_panel_mut().set_show_home_override(false);
                            pane.side_panel_mut().set_focused(false);
                        }
                    }
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
                // Unreachable — a Paste decision returns `false` above
                // so the browser's ClipboardEvent can deliver the
                // payload (see `agent_insert_paste`).
                AgentKeyIntent::Paste => {}
            }
        }
        let _ = self.drain_agent_outbound();
        true
    }

    /// Step through input history. `delta < 0` walks back in time
    /// (older entries); `delta > 0` walks forward toward the live
    /// edit. Returns the resulting input text so JS can mirror it
    /// into its DOM composer in one step. Backed entirely by the
    /// pane's own zsh-style history walk (`sent_history` +
    /// `history_draft` inside `AgentInputBuffer`) — the bridge no
    /// longer keeps a parallel history vec.
    pub fn agent_history_step(&mut self, delta: i32) -> String {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return String::new();
        };
        for _ in 0..delta.unsigned_abs() {
            if delta < 0 {
                pane.move_input_up_or_history();
            } else {
                pane.move_input_down_or_history();
            }
        }
        pane.input().to_string()
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

    /// True if a `question` tool request is awaiting the user's
    /// answer (the shared prompt picker owns the keyboard while set).
    pub fn agent_has_pending_question(&self) -> bool {
        self.chrome
            .agent_pane()
            .is_some_and(|pane| pane.pending_question().is_some())
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
        if self.chrome.agent_pane_mut().is_some() {
            {
                let pane = self
                    .chrome
                    .agent_pane_mut()
                    .expect("agent pane checked above");
                if pane.input() != text {
                    pane.replace_input(text);
                }
                // The pane records the prompt into its own
                // `sent_history` (Up-arrow recall) as part of submit.
                let _ = pane.submit();
            }
            let _ = self.drain_agent_outbound();
            return;
        }

        let (mode, model, thinking) = self.agent_prompt_defaults();
        self.note_prompt_for_history(trimmed);
        self.agent_state.pending_prompt = Some(PendingAgentPrompt {
            message_id: neoism_ui::panels::agent_pane::outbound::next_prompt_message_id(),
            text: trimmed.to_string(),
            author: self.agent_state.local_presence_name.clone(),
            attachments: Vec::new(),
            mode,
            model,
            thinking,
            delivery: neoism_protocol::agent::PromptDelivery::Steer,
        });
        self.create_agent_thread_with_defaults();
    }

    /// Attach a pasted clipboard image to the shared composer as an
    /// `[imageN]` token + chip — desktop's Ctrl+V-with-image flow. The
    /// image is NOT sent yet; it rides along with the next submit
    /// (Enter), and the sent user card renders it like desktop.
    /// Returns `false` when the pane rejected the payload (empty,
    /// non-image mime, or over the 20MB cap) so the host can surface
    /// a notice.
    pub fn agent_attach_clipboard_image(
        &mut self,
        filename: &str,
        mime: &str,
        bytes: &[u8],
    ) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        let attached = pane.attach_clipboard_image(filename, mime, bytes);
        if attached {
            let _ = self.drain_agent_outbound();
        }
        attached
    }

    /// Attach a host-mediated file (drag-and-drop onto the agent pane,
    /// file picker) to the shared composer — the web analogue of
    /// desktop's `DroppedFile` → `attach_path`. Any mime is accepted
    /// (`[imageN]` / `[pdfN]` / `[fileN: name]` token per the shared
    /// attachment policy); an empty `mime` is sniffed from the
    /// filename. Send happens on the next submit.
    pub fn agent_attach_file(
        &mut self,
        filename: &str,
        mime: &str,
        bytes: &[u8],
    ) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        let attached = pane.attach_file_bytes(filename, mime, bytes);
        if attached {
            let _ = self.drain_agent_outbound();
        }
        attached
    }

    /// The active `@`-mention query in the shared composer (the text
    /// between `@` and the caret), or `null` when no mention is being
    /// typed. JS polls this after key/paste routing to decide when to
    /// fetch + feed mention candidates.
    pub fn agent_file_mention_query(&self) -> JsValue {
        match self
            .chrome
            .agent_pane()
            .and_then(|pane| pane.file_mention_query())
        {
            Some(query) => JsValue::from_str(&query),
            None => JsValue::NULL,
        }
    }

    /// Install the `@`-mention candidate list (JSON array of
    /// workspace-relative file paths) on the shared pane. The pane
    /// ranks candidates per keystroke with the desktop `fuzzy_score`
    /// policy, so this only needs to be re-fed when the file list
    /// itself changes. Returns `false` on a parse error or when no
    /// agent pane is installed.
    pub fn agent_set_file_mention_candidates(&mut self, json: &str) -> bool {
        let Ok(paths) = serde_json::from_str::<Vec<String>>(json) else {
            return false;
        };
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        pane.set_file_mention_candidates(paths);
        true
    }

    /// Same submit path as `agent_send_message`, but with structured
    /// attachments supplied by the JS host. Kept for older host code:
    /// the attachments are attached to the shared pane as composer
    /// chips (`attach_file_bytes`) and the prompt submits through the
    /// pane, so the user bubble, image rail, streaming state, and
    /// session bootstrap all match a plain Enter. New host code should
    /// attach via `agent_attach_clipboard_image` / `agent_attach_file`
    /// and let the user press Enter instead.
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
        let prompt_text = if trimmed.is_empty() {
            "Please analyze the pasted image."
        } else {
            trimmed
        };

        if self.chrome.agent_pane_mut().is_some() {
            {
                let pane = self
                    .chrome
                    .agent_pane_mut()
                    .expect("agent pane checked above");
                if pane.input() != prompt_text {
                    pane.replace_input(prompt_text);
                }
                for attachment in &attachments {
                    let filename = attachment.path.as_deref().unwrap_or("");
                    let _ = pane.attach_file_bytes(
                        filename,
                        &attachment.kind,
                        &attachment.bytes,
                    );
                }
                let _ = pane.submit();
            }
            let _ = self.drain_agent_outbound();
            return Ok(());
        }

        // No pane installed (headless bridge) — legacy direct path.
        self.note_prompt_for_history(prompt_text);
        let (mode, model, thinking) = self.agent_prompt_defaults();
        if let Some(session_id) = self.agent_state.session_id.clone() {
            self.send_agent_envelope(&AgentClientMessage::SubmitPrompt {
                session_id,
                message_id:
                    neoism_ui::panels::agent_pane::outbound::next_prompt_message_id(),
                text: prompt_text.to_string(),
                author: self.agent_state.local_presence_name.clone(),
                attachments,
                mode,
                model,
                thinking,
                delivery: neoism_protocol::agent::PromptDelivery::Steer,
            });
        } else {
            self.agent_state.pending_prompt = Some(PendingAgentPrompt {
                message_id:
                    neoism_ui::panels::agent_pane::outbound::next_prompt_message_id(),
                text: prompt_text.to_string(),
                author: self.agent_state.local_presence_name.clone(),
                attachments,
                mode,
                model,
                thinking,
                delivery: neoism_protocol::agent::PromptDelivery::Steer,
            });
            self.create_agent_thread_with_defaults();
        }
        Ok(())
    }

    // -------- prompt history persistence ------------------------
    //
    // Desktop keeps a zsh-style global prompt history file
    // (`desktop/src/neoism/agent/prompt_history.rs`): every sent
    // prompt appends, capped to the most recent 1000, and each new
    // pane seeds Up-arrow recall from it. The web analogue lives in
    // browser localStorage. The bridge holds only a persistence
    // LEDGER (oldest-first, like the pane's `sent_history` walk
    // order) — live recall state stays in the pane. JSON exchanged
    // with JS is newest-first, matching the desktop file's read
    // order for a fresh pane.

    /// Record one sent prompt into the persistence ledger and
    /// write-through to localStorage. Empties are skipped and a
    /// prompt identical to the previous entry is deduped
    /// (`HIST_IGNORE_DUPS`), mirroring desktop `prompt_history::append`.
    pub(crate) fn note_prompt_for_history(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.ensure_prompt_history_loaded();
        if self
            .agent_state
            .prompt_history
            .last()
            .is_some_and(|last| last == trimmed)
        {
            return;
        }
        self.agent_state.prompt_history.push(trimmed.to_string());
        let len = self.agent_state.prompt_history.len();
        if len > MAX_PROMPT_HISTORY {
            self.agent_state
                .prompt_history
                .drain(0..len - MAX_PROMPT_HISTORY);
        }
        self.persist_prompt_history();
    }

    fn ensure_prompt_history_loaded(&mut self) {
        if self.agent_state.prompt_history_loaded {
            return;
        }
        self.agent_state.prompt_history_loaded = true;
        let Some(stored) = local_storage_get(PROMPT_HISTORY_KEY) else {
            return;
        };
        let Ok(newest_first) = serde_json::from_str::<Vec<String>>(&stored) else {
            return;
        };
        let mut restored: Vec<String> = newest_first
            .into_iter()
            .rev()
            .filter(|entry| !entry.trim().is_empty())
            .collect();
        // Entries recorded before the load (a prompt raced the first
        // read) stay newest — append them after the restored base.
        restored.append(&mut self.agent_state.prompt_history);
        restored.dedup();
        let len = restored.len();
        if len > MAX_PROMPT_HISTORY {
            restored.drain(0..len - MAX_PROMPT_HISTORY);
        }
        self.agent_state.prompt_history = restored;
    }

    fn persist_prompt_history(&self) {
        let newest_first: Vec<&str> = self
            .agent_state
            .prompt_history
            .iter()
            .rev()
            .map(String::as_str)
            .collect();
        if let Ok(json) = serde_json::to_string(&newest_first) {
            local_storage_set(PROMPT_HISTORY_KEY, &json);
        }
    }

    /// Snapshot the persisted prompt history as a JSON array of
    /// strings, NEWEST FIRST, capped at 1000 — the desktop
    /// `prompt_history.rs` shape. JS can persist / inspect this;
    /// the bridge also write-throughs to localStorage itself on
    /// every send, so calling this is optional.
    pub fn agent_prompt_history_json(&mut self) -> String {
        self.ensure_prompt_history_loaded();
        let newest_first: Vec<&str> = self
            .agent_state
            .prompt_history
            .iter()
            .rev()
            .map(String::as_str)
            .collect();
        serde_json::to_string(&newest_first).unwrap_or_else(|_| "[]".to_string())
    }

    /// Restore a persisted prompt history (JSON array of strings,
    /// newest first — the `agent_prompt_history_json` shape) into
    /// the bridge ledger, replacing any localStorage-loaded base.
    /// Returns `false` on a parse error.
    ///
    pub fn agent_restore_prompt_history(&mut self, json: &str) -> bool {
        let Ok(newest_first) = serde_json::from_str::<Vec<String>>(json) else {
            return false;
        };
        self.agent_state.prompt_history_loaded = true;
        let mut oldest_first: Vec<String> = newest_first
            .into_iter()
            .rev()
            .filter(|entry| !entry.trim().is_empty())
            .collect();
        let len = oldest_first.len();
        if len > MAX_PROMPT_HISTORY {
            oldest_first.drain(0..len - MAX_PROMPT_HISTORY);
        }
        self.agent_state.prompt_history = oldest_first;
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.seed_sent_history(self.agent_state.prompt_history.clone());
        }
        self.persist_prompt_history();
        true
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
    pub fn agent_attach(
        &mut self,
        directory: Option<String>,
        presence_name: Option<String>,
    ) {
        use neoism_protocol::agent::AgentClientMessage;

        self.agent_state.local_presence_name =
            presence_name.filter(|name| !name.trim().is_empty());
        self.agent_state.default_directory = directory
            .as_ref()
            .filter(|dir| !dir.trim().is_empty())
            .cloned();
        if let Some(pane) = self.chrome.agent_pane_mut() {
            pane.set_directory(self.agent_state.default_directory.clone());
            pane.set_local_presence_name(self.agent_state.local_presence_name.clone());
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
                local_author: self.agent_state.local_presence_name.clone(),
            };
            match map_outbound_command(command, &context) {
                AgentProtocolMapping::EnsureSession => {
                    if self.agent_state.session_id.is_none() {
                        self.create_agent_thread_with_defaults();
                        delivered = delivered.saturating_add(1);
                    }
                }
                AgentProtocolMapping::PendingPrompt(prompt) => {
                    // Mirror desktop's zsh-style history: the ledger
                    // records prompts at send time (persisted to
                    // localStorage; see `note_prompt_for_history`).
                    // Attachments travel INSIDE the pending prompt now
                    // (extracted from the pane's file parts by
                    // `protocol_mapping`), so the no-session path ships
                    // pasted images exactly like a live-session submit.
                    self.note_prompt_for_history(&prompt.text);
                    self.agent_state.pending_prompt = Some(PendingAgentPrompt {
                        message_id: prompt.message_id,
                        text: prompt.text,
                        author: prompt.author,
                        attachments: prompt.attachments,
                        mode: prompt.mode,
                        model: prompt.model,
                        thinking: prompt.thinking,
                        delivery: prompt.delivery,
                    });
                    if self.agent_state.session_id.is_none() {
                        self.create_agent_thread_with_defaults();
                        delivered = delivered.saturating_add(1);
                    }
                }
                AgentProtocolMapping::Messages(messages) => {
                    for envelope in messages {
                        // Session we just moved to, if any — it needs a
                        // backfill (see below).
                        let mut opened_session: Option<String> = None;
                        match &envelope {
                            neoism_protocol::agent::AgentClientMessage::SwitchThread {
                                session_id,
                            } => {
                                self.agent_state.session_id = Some(session_id.clone());
                                self.agent_state.requested_session_id =
                                    Some(session_id.clone());
                                opened_session = Some(session_id.clone());
                            }
                            neoism_protocol::agent::AgentClientMessage::SubmitPrompt {
                                text,
                                ..
                            } => {
                                self.note_prompt_for_history(&text.clone());
                            }
                            _ => {}
                        }
                        if self.send_agent_envelope(&envelope) {
                            delivered = delivered.saturating_add(1);
                        }
                        // `SwitchThread` only BINDS the daemon's SSE stream,
                        // so events flow from that instant onward and
                        // anything the turn already produced is missing —
                        // open a session mid-run (or reload the page during
                        // one) and the timeline never caught up. Desktop
                        // fetches the transcript over HTTP on entry; the web
                        // twin is `GetHistory`, which the daemon answers with
                        // a `HistoryChunk` that `apply_history` folds in.
                        // Nothing in the web build sent either of these
                        // before — they existed end-to-end and had zero
                        // callers.
                        //
                        // `ResumeStream` additionally replays interactions
                        // that were parked while no client was attached (a
                        // `question` tool call blocks the run and its SSE
                        // event is long gone by the time the page loads).
                        if let Some(session_id) = opened_session {
                            let history =
                                neoism_protocol::agent::AgentClientMessage::GetHistory {
                                    session_id: session_id.clone(),
                                    cursor: None,
                                    limit: None,
                                };
                            if self.send_agent_envelope(&history) {
                                delivered = delivered.saturating_add(1);
                            }
                            let resume =
                                neoism_protocol::agent::AgentClientMessage::ResumeStream {
                                    session_id,
                                };
                            if self.send_agent_envelope(&resume) {
                                delivered = delivered.saturating_add(1);
                            }
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
            /// A text selection just started under this press. The JS host
            /// must now track pointermove -> `agent_selection_drag` and
            /// pointerup -> `agent_selection_end` to finish the drag-copy.
            selecting: bool,
        }
        let mut result = ClickResult::default();
        let mut relayout = false;
        let mut usage_menu_lines = None;
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
            if pane.side_panel().usage_contains(x, y) {
                usage_menu_lines = Some(pane.usage_detail_lines());
                result.handled = true;
                break 'chain;
            }
            if pane.side_panel().contains_point(x, y) {
                pane.side_panel_mut().set_focused(true);
                if pane.side_panel().back_button_contains(x, y) {
                    // "← Back" flips the home-override view; the live
                    // conversation stays open underneath.
                    pane.side_panel_mut().trigger_back_scramble();
                    pane.side_panel_mut().toggle_home_override();
                    pane.side_panel_mut().focus_back();
                } else if let Some(rect) = pane.side_panel().last_panel_rect() {
                    if let Some(row) = pane.side_panel().hit_test_row(x, y, rect) {
                        pane.side_panel_mut().set_selected(row);
                        // The sessions list shows when there's no
                        // conversation OR the Back override is active.
                        let showing_sessions = !pane.has_conversation()
                            || pane.side_panel().show_home_override();
                        let mut activated = if showing_sessions {
                            pane.activate_side_panel_selection()
                        } else {
                            pane.activate_side_panel_subagent()
                        };
                        if !activated
                            && pane.side_panel().show_home_override()
                            && pane.selected_side_panel_session_is_current()
                        {
                            activated = true;
                        }
                        if activated {
                            pane.side_panel_mut().set_show_home_override(false);
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
            // `question` tool prompt rows — click selects + commits,
            // mirroring desktop's `respond_question_at` hit right after
            // the permission chain.
            if pane.respond_question_at(x, y) {
                result.handled = true;
                break 'chain;
            }
            if pane.usage_chip_contains(x, y) {
                usage_menu_lines = Some(pane.usage_detail_lines());
                result.handled = true;
                break 'chain;
            }
            if let Some(chip) = pane.status_chip_at(x, y) {
                pane.open_status_chip_picker(chip);
                result.handled = true;
                break 'chain;
            }
            if pane.begin_markdown_horizontal_scrollbar_drag(x, y) {
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
                        pane.mark_code_copied(&link);
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
            // Nothing interactive under the press: start a text selection.
            // Last link in the chain for the same reason desktop puts
            // `begin_selection_at` last (`bridges/agent.rs`) - pickers,
            // side-panel rows, links and tool toggles all outrank it.
            if pane.begin_selection_at(x, y) {
                result.handled = true;
                result.selecting = true;
                break 'chain;
            }
        }
        if relayout {
            self.relayout_chrome();
        }
        if let Some(lines) = usage_menu_lines {
            open_agent_usage_menu(self, lines, x, y);
        }
        if result.handled {
            // Picker commits / permission replies queue outbound
            // agent messages — flush them to the daemon now.
            let _ = self.drain_agent_outbound();
        }
        serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL)
    }

    /// Extend the in-progress timeline text selection to `(x, y)`.
    /// Driven by the JS host's pointermove while the button is held,
    /// after `agent_pointer_down` reported `selecting: true`. Mirrors
    /// desktop's `drag_selection_to` call in `bridges/agent.rs`.
    pub fn agent_selection_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        pane.drag_selection_to(x, y)
    }

    /// Finish the drag and hand back the selected text so the host can
    /// put it on the clipboard (desktop copies on mouse-up the same
    /// way). Returns `None` when nothing was selected - a plain click.
    pub fn agent_selection_end(&mut self) -> Option<String> {
        let pane = self.chrome.agent_pane_mut()?;
        pane.end_selection()
    }

    /// True while a timeline selection drag owns the pointer.
    pub fn agent_has_active_selection(&self) -> bool {
        self.chrome
            .agent_pane()
            .is_some_and(|pane| pane.has_active_selection())
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

    /// Desktop-parity agent wheel. `delta_mode` mirrors the DOM
    /// `WheelEvent.deltaMode` (0 = pixels, 1 = lines, 2 = pages).
    ///
    /// Web used to feed the raw pixel delta straight into
    /// `scroll_timeline_pixels`, so a mouse notch lurched by whatever
    /// the browser reported (~100-120px on Chrome) with no smoothing,
    /// while desktop runs every wheel through
    /// `scroll_model::agent_timeline_wheel`: a NOTCH becomes
    /// `clamp(±3) * line_height * 3` and scrolls SMOOTHLY, and only
    /// trackpad pixel deltas go through raw. That policy difference is
    /// the whole reason chat scrolling felt worse in the browser.
    ///
    /// Browsers report a mouse wheel in pixels on most platforms, so a
    /// large quantised pixel delta is treated as a notch too — otherwise
    /// the smooth path would only ever engage on Firefox.
    pub fn agent_scroll_wheel_at(
        &mut self,
        x: f32,
        y: f32,
        delta_y: f32,
        delta_mode: u32,
    ) -> bool {
        use neoism_ui::editor::scroll_model::agent_timeline_wheel;
        use neoism_ui::panels::completion_menu::ScrollDelta;
        const LINE_HEIGHT: f32 = 24.0;
        // A notch is either an explicit line/page delta, or a pixel
        // delta big enough that no trackpad would emit it per event.
        let notch_lines = match delta_mode {
            1 => Some(delta_y),
            2 => Some(delta_y * 3.0),
            _ if delta_y.abs() >= 40.0 => Some(delta_y / 100.0),
            _ => None,
        };
        let shared = match notch_lines {
            Some(y) => ScrollDelta::Lines { x: 0.0, y },
            None => ScrollDelta::Pixels { x: 0.0, y: delta_y },
        };
        let wheel = agent_timeline_wheel(&shared, LINE_HEIGHT);
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        // Picker / side panel / diff cards keep their own routing.
        if pane.picker_contains_point(x, y) {
            pane.scroll_picker_pixels(wheel.pixels);
            return true;
        }
        if pane.side_panel().contains_point(x, y) {
            let rows = pane.side_panel().last_panel_height_rows();
            pane.side_panel_mut().scroll_pixels(wheel.pixels, rows);
            return true;
        }
        if !pane.timeline_contains_point(x, y) {
            return false;
        }
        if let Some(scrolled) = pane.scroll_diff_at(x, y, -wheel.pixels) {
            return scrolled;
        }
        if wheel.smooth {
            pane.scroll_timeline_wheel_pixels(wheel.pixels)
        } else {
            pane.scroll_timeline_pixels(wheel.pixels)
        }
    }

    /// Horizontal wheel/trackpad routing for rendered Markdown code blocks
    /// and tables. Kept separate from `agent_scroll_at` so a normal vertical
    /// wheel never gets captured by an overflow block.
    pub fn agent_scroll_horizontal_at(
        &mut self,
        x: f32,
        y: f32,
        delta_pixels: f32,
    ) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        if !pane.timeline_contains_point(x, y) {
            return false;
        }
        pane.scroll_markdown_horizontal_at(x, y, delta_pixels)
            .is_some()
    }

    /// Continue a direct mouse drag of a rendered Markdown code/table
    /// horizontal scrollbar. A true result means the active drag owns the
    /// pointer even when it is already clamped at an edge.
    pub fn agent_drag_markdown_horizontal_scrollbar(&mut self, x: f32) -> bool {
        let Some(pane) = self.chrome.agent_pane_mut() else {
            return false;
        };
        if !pane.markdown_horizontal_scrollbar_dragging() {
            return false;
        }
        pane.drag_markdown_horizontal_scrollbar_to(x);
        true
    }

    pub fn agent_end_markdown_horizontal_scrollbar_drag(&mut self) -> bool {
        self.chrome
            .agent_pane_mut()
            .is_some_and(|pane| pane.end_markdown_horizontal_scrollbar_drag())
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

/// localStorage key for the persisted agent prompt history (JSON
/// array of strings, newest first).
pub(crate) const PROMPT_HISTORY_KEY: &str = "neoism.agent.prompt-history.v1";
/// zsh's default `SAVEHIST` — matches desktop
/// `prompt_history::MAX_PROMPT_HISTORY`.
pub(crate) const MAX_PROMPT_HISTORY: usize = 1000;

// Browser localStorage via `js_sys` reflection (the crate's `web-sys`
// feature set doesn't include `Storage`, and these two calls don't
// justify widening it). Best-effort on both ends: a sandboxed /
// storage-denied context (some private modes throw on ACCESS) simply
// yields `None` and history stays session-local, mirroring desktop's
// swallow-on-write-failure stance.
fn local_storage() -> Option<JsValue> {
    let storage =
        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("localStorage"))
            .ok()?;
    (!storage.is_undefined() && !storage.is_null()).then_some(storage)
}

fn local_storage_call(method: &str, args: &[&JsValue]) -> Option<JsValue> {
    use wasm_bindgen::JsCast;
    let storage = local_storage()?;
    let func: js_sys::Function =
        js_sys::Reflect::get(&storage, &JsValue::from_str(method))
            .ok()?
            .dyn_into()
            .ok()?;
    match args {
        [a] => func.call1(&storage, a).ok(),
        [a, b] => func.call2(&storage, a, b).ok(),
        _ => None,
    }
}

pub(crate) fn local_storage_get(key: &str) -> Option<String> {
    local_storage_call("getItem", &[&JsValue::from_str(key)])?.as_string()
}

pub(crate) fn local_storage_set(key: &str, value: &str) {
    let _ = local_storage_call(
        "setItem",
        &[&JsValue::from_str(key), &JsValue::from_str(value)],
    );
}
