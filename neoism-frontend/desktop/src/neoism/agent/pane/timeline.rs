use super::*;

impl NeoismAgentPane {
    pub fn drain_ui_events(&mut self) -> Vec<NeoismAgentUiEvent> {
        std::mem::take(&mut self.ui_events)
    }

    pub fn timeline_scroll_offset(&self) -> f32 {
        self.timeline_scroll_px
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
    pub(crate) const TIMELINE_WHEEL_DECAY_TAU: f32 = 0.12;
    pub(crate) const TIMELINE_WHEEL_STOP_PX_S: f32 = 30.0;

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

    pub(crate) fn scroll_timeline_pixels_with_inertia(
        &mut self,
        delta_pixels: f32,
        immediate: f32,
        velocity_multiplier: f32,
        decay_tau: f32,
        stop_px_s: f32,
    ) -> bool {
        let started = crate::neoism::agent::perf::now();
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
                crate::neoism::agent::perf::elapsed_us(started),
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
            // Compound velocity for consecutive flicks but keep the cap
            // and multiplier conservative — small touchpad swipes glide
            // without flying past the target.
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
            crate::neoism::agent::perf::elapsed_us(started),
        );
        true
    }

    pub fn scroll_timeline_half_page(&mut self, older_history: bool) -> bool {
        let delta =
            ctrl_u_d_scroll_delta(self.timeline_viewport_height_px, older_history);
        self.scroll_timeline_pixels(delta)
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
            .map(|last| now.duration_since(last).as_secs_f32().min(0.05))
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
    pub(crate) fn log_timeline_scroll_perf(
        delta_pixels: f32,
        immediate: f32,
        max_scroll: f32,
        before: f32,
        after: f32,
        velocity_px_s: f32,
        elapsed_us: Option<u128>,
    ) {
        if !crate::neoism::agent::perf::enabled() {
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn log_render_perf(
        &mut self,
        elapsed_us: Option<u128>,
        rect: [f32; 4],
        input_rect: [f32; 4],
        active: bool,
        ticked_scroll: bool,
        occlusion_count: usize,
        panel_bottom_override: Option<f32>,
        panel_top_override: Option<f32>,
    ) {
        if !crate::neoism::agent::perf::enabled() {
            return;
        }

        let now = Instant::now();
        let frame_delta_us = self
            .perf_frame
            .last_render_at
            .map(|previous| now.duration_since(previous).as_micros());
        self.perf_frame.last_render_at = Some(now);
        self.perf_frame.frames = self.perf_frame.frames.saturating_add(1);

        tracing::info!(
            target: "neoism::agent_ui_perf",
            frame = self.perf_frame.frames,
            elapsed_us = elapsed_us,
            frame_delta_us = frame_delta_us,
            target_165hz_us = 6060u64,
            active,
            ticked_scroll,
            animating = self.is_animating(),
            animation_reason = self.animation_reason(),
            streaming = ?self.streaming_state,
            messages = self.messages.len(),
            layout_epoch = self.timeline_layout_epoch,
            layout_cache = self.timeline_layout_cache.borrow().is_some(),
            dirty_ids = self.timeline_dirty_message_ids.len(),
            dirty_indices = self.timeline_dirty_message_indices.len(),
            measure_cache = self.timeline_measure_cache.borrow().len(),
            markdown_cache = self.markdown_blocks_cache.borrow().len(),
            scroll_px = self.timeline_scroll_px,
            velocity_px_s = self.timeline_velocity_px_s,
            content_height_px = self.timeline_content_height_px,
            viewport_height_px = self.timeline_viewport_height_px,
            rect_w = rect[2],
            rect_h = rect[3],
            input_w = input_rect[2],
            input_h = input_rect[3],
            occlusion_count,
            panel_bottom_override = ?panel_bottom_override,
            panel_top_override = ?panel_top_override,
            "agent pane render perf"
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

    pub(crate) fn jump_timeline_to_track_y(&mut self, y: f32) {
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
        self.session_id.is_some() || !self.messages.is_empty()
    }

    pub(crate) fn refresh_model_context_limit(&mut self) {
        self.push_outbound(OutboundAgentCommand::RefreshModelContextLimit);
    }

    pub(crate) fn execute_refresh_model_context_limit_command(&mut self) {
        self.model_context_limit =
            fetch_model_context_limit(&self.server, self.model.as_str())
                .ok()
                .flatten();
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
        Some(usage_policy::usage_summary_label(
            usage_snapshot(&usage),
            total_cost,
        ))
    }

    pub fn usage_detail_lines(&self) -> Vec<String> {
        let Some(usage) = self.latest_usage() else {
            return Vec::new();
        };
        usage_policy::usage_detail_lines(
            usage_snapshot(&usage),
            self.total_usage_cost_micros(),
            self.model(),
        )
    }

    pub(crate) fn total_usage_cost_micros(&self) -> u64 {
        self.messages
            .iter()
            .filter_map(|message| message.usage.as_ref())
            .fold(0_u64, |sum, usage| sum.saturating_add(usage.cost_micros))
    }
}
