use super::*;

impl NeoismAgentPane {
    pub fn drain_ui_events(&mut self) -> Vec<NeoismAgentUiEvent> {
        std::mem::take(&mut self.ui_events)
    }

    pub fn timeline_scroll_offset(&self) -> f32 {
        self.timeline_scroll_px
    }

    pub fn sync_virtual_timeline(
        &mut self,
        viewport_rect: [f32; 4],
        content_width: f32,
        content_height: f32,
        scroll_top: f32,
        scale: f32,
        rows: &[TimelineVirtualRowMeasurement],
    ) {
        let session_id = self
            .session_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .unwrap_or("neoism-agent-session")
            .to_string();
        let needs_rebuild = self.virtual_timeline.layout_epoch
            != self.timeline_layout_epoch
            || self.virtual_timeline.content_revision != self.timeline_content_revision;
        if needs_rebuild {
            let messages = self.virtual_agent_messages();
            let next_signatures = messages
                .iter()
                .map(virtual_agent_message_signature)
                .collect::<Vec<_>>();
            self.virtual_timeline.revision =
                self.virtual_timeline.revision.saturating_add(1).max(1);
            let can_patch_messages = self.virtual_timeline.last_session_id.as_deref()
                == Some(session_id.as_str())
                && self.virtual_timeline.message_signatures.len()
                    == next_signatures.len()
                && self
                    .virtual_timeline
                    .message_signatures
                    .iter()
                    .zip(next_signatures.iter())
                    .all(|(previous, next)| {
                        previous.id == next.id
                            && previous.role == next.role
                            && previous.tool_name == next.tool_name
                    });
            if can_patch_messages {
                let previous_signatures =
                    self.virtual_timeline.message_signatures.clone();
                for (index, (message, (previous, next))) in messages
                    .iter()
                    .cloned()
                    .zip(previous_signatures.iter().zip(next_signatures.iter()))
                    .enumerate()
                {
                    if previous.markdown_hash == next.markdown_hash
                        && previous.markdown_len == next.markdown_len
                    {
                        continue;
                    }
                    let batch = self.virtual_timeline.adapter.build_update_message_batch(
                        &session_id,
                        VirtualAgentMessageUpdate {
                            index,
                            message,
                            old_range: NodeSourceRange::new(0, previous.markdown_len),
                            new_range: NodeSourceRange::new(0, next.markdown_len),
                            kind: DirtyKind::Layout,
                        },
                        VirtualSourceRevision(self.virtual_timeline.revision),
                    );
                    for command in batch.commands {
                        let _ = self.virtual_timeline.surface.apply(command);
                    }
                }
            } else {
                let batch = self.virtual_timeline.adapter.build_replace_batch(
                    &session_id,
                    &messages,
                    VirtualSourceRevision(self.virtual_timeline.revision),
                );
                for command in batch.commands {
                    let _ = self.virtual_timeline.surface.apply(command);
                }
            }
            self.virtual_timeline.last_session_id = Some(session_id.clone());
            self.virtual_timeline.message_signatures = next_signatures;
            self.virtual_timeline.layout_epoch = self.timeline_layout_epoch;
            self.virtual_timeline.content_revision = self.timeline_content_revision;
        }

        let _ = self
            .virtual_timeline
            .surface
            .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
                0.0,
                viewport_rect[1],
                content_width,
                viewport_rect[3],
                scale.max(0.01),
            )));
        self.virtual_timeline.surface.resolve_dirty_layout();
        if !rows.is_empty() {
            self.commit_virtual_timeline_measurements(rows);
            self.virtual_timeline.measured_layout_epoch = self.timeline_layout_epoch;
            self.virtual_timeline.measured_content_revision =
                self.timeline_content_revision;
            self.virtual_timeline.measured_width_bucket =
                f32_measure_bucket(content_width);
            self.virtual_timeline.measured_scale_bucket = f32_measure_bucket(scale);
            self.virtual_timeline.measured_row_count = rows.len();
            self.virtual_timeline.measured_content_height_bits =
                content_height.max(0.0).to_bits();
        }
        let _ = self
            .virtual_timeline
            .surface
            .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
                scroll_y: scroll_top.max(0.0),
                velocity_y: self.timeline_velocity_px_s,
            }));
        let visible = self.virtual_timeline.surface.visible_set();
        self.virtual_timeline.last_visible_nodes = visible.nodes.len();
        self.virtual_timeline.last_visible_source_range = visible
            .nodes
            .iter()
            .map(|node| node.index)
            .fold(None, |range: Option<(usize, usize)>, index| {
                Some(match range {
                    Some((start, end)) => (start.min(index), end.max(index)),
                    None => (index, index),
                })
            });
    }

    pub fn virtual_timeline_needs_measurements(
        &self,
        content_width: f32,
        scale: f32,
        row_count: usize,
        content_height: f32,
    ) -> bool {
        row_count > 0
            && (self.virtual_timeline.measured_layout_epoch != self.timeline_layout_epoch
                || self.virtual_timeline.measured_content_revision
                    != self.timeline_content_revision
                || self.virtual_timeline.measured_width_bucket
                    != f32_measure_bucket(content_width)
                || self.virtual_timeline.measured_scale_bucket
                    != f32_measure_bucket(scale)
                || self.virtual_timeline.measured_row_count != row_count
                || self.virtual_timeline.measured_content_height_bits
                    != content_height.max(0.0).to_bits())
    }

    pub fn virtual_timeline_visible_nodes(&self) -> usize {
        self.virtual_timeline.last_visible_nodes
    }

    pub fn virtual_timeline_visible_source_range(&self) -> Option<(usize, usize)> {
        self.virtual_timeline.last_visible_source_range
    }

    pub(in crate::panels::agent_pane::state) fn virtual_agent_messages(&self) -> Vec<VirtualAgentMessage> {
        self.messages
            .iter()
            .map(|message| VirtualAgentMessage {
                id: message.id.clone(),
                role: virtual_agent_role(message.kind),
                markdown: virtual_agent_markdown(message),
                tool_name: (!message.tool.trim().is_empty())
                    .then(|| message.tool.clone()),
            })
            .collect()
    }

    pub(in crate::panels::agent_pane::state) fn commit_virtual_timeline_measurements(
        &mut self,
        rows: &[TimelineVirtualRowMeasurement],
    ) {
        if rows.is_empty() {
            return;
        }
        let nodes = self.virtual_timeline.surface.nodes();
        let mut measurements = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(node) = nodes.get(row.source_index) else {
                continue;
            };
            measurements.push(VirtualMeasuredLayout::new(
                node.id,
                node.revision,
                row.height,
                0.0,
                row.visual_line_count,
            ));
            for hidden_index in row.source_index.saturating_add(1)..=row.source_end_index
            {
                let Some(hidden) = nodes.get(hidden_index) else {
                    break;
                };
                measurements.push(VirtualMeasuredLayout::new(
                    hidden.id,
                    hidden.revision,
                    0.0,
                    0.0,
                    0,
                ));
            }
        }
        let _ = self
            .virtual_timeline
            .surface
            .apply(VirtualSurfaceCommand::CommitMeasuredLayouts(measurements));
    }

    pub fn timeline_scrollbar_state(&self) -> Option<(f32, f32, f32, Option<Instant>)> {
        let viewport_h = self.timeline_viewport_height_px.max(0.0);
        let content_h = self.timeline_content_height_px.max(0.0);
        if viewport_h <= 0.0 || content_h <= viewport_h {
            return None;
        }
        Some((
            self.timeline_scroll_px,
            content_h,
            viewport_h,
            self.timeline_last_scroll_at,
        ))
    }

    pub fn set_timeline_metrics(
        &mut self,
        viewport_rect: [f32; 4],
        content_height_px: f32,
        viewport_height_px: f32,
    ) {
        let old_max_scroll =
            (self.timeline_content_height_px - self.timeline_viewport_height_px).max(0.0);
        let old_scroll_top =
            (old_max_scroll - self.timeline_scroll_px).clamp(0.0, old_max_scroll);
        let was_following_bottom = self.timeline_scroll_px <= 1.0;
        self.timeline_viewport_rect = Some(viewport_rect);
        self.timeline_content_height_px = content_height_px.max(0.0);
        self.timeline_viewport_height_px = viewport_height_px.max(0.0);
        if let Some(anchor) = self.pending_timeline_anchor {
            let keep_anchor = self.tool_expansion_is_animating();
            self.apply_timeline_anchor(anchor);
            if !keep_anchor {
                self.pending_timeline_anchor = None;
            }
        } else if let Some(previous_height) = self.pending_timeline_prepend_height_px {
            let max_scroll = self.max_timeline_scroll();
            let inserted_height =
                (self.timeline_content_height_px - previous_height).max(0.0);
            if max_scroll <= 0.0 {
                self.timeline_scroll_px = 0.0;
                self.timeline_velocity_px_s = 0.0;
                self.timeline_last_tick_at = None;
            } else if inserted_height > 0.0 {
                let keep_scroll_top = old_scroll_top + inserted_height;
                self.timeline_scroll_px =
                    (max_scroll - keep_scroll_top).clamp(0.0, max_scroll);
                self.pending_timeline_prepend_height_px = None;
            } else {
                self.clamp_timeline_scroll();
            }
        } else {
            let max_scroll = self.max_timeline_scroll();
            if max_scroll <= 0.0 {
                self.timeline_scroll_px = 0.0;
                self.timeline_velocity_px_s = 0.0;
                self.timeline_last_tick_at = None;
            } else if was_following_bottom {
                self.timeline_scroll_px = 0.0;
            } else if old_max_scroll > 0.0 {
                self.timeline_scroll_px =
                    (max_scroll - old_scroll_top).clamp(0.0, max_scroll);
            } else {
                self.clamp_timeline_scroll();
            }
        }
    }

    pub fn timeline_contains_point(&self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.timeline_viewport_rect else {
            return false;
        };
        x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
    }

    /// Trackpad glide: the long-standing feel — 1:1 direct response plus a
    /// gentle glide that cuts off at 50 px/s so motion ends when the fingers
    /// do, without a lingering low-speed tail.
    pub(crate) const TIMELINE_TRACKPAD_DECAY_TAU: f32 = 0.28;
    pub(crate) const TIMELINE_TRACKPAD_STOP_PX_S: f32 = 50.0;
    /// External mouse wheel: each notch is animated (small immediate nudge,
    /// rest delivered by velocity) so it reads smooth instead of a lurch, but
    /// with a short half-life so the glide settles in ~0.25s rather than
    /// drifting on after the wheel stops.
    const TIMELINE_WHEEL_DECAY_TAU: f32 = 0.12;
    const TIMELINE_WHEEL_STOP_PX_S: f32 = 30.0;

    pub fn scroll_timeline_pixels(&mut self, delta_pixels: f32) -> bool {
        self.scroll_timeline_pixels_with_inertia(
            delta_pixels,
            delta_pixels,
            7.0,
            Self::TIMELINE_TRACKPAD_DECAY_TAU,
            Self::TIMELINE_TRACKPAD_STOP_PX_S,
        )
    }

    pub fn scroll_timeline_wheel_pixels(&mut self, delta_pixels: f32) -> bool {
        self.scroll_timeline_pixels_with_inertia(
            delta_pixels,
            delta_pixels * 0.2,
            12.0,
            Self::TIMELINE_WHEEL_DECAY_TAU,
            Self::TIMELINE_WHEEL_STOP_PX_S,
        )
    }

    pub(in crate::panels::agent_pane::state) fn scroll_timeline_pixels_with_inertia(
        &mut self,
        delta_pixels: f32,
        immediate: f32,
        velocity_multiplier: f32,
        decay_tau: f32,
        stop_px_s: f32,
    ) -> bool {
        let started = perf::now();
        if delta_pixels.abs() < f32::EPSILON {
            return false;
        }
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            self.timeline_velocity_px_s = 0.0;
            Self::log_timeline_scroll_perf(
                delta_pixels,
                0.0,
                max_scroll,
                self.timeline_scroll_px,
                self.timeline_scroll_px,
                self.timeline_velocity_px_s,
                perf::elapsed_us(started),
            );
            return false;
        }
        // Direct nudge for instant response, plus a velocity injection so the
        // scroll keeps gliding after the wheel/touchpad event ends. External
        // mouse wheels use a tiny nudge and a stronger glide; precision
        // trackpads keep the existing 1:1 direct response.
        let at_top = self.timeline_scroll_px >= max_scroll - 0.5;
        let at_bottom = self.timeline_scroll_px <= 0.5;
        let before = self.timeline_scroll_px;
        self.timeline_scroll_px =
            (self.timeline_scroll_px + immediate).clamp(0.0, max_scroll);
        self.pending_timeline_anchor = None;
        // If the user was holding the edge, don't keep building velocity —
        // they can't move further that way and the inertia would feel sticky.
        if (delta_pixels > 0.0 && !at_top) || (delta_pixels < 0.0 && !at_bottom) {
            // Match markdown pane wheel inertia so agent and rendered notes
            // share the same scroll acceleration.
            let injected = delta_pixels * velocity_multiplier;
            self.timeline_velocity_px_s =
                (self.timeline_velocity_px_s + injected).clamp(-2800.0, 2800.0);
            self.timeline_scroll_decay_tau = decay_tau;
            self.timeline_scroll_stop_px_s = stop_px_s;
            self.timeline_last_tick_at.get_or_insert_with(Instant::now);
        } else {
            self.timeline_velocity_px_s = 0.0;
        }
        self.timeline_last_scroll_at = Some(Instant::now());
        Self::log_timeline_scroll_perf(
            delta_pixels,
            immediate,
            max_scroll,
            before,
            self.timeline_scroll_px,
            self.timeline_velocity_px_s,
            perf::elapsed_us(started),
        );
        true
    }

    pub fn scroll_timeline_half_page(&mut self, older_history: bool) -> bool {
        let delta =
            ctrl_u_d_scroll_delta(self.timeline_viewport_height_px, older_history);
        self.scroll_timeline_pixels(delta)
    }

    /// 1:1 touch drag: move the timeline exactly with the finger.
    /// Unlike `scroll_timeline_pixels` there is NO velocity injection —
    /// mid-drag inertia would make the content outrun the finger. The
    /// glide comes from `fling_timeline` when the finger lifts.
    pub fn drag_timeline_pixels(&mut self, delta_pixels: f32) -> bool {
        if delta_pixels.abs() < f32::EPSILON {
            return false;
        }
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            self.timeline_velocity_px_s = 0.0;
            return false;
        }
        let before = self.timeline_scroll_px;
        self.timeline_scroll_px = (before + delta_pixels).clamp(0.0, max_scroll);
        self.pending_timeline_anchor = None;
        self.timeline_velocity_px_s = 0.0;
        self.timeline_last_tick_at = None;
        self.timeline_last_scroll_at = Some(Instant::now());
        (self.timeline_scroll_px - before).abs() > f32::EPSILON
    }

    /// Start a kinetic glide at `velocity_px_s` (positive = up into
    /// history, matching `scroll_timeline_pixels`), or stop the current
    /// glide with `0.0`. Returns whether the timeline was gliding
    /// before the call so touch hosts can swallow the tap that stopped
    /// a fling instead of treating it as a click.
    pub fn fling_timeline(&mut self, velocity_px_s: f32) -> bool {
        // Report "was gliding" only for motion the user can actually
        // SEE — `timeline_is_inertial`'s 4 px/s floor also catches
        // residual velocity that never produced a visible glide, and a
        // false positive here makes the host swallow a legitimate tap
        // (e.g. the tap that should focus the input and summon the
        // mobile keyboard).
        let was_gliding = self.timeline_velocity_px_s.abs() >= 120.0;
        // Touch flicks are faster than wheel notches; allow a stronger
        // launch than the wheel path's ±2800 but keep it bounded.
        let velocity = velocity_px_s.clamp(-6000.0, 6000.0);
        self.timeline_velocity_px_s = velocity;
        if velocity.abs() >= f32::EPSILON {
            self.timeline_last_tick_at = Some(Instant::now());
            self.timeline_last_scroll_at = Some(Instant::now());
        } else {
            self.timeline_last_tick_at = None;
        }
        was_gliding
    }

    pub fn tick_timeline_scroll(&mut self) -> bool {
        // Velocity-based stop threshold — refresh-rate independent, tuned per
        // input device at injection time so neither trackpads nor wheels keep
        // crawling once the deliberate part of the motion has ended.
        if self.timeline_velocity_px_s.abs() < self.timeline_scroll_stop_px_s {
            self.timeline_velocity_px_s = 0.0;
            self.timeline_last_tick_at = None;
            return false;
        }
        let now = Instant::now();
        let dt = self
            .timeline_last_tick_at
            .map(|last| now.saturating_duration_since(last).as_secs_f32().min(0.05))
            .unwrap_or(0.016);
        self.timeline_last_tick_at = Some(now);
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            self.timeline_velocity_px_s = 0.0;
            return false;
        }
        // Exponential decay with the half-life chosen by the last gesture.
        let decay = (-dt / self.timeline_scroll_decay_tau.max(0.01)).exp();
        self.timeline_velocity_px_s *= decay;
        let step = self.timeline_velocity_px_s * dt;
        let next = (self.timeline_scroll_px + step).clamp(0.0, max_scroll);
        if (next - self.timeline_scroll_px).abs() < f32::EPSILON {
            // Hit an edge — kill remaining momentum so we don't burn frames.
            self.timeline_velocity_px_s = 0.0;
            self.timeline_last_tick_at = None;
            return false;
        }
        self.timeline_scroll_px = next;
        self.timeline_last_scroll_at = Some(now);
        true
    }

    pub fn timeline_is_inertial(&self) -> bool {
        self.timeline_velocity_px_s.abs() >= self.timeline_scroll_stop_px_s
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::panels::agent_pane::state) fn log_timeline_scroll_perf(
        delta_pixels: f32,
        immediate: f32,
        max_scroll: f32,
        before: f32,
        after: f32,
        velocity_px_s: f32,
        elapsed_us: Option<u128>,
    ) {
        if !perf::enabled() {
            return;
        }
        tracing::info!(
            target: "neoism::agent_ui_perf",
            delta_pixels,
            immediate,
            max_scroll,
            before,
            after,
            velocity_px_s,
            elapsed_us,
            "agent timeline scroll"
        );
    }

    /// Cache the scrollbar rects from the renderer so the input layer can
    /// hit-test them on mouse press without re-deriving the geometry.
    pub fn set_scrollbar_geometry(
        &mut self,
        track: Option<[f32; 4]>,
        thumb: Option<[f32; 4]>,
    ) {
        self.scrollbar_track_rect = track;
        self.scrollbar_thumb_rect = thumb;
    }

    pub fn scrollbar_dragging(&self) -> bool {
        self.scrollbar_drag.is_some()
    }

    pub fn scrollbar_hit(&self, x: f32, y: f32) -> Option<ScrollbarHit> {
        let track = self.scrollbar_track_rect?;
        if !interaction_policy::rect_contains(track, x, y) {
            return None;
        }
        if let Some(thumb) = self.scrollbar_thumb_rect {
            if interaction_policy::rect_contains(thumb, x, y) {
                return Some(ScrollbarHit::Thumb);
            }
        }
        Some(ScrollbarHit::Track)
    }

    pub fn begin_scrollbar_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(hit) = self.scrollbar_hit(x, y) else {
            return false;
        };
        // Page-jump when the user clicks the track outside the thumb so it
        // matches the conventional scrollbar behaviour.
        if hit == ScrollbarHit::Track {
            self.jump_timeline_to_track_y(y);
        }
        self.scrollbar_drag = Some(ScrollbarDrag {
            pointer_start_y: y,
            scroll_offset_start: self.timeline_scroll_px,
        });
        // Kill any in-flight kinetic scroll so the drag is responsive.
        self.pending_timeline_anchor = None;
        self.timeline_velocity_px_s = 0.0;
        true
    }

    pub fn drag_scrollbar_to(&mut self, _x: f32, y: f32) -> bool {
        let Some(drag) = self.scrollbar_drag else {
            return false;
        };
        let Some(track) = self.scrollbar_track_rect else {
            return false;
        };
        let Some(thumb) = self.scrollbar_thumb_rect else {
            return false;
        };
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            return false;
        }
        let travel = (track[3] - thumb[3]).max(1.0);
        // The thumb sits *higher* (smaller y) as `timeline_scroll_px`
        // grows (we scroll up to older content). Dragging down therefore
        // decreases `timeline_scroll_px`.
        let pointer_delta = y - drag.pointer_start_y;
        let scroll_delta = -(pointer_delta / travel) * max_scroll;
        let next = (drag.scroll_offset_start + scroll_delta).clamp(0.0, max_scroll);
        if (next - self.timeline_scroll_px).abs() < f32::EPSILON {
            return false;
        }
        self.timeline_scroll_px = next;
        self.pending_timeline_anchor = None;
        self.timeline_last_scroll_at = Some(Instant::now());
        true
    }

    pub fn end_scrollbar_drag(&mut self) -> bool {
        if self.scrollbar_drag.take().is_some() {
            self.timeline_last_scroll_at = Some(Instant::now());
            true
        } else {
            false
        }
    }

    pub(in crate::panels::agent_pane::state) fn jump_timeline_to_track_y(&mut self, y: f32) {
        let Some(track) = self.scrollbar_track_rect else {
            return;
        };
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            return;
        }
        let progress = ((y - track[1]) / track[3].max(1.0)).clamp(0.0, 1.0);
        // Higher y → further down the visible content → less `timeline_scroll_px`.
        let scroll_top = progress * max_scroll;
        self.timeline_scroll_px = (max_scroll - scroll_top).clamp(0.0, max_scroll);
        self.pending_timeline_anchor = None;
        self.timeline_last_scroll_at = Some(Instant::now());
    }

    pub fn has_conversation(&self) -> bool {
        self.messages
            .iter()
            .any(|message| !matches!(message.kind, NeoismAgentMessageKind::System))
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn refresh_model_context_limit(&mut self) {
        // In-memory: nothing to do without the response. Host fetches
        // the limit and feeds it back through the snapshot path.
        self.push_outbound(OutboundAgentCommand::RefreshModelContextLimit);
    }

    pub fn latest_usage(&self) -> Option<NeoismAgentUsage> {
        let mut usage = self
            .messages
            .iter()
            .rev()
            .find_map(|message| message.usage.clone())?;
        if usage.context_limit.is_none() {
            usage.context_limit = self.model_context_limit;
        }
        Some(usage)
    }

    pub fn usage_summary_label(&self) -> Option<String> {
        let usage = self.latest_usage()?;
        let total_cost = self.total_usage_cost_micros();
        Some(usage_summary_label((&usage).into(), total_cost))
    }

    pub fn usage_detail_lines(&self) -> Vec<String> {
        let Some(usage) = self.latest_usage() else {
            return Vec::new();
        };
        usage_detail_lines(
            (&usage).into(),
            self.total_usage_cost_micros(),
            self.model(),
        )
    }

    pub(in crate::panels::agent_pane::state) fn total_usage_cost_micros(&self) -> u64 {
        self.messages
            .iter()
            .filter_map(|message| message.usage.as_ref())
            .fold(0_u64, |sum, usage| sum.saturating_add(usage.cost_micros))
    }

    pub fn drain_server_updates(&mut self) -> bool {
        false
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn drain_background_updates(&mut self) -> bool {
        false
    }

    pub(in crate::panels::agent_pane) fn push_notice(
        &mut self,
        message: impl Into<String>,
        level: NeoismAgentNoticeLevel,
    ) {
        let message = message.into();
        if message.trim().is_empty() {
            return;
        }
        self.ui_events
            .push(NeoismAgentUiEvent::Notice { message, level });
    }

    /// Surface a Neoism-style "Copied" notification — fires after a
    /// drag-to-select copy lands in the clipboard.
    pub fn push_copied_notice(&mut self, char_count: usize) {
        let message = if char_count == 1 {
            "Copied 1 char to clipboard".to_string()
        } else {
            format!("Copied {char_count} chars to clipboard")
        };
        self.push_notice(message, NeoismAgentNoticeLevel::Info);
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn push_dialog(
        &mut self,
        title: impl Into<String>,
        body: impl Into<String>,
    ) {
        let title = title.into();
        let body = body.into();
        if body.trim().is_empty() {
            return;
        }
        self.ui_events
            .push(NeoismAgentUiEvent::Dialog { title, body });
    }

    pub(in crate::panels::agent_pane) fn request_close_tab(&mut self) {
        self.ui_events.push(NeoismAgentUiEvent::CloseTab);
    }

    pub(in crate::panels::agent_pane::state) fn max_timeline_scroll(&self) -> f32 {
        (self.timeline_content_height_px - self.timeline_viewport_height_px).max(0.0)
    }

    pub(in crate::panels::agent_pane::state) fn clamp_timeline_scroll(&mut self) {
        self.timeline_scroll_px = self
            .timeline_scroll_px
            .clamp(0.0, self.max_timeline_scroll());
    }

    pub(in crate::panels::agent_pane) fn invalidate_timeline_layout(&mut self) {
        self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
        self.timeline_content_revision = self.timeline_content_revision.wrapping_add(1);
        self.timeline_dirty_message_ids.clear();
        self.timeline_dirty_message_indices.clear();
        *self.timeline_layout_cache.borrow_mut() = None;
    }

    pub(in crate::panels::agent_pane::state) fn retain_current_turn_trace(&mut self) {
        if self.timeline_live_trace_start.is_some() {
            return;
        }
        self.timeline_live_trace_start = Some(
            self.messages
                .iter()
                .rposition(|message| message.kind == NeoismAgentMessageKind::User)
                .map_or(0, |index| index + 1),
        );
        // Rows hidden in the settled projection may now be visible, including
        // progress text that arrived before the first tool/reasoning item.
        self.invalidate_timeline_layout();
    }

    pub(in crate::panels::agent_pane) fn timeline_live_trace_start(&self) -> Option<usize> {
        self.timeline_live_trace_start
    }

    pub(in crate::panels::agent_pane::state) fn mark_timeline_message_dirty_at(&mut self, index: usize) {
        self.refresh_background_task_activity_clock();
        self.timeline_dirty_message_indices.insert(index);
        self.timeline_content_revision = self.timeline_content_revision.wrapping_add(1);
    }

    pub(in crate::panels::agent_pane::state) fn mark_timeline_message_and_next_dirty_at(&mut self, index: usize) {
        self.refresh_background_task_activity_clock();
        self.timeline_dirty_message_indices.insert(index);
        self.timeline_dirty_message_indices
            .insert(index.saturating_add(1));
        self.timeline_content_revision = self.timeline_content_revision.wrapping_add(1);
    }

    pub(in crate::panels::agent_pane::state) fn tool_expansion_is_animating(&self) -> bool {
        self.tool_expand_anims.values().any(|anim| anim.is_active())
    }

    pub(in crate::panels::agent_pane::state) fn apply_timeline_anchor(&mut self, anchor: TimelineAnchor) {
        let max_scroll = self.max_timeline_scroll();
        if max_scroll <= 0.0 {
            self.timeline_scroll_px = 0.0;
            self.timeline_velocity_px_s = 0.0;
            self.timeline_last_tick_at = None;
            return;
        }
        let viewport_y = self
            .timeline_viewport_rect
            .map(|rect| rect[1])
            .unwrap_or(0.0);
        let scroll_top =
            (anchor.content_y - (anchor.screen_y - viewport_y)).clamp(0.0, max_scroll);
        self.timeline_scroll_px = (max_scroll - scroll_top).clamp(0.0, max_scroll);
        self.timeline_velocity_px_s = 0.0;
        self.timeline_last_tick_at = None;
        self.timeline_last_scroll_at = Some(Instant::now());
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn start_session_updates(&mut self, _session_id: &str) {}

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn fail_compaction_message(&mut self, error: impl Into<String>) {
        self.finish_compaction_message("", "failed");
        self.note_streaming(NeoismAgentStreamingState::Idle, None);
        self.system_message("Compaction failed", error.into());
    }

    pub(in crate::panels::agent_pane) fn remember_pending_user_prompt(&mut self, text: &str) {
        if !text.trim().is_empty() {
            self.pending_user_prompts.push(text.to_string());
        }
    }

    pub(in crate::panels::agent_pane) fn clear_pending_user_prompts(&mut self) {
        self.pending_user_prompts.clear();
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn clear_composer(&mut self) {
        self.input.clear();
        self.cursor_byte = 0;
        self.input_attachments.clear();
        self.history_index = None;
        self.file_mention_anchor = None;
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn reset_session_runtime_ui(&mut self) {
        self.queued_prompt_count = 0;
        self.queued_prompt_preview = None;
        self.streaming_state = NeoismAgentStreamingState::Idle;
        self.streaming_started_at = None;
        self.streaming_state_changed_at = None;
        self.streaming_tool_label = None;
        self.subagent_waiting_started_at = None;
        self.active_subagent_ids.clear();
        self.active_subagent_started_at.clear();
        self.pending_permission = None;
        self.pending_permission_queue.clear();
        self.permission_choice_hit_rects.clear();
        self.timeline_live_trace_start = None;
    }

}
