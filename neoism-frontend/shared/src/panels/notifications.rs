// Rust-owned notifications surface. Replaces nvim's native message
// area (we already suppress it via shortmess+`F` and cmdheight=0) so
// our IDE's chrome owns every visible message — same pattern as the
// statusline / buffer-tabs / file-tree.
//
// Visual: stack of toast cards anchored to the top-right below the
// Rust chrome (Rio tabs, workspace tabs, breadcrumbs), fading out over
// `FADE_MS` after `LIFETIME_MS`. Progress toasts stay opaque and animate
// until explicitly finished. Pure view: a dispatcher pushes
// (`message`, `level`) and we handle layout + expiry. Drawn
// unconditionally each frame from `Renderer::run` so `is_active()` only
// governs whether we ask for redraws.

use unicode_segmentation::UnicodeSegmentation;
use web_time::Duration;
use web_time::Instant;

use sugarloaf::text::DrawOpts;
use crate::primitives::paint::Paint;

use crate::primitives::IdeTheme;

const TOAST_WIDTH: f32 = 320.0;
const TOAST_PADDING_X: f32 = 14.0;
const TOAST_PADDING_Y: f32 = 10.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_RADIUS: f32 = 8.0;
const FONT_SIZE: f32 = 12.0;
const ACCENT_WIDTH: f32 = 3.0;
const RIGHT_MARGIN: f32 = 24.0;
const TOP_OFFSET: f32 = 16.0;
const MAX_VISIBLE: usize = 5;
const LIFETIME_MS: u128 = 4_000;
const FADE_MS: u128 = 350;
/// Upper bound on wrapped lines per toast so a pathologically long message
/// can't grow a single card off-screen; the last kept line is ellipsised.
const MAX_TOAST_LINES: usize = 10;

const DEPTH: f32 = 0.0;
// Toasts are top-right system messages and must read clearly over
// WHATEVER page is behind them — file tree, palette, context menus, the
// chrome topbar menu (ORDER 30-33), modals (24), even the neodraw
// overlay sits at 200. We draw in a high band so the toast layer is
// never occluded by page chrome, and we paint an OPAQUE base under the
// surface tint (see `render`) so the toast is never see-through. The
// per-frame fade `alpha` still scales these for the fade-out animation.
const ORDER: u8 = 190;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Warn,
    Error,
}

impl NotificationLevel {
    fn accent(self, theme: &IdeTheme) -> [f32; 4] {
        match self {
            NotificationLevel::Info => theme.f32(theme.accent),
            NotificationLevel::Warn => theme.f32(theme.yellow),
            NotificationLevel::Error => theme.f32(theme.red),
        }
    }
}

/// Opaque identity for a persistent indeterminate progress toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressHandle(u64);

struct Toast {
    progress: Option<ProgressHandle>,
    message: String,
    level: NotificationLevel,
    created: Instant,
    paused_for: Duration,
    hover_started: Option<Instant>,
    wrapped_lines: Vec<String>,
}

pub struct Notifications {
    toasts: Vec<Toast>,
    /// Multiplier applied to font / padding so toasts grow with
    /// Ctrl+/Ctrl- font zoom alongside the rest of the chrome.
    scale: f32,
    next_progress_id: u64,
}

impl Notifications {
    pub fn new() -> Self {
        Self {
            toasts: Vec::new(),
            scale: 1.0,
            next_progress_id: 0,
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.5, 3.0);
        if self.scale != scale {
            self.scale = scale;
            for toast in &mut self.toasts {
                toast.wrapped_lines.clear();
            }
        }
    }

    /// Push a fresh toast. Older toasts past MAX_VISIBLE are dropped
    /// from the front so the stack never grows unbounded.
    ///
    /// **Dedupe**: if an existing visible toast has the same level +
    /// message AND was emitted within `DEDUPE_WINDOW`, we refresh that
    /// toast's `created` timestamp instead of pushing a duplicate.
    /// Several editor surfaces (nvim msg events, LSP autoreload pings,
    /// retry routines) all bounce the same string through this path in
    /// the same frame; without this the user sees stacked twin toasts.
    pub fn push(&mut self, message: impl Into<String>, level: NotificationLevel) {
        const DEDUPE_WINDOW: Duration = Duration::from_millis(1500);
        let message = message.into();
        if message.is_empty() {
            return;
        }
        let now = Instant::now();
        if let Some(existing) = self.toasts.iter_mut().rev().find(|t| {
            t.progress.is_none()
                && t.level == level
                && t.message == message
                && now.saturating_duration_since(t.created) <= DEDUPE_WINDOW
        }) {
            existing.created = now;
            existing.paused_for = Duration::ZERO;
            return;
        }
        self.toasts.push(Toast {
            progress: None,
            message,
            level,
            created: now,
            paused_for: Duration::ZERO,
            hover_started: None,
            wrapped_lines: Vec::new(),
        });
        self.trim_transient_toasts();
    }

    /// Start indeterminate progress. It neither expires nor deduplicates with
    /// ordinary messages; the owner must finish it (or drop this window).
    pub fn start_progress(&mut self, message: impl Into<String>) -> ProgressHandle {
        let handle = ProgressHandle(self.next_progress_id);
        self.next_progress_id = self
            .next_progress_id
            .checked_add(1)
            .expect("progress handle exhausted");
        self.toasts.push(Toast {
            progress: Some(handle),
            message: message.into(),
            level: NotificationLevel::Info,
            created: Instant::now(),
            paused_for: Duration::ZERO,
            hover_started: None,
            wrapped_lines: Vec::new(),
        });
        self.trim_transient_toasts();
        handle
    }

    /// Remove only this operation’s toast. Repeated completion is harmless.
    pub fn finish_progress(&mut self, handle: ProgressHandle) {
        self.toasts.retain(|toast| toast.progress != Some(handle));
    }

    fn trim_transient_toasts(&mut self) {
        // Never evict an in-flight operation just because other messages arrive.
        while self.toasts.len() > MAX_VISIBLE {
            let Some(index) = self
                .toasts
                .iter()
                .position(|toast| toast.progress.is_none())
            else {
                break;
            };
            self.toasts.remove(index);
        }
    }

    /// `true` while at least one toast needs animation or fade/expiry frames.
    /// Ordinary hovered toasts pause expiry without requesting frames;
    /// indeterminate progress continues animating even when hovered.
    pub fn is_active(&self) -> bool {
        let now = Instant::now();
        self.toasts.iter().any(|t| {
            t.progress.is_some()
                || (t.hover_started.is_none()
                    && visible_age(t, now) < LIFETIME_MS + FADE_MS)
        })
    }

    /// Drop expired toasts. Cheap; safe to call once per frame from
    /// `Renderer::run`.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|t| {
            t.progress.is_some()
                || t.hover_started.is_some()
                || visible_age(t, now) < LIFETIME_MS + FADE_MS
        });
    }

    pub fn hover(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
        top_offset: f32,
    ) -> bool {
        self.tick();
        let now = Instant::now();
        let hovered =
            self.hit_test(mouse_x, mouse_y, window_width, scale_factor, top_offset);
        let mut changed = false;

        for (idx, toast) in self.toasts.iter_mut().enumerate() {
            let should_hover = hovered == Some(idx);
            match (should_hover, toast.hover_started) {
                (true, None) => {
                    toast.hover_started = Some(now);
                    changed = true;
                }
                (false, Some(started)) => {
                    toast.paused_for += now.saturating_duration_since(started);
                    toast.hover_started = None;
                    changed = true;
                }
                _ => {}
            }
        }

        changed
    }

    pub fn clear_hover(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for toast in &mut self.toasts {
            if let Some(started) = toast.hover_started.take() {
                toast.paused_for += now.saturating_duration_since(started);
                changed = true;
            }
        }
        changed
    }

    fn hit_test(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        window_width: f32,
        scale_factor: f32,
        top_offset: f32,
    ) -> Option<usize> {
        let logical_w = window_width / scale_factor;
        let toast_w = (TOAST_WIDTH * self.scale).min((logical_w-2.0*RIGHT_MARGIN*self.scale).max(1.0));
        let pad_x = TOAST_PADDING_X * self.scale;
        let pad_y = TOAST_PADDING_Y * self.scale;
        let gap = TOAST_GAP * self.scale;
        let font_size = FONT_SIZE * self.scale;
        let right_margin = RIGHT_MARGIN * self.scale;
        let available_text_w = toast_w - ACCENT_WIDTH * self.scale - pad_x * 2.0;
        let x = (logical_w - toast_w - right_margin).max(0.0);
        let mut y = top_offset + TOP_OFFSET * self.scale;
        let now = Instant::now();

        for (idx, toast) in self.toasts.iter().enumerate() {
            // Height varies per toast now that long messages wrap — use the
            // same wrap the renderer does so hover/hit-testing lines up.
            let line_count = if toast.wrapped_lines.is_empty() {
                wrap_message_measured(
                    &toast.message,
                    available_text_w
                        - progress_inset(toast.progress.is_some(), self.scale),
                    |text| text.graphemes(true).count() as f32 * font_size * 0.6,
                )
                .len()
            } else {
                toast.wrapped_lines.len()
            };
            let toast_h = toast_height(line_count, font_size, pad_y);
            // Fully-faded toasts are skipped WITHOUT advancing `y`, matching
            // the renderer (which `continue`s on alpha<=0 before its `y +=`),
            // so hit-testing stays aligned with what's actually drawn.
            if toast.progress.is_none()
                && visible_age(toast, now) >= LIFETIME_MS + FADE_MS
                && toast.hover_started.is_none()
            {
                continue;
            }
            if mouse_x >= x
                && mouse_x <= x + toast_w
                && mouse_y >= y
                && mouse_y <= y + toast_h
            {
                return Some(idx);
            }
            y += toast_h + gap;
        }

        None
    }

    pub fn host_messages(&self)->Vec<String>{self.toasts.iter().map(|toast|toast.message.clone()).collect()}
    pub fn dismiss(&mut self,index:usize){if index<self.toasts.len(){self.toasts.remove(index);}}
    /// Last measured toast geometry for another native host's pointer regions.
    pub fn host_rects(&self,width:f32,top:f32)->Vec<[f32;4]>{
        let toast_w=(TOAST_WIDTH*self.scale).min((width-2.0*RIGHT_MARGIN*self.scale).max(1.0));
        let x=(width-toast_w-RIGHT_MARGIN*self.scale).max(0.0);let mut y=top+TOP_OFFSET*self.scale;
        self.toasts.iter().map(|toast|{let h=toast_height(toast.wrapped_lines.len().max(1),FONT_SIZE*self.scale,TOAST_PADDING_Y*self.scale);let rect=[x,y,toast_w,h];y+=h+TOAST_GAP*self.scale;rect}).collect()
    }

    /// Draw all live toasts. `(window_width, _, scale_factor)` matches
    /// the dimensions tuple used by the rest of the overlays so the
    /// caller doesn't need to remember a fourth signature.
    pub fn render(
        &mut self,
        sugarloaf: &mut (impl Paint + ?Sized),
        dimensions: (f32, f32, f32),
        top_offset: f32,
        theme: &IdeTheme,
    ) {
        self.tick();
        if self.toasts.is_empty() {
            return;
        }

        let (window_width, _window_height, scale_factor) = dimensions;
        let logical_w = window_width / scale_factor;

        let toast_w = (TOAST_WIDTH * self.scale).min((logical_w-2.0*RIGHT_MARGIN*self.scale).max(1.0));
        let pad_x = TOAST_PADDING_X * self.scale;
        let pad_y = TOAST_PADDING_Y * self.scale;
        let gap = TOAST_GAP * self.scale;
        let font_size = FONT_SIZE * self.scale;
        let right_margin = RIGHT_MARGIN * self.scale;
        let mut y = top_offset + TOP_OFFSET * self.scale;
        let x = (logical_w - toast_w - right_margin).max(0.0);

        // Snapshot so the borrow checker tolerates the mut sugarloaf
        // calls inside the loop alongside the per-toast field reads.
        let now = Instant::now();
        let available_text_w = toast_w - ACCENT_WIDTH * self.scale - pad_x * 2.0;
        let measure_opts = DrawOpts {
            font_size,
            ..DrawOpts::default()
        };
        let snapshot: Vec<(Vec<String>, NotificationLevel, u128, bool)> = {
            let ui = sugarloaf.overlay_text_mut();
            self.toasts
                .iter_mut()
                .map(|toast| {
                    let lines = wrap_message_measured(
                        &toast.message,
                        available_text_w
                            - progress_inset(toast.progress.is_some(), self.scale),
                        |text| ui.measure(text, &measure_opts),
                    );
                    toast.wrapped_lines.clone_from(&lines);
                    (
                        lines,
                        toast.level,
                        visible_age(toast, now),
                        toast.progress.is_some(),
                    )
                })
                .collect()
        };

        for (lines, level, age, progress) in snapshot {
            // Linear fade once we cross LIFETIME_MS — clamps so that
            // the very last frame paints at zero alpha rather than a
            // sudden snap-out.
            let alpha = if progress || age <= LIFETIME_MS {
                1.0
            } else {
                let into_fade = age - LIFETIME_MS;
                if into_fade >= FADE_MS {
                    0.0
                } else {
                    1.0 - (into_fade as f32 / FADE_MS as f32)
                }
            };
            if alpha <= 0.0 {
                continue;
            }

            // Long messages wrap across multiple lines so the whole toast
            // is readable (install-failure reasons, LSP errors, etc.). The
            // card grows to fit; `toast_height` mirrors the same wrap used by
            // `hit_test` so hover/expiry line up.
            let toast_h = toast_height(lines.len(), font_size, pad_y);

            // Background card with corner radius. The toast must NEVER be
            // see-through over the page behind it. We draw on Sugarloaf's
            // LATE OVERLAY pass (`overlay_*`) — the same mechanism the
            // chrome topbar menu uses — so the card is composited AFTER the
            // page's normal UI text and quads, never bleeding the page
            // through. We also lay an OPAQUE base (theme.bg) under the
            // `surface` chrome tint: the old single 0.95-alpha `surface`
            // rect on the regular pass was what let content show through.
            // `alpha` here is only the fade-out animation multiplier.
            let mut base = theme.f32(theme.bg);
            base[3] = alpha;
            sugarloaf.overlay_rounded_rect(
                x,
                y,
                toast_w,
                toast_h,
                base,
                DEPTH,
                TOAST_RADIUS,
                ORDER,
            );
            let mut bg = theme.f32(theme.surface);
            bg[3] = alpha;
            sugarloaf.overlay_rounded_rect(
                x,
                y,
                toast_w,
                toast_h,
                bg,
                DEPTH,
                TOAST_RADIUS,
                ORDER + 1,
            );

            // Accent bar on the left edge — color carries severity. Sits
            // above the surface tint (ORDER + 1) so it stays crisp.
            let mut accent = level.accent(theme);
            accent[3] *= alpha;
            sugarloaf.overlay_rect(
                x,
                y,
                ACCENT_WIDTH * self.scale,
                toast_h,
                accent,
                DEPTH,
                ORDER + 2,
            );

            let text_color = match level {
                NotificationLevel::Info => theme.u8(theme.fg),
                NotificationLevel::Warn | NotificationLevel::Error => theme.u8(theme.dim),
            };
            let mut text_color = text_color;
            // Apply alpha to text by scaling the alpha byte. Text
            // shaping doesn't read premul; the renderer handles it.
            text_color[3] = (text_color[3] as f32 * alpha) as u8;

            let inset = progress_inset(progress, self.scale);
            let text_x = x + ACCENT_WIDTH * self.scale + pad_x + inset;
            if progress {
                let now = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .unwrap_or_default();
                let phase = crate::render_policy::animation_phase_from_unix_secs(
                    now.as_secs(),
                    now.subsec_nanos(),
                );
                for (trail, alpha) in crate::render_policy::RUNNING_SQUARE_ALPHAS
                    .into_iter()
                    .enumerate()
                {
                    let (dx, dy) = crate::render_policy::running_square_position(
                        phase, trail, self.scale,
                    );
                    sugarloaf.overlay_rect(
                        text_x - inset * 0.5 + dx - self.scale,
                        y + pad_y + font_size * 0.5 + dy - self.scale,
                        2.0 * self.scale,
                        2.0 * self.scale,
                        theme.f32_alpha(theme.fg, alpha),
                        DEPTH,
                        ORDER + 2,
                    );
                }
            }
            let opts = DrawOpts {
                font_size,
                color: text_color,
                clip_rect: Some([text_x, y, available_text_w - inset, toast_h]),
                ..DrawOpts::default()
            };

            let line_h = line_height(font_size);
            // Overlay text pass: renders above the overlay quads above,
            // matching the chrome topbar menu's label path. Each wrapped line
            // stacks down from the top padding.
            let ui = sugarloaf.overlay_text_mut();
            let mut line_y = y + pad_y;
            for line in &lines {
                ui.draw(text_x, line_y, line, &opts);
                line_y += line_h;
            }

            y += toast_h + gap;
        }
    }
}

fn progress_inset(progress: bool, scale: f32) -> f32 {
    if progress {
        20.0 * scale
    } else {
        0.0
    }
}

fn visible_age(toast: &Toast, now: Instant) -> u128 {
    let paused = toast.paused_for
        + toast
            .hover_started
            .map(|started| now.saturating_duration_since(started))
            .unwrap_or_default();
    now.saturating_duration_since(toast.created)
        .saturating_sub(paused)
        .as_millis()
}

/// Vertical advance between wrapped lines.
fn line_height(font_size: f32) -> f32 {
    font_size * 1.35
}

/// Card height for a toast with `line_count` wrapped lines.
fn toast_height(line_count: usize, font_size: f32, pad_y: f32) -> f32 {
    let block = font_size + line_count.saturating_sub(1) as f32 * line_height(font_size);
    pad_y * 2.0 + block
}

/// Greedy word-wrap a message to fit `available_width`, capped at
/// `MAX_TOAST_LINES` (the last kept line is ellipsised when it overflows).
/// Honours explicit newlines; hard-splits words longer than one line so a
/// single URL/path can't overflow the card. Replaces the old single-line
/// truncate-with-… + hover-scroll so long install/LSP errors are fully
/// readable.
fn wrap_message_measured(
    message: &str,
    available_width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw_line in message.split('\n') {
        wrap_one_line_measured(raw_line, available_width, &mut measure, &mut lines);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > MAX_TOAST_LINES {
        lines.truncate(MAX_TOAST_LINES);
        if let Some(last) = lines.last_mut() {
            while !last.is_empty() {
                let candidate = format!("{last}…");
                if measure(&candidate) <= available_width {
                    break;
                }
                let Some((index, _)) = last.grapheme_indices(true).next_back() else {
                    break;
                };
                last.truncate(index);
            }
            last.push('…');
        }
    }
    lines
}

fn wrap_one_line_measured(
    raw_line: &str,
    available_width: f32,
    measure: &mut impl FnMut(&str) -> f32,
    out: &mut Vec<String>,
) {
    if raw_line.trim().is_empty() {
        out.push(String::new());
        return;
    }
    let mut current = String::new();
    for word in raw_line.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= available_width {
            current = candidate;
            continue;
        }

        if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if measure(word) <= available_width {
            current.push_str(word);
            continue;
        }

        // Keep combining characters and emoji ZWJ sequences intact when a
        // path or URL has to be split without whitespace.
        let mut chunk = String::new();
        for grapheme in word.graphemes(true) {
            let candidate = format!("{chunk}{grapheme}");
            if !chunk.is_empty() && measure(&candidate) > available_width {
                out.push(std::mem::take(&mut chunk));
            }
            chunk.push_str(grapheme);
        }
        current = chunk;
    }
    if !current.is_empty() {
        out.push(current);
    }
}

impl Default for Notifications {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_survives_expiry_hover_and_notification_pressure() {
        let mut notifications = Notifications::new();
        let handle = notifications.start_progress("Starting Fast Server...");
        notifications.toasts[0].created = Instant::now() - Duration::from_secs(60);
        notifications.tick();
        assert_eq!(
            notifications.toasts.len(),
            1,
            "pending toast must not expire"
        );
        assert_eq!(
            notifications.hit_test(700.0, 20.0, 1000.0, 1.0, 0.0),
            Some(0)
        );
        notifications.toasts[0].hover_started = Some(Instant::now());
        for i in 0..10 {
            notifications.push(format!("Message {i}"), NotificationLevel::Info);
        }
        notifications.tick();
        assert!(
            notifications.is_active(),
            "hover must not freeze the loader"
        );
        assert_eq!(notifications.toasts.len(), MAX_VISIBLE);
        assert!(notifications
            .toasts
            .iter()
            .any(|t| t.progress == Some(handle)));
        notifications.finish_progress(handle);
        notifications.finish_progress(handle);
        assert!(notifications.toasts.iter().all(|t| t.progress.is_none()));
    }

    #[test]
    fn progress_handles_are_independent_and_stop_redraw_when_finished() {
        let mut notifications = Notifications::new();
        let first = notifications.start_progress("Starting Fast Server...");
        let second = notifications.start_progress("Starting Fast Server...");
        assert_ne!(first, second);
        notifications.finish_progress(first);
        assert_eq!(notifications.toasts.len(), 1);
        assert!(notifications.is_active());
        notifications.finish_progress(second);
        assert!(!notifications.is_active());
        assert!(notifications.toasts.is_empty());
    }

    #[test]
    fn ordinary_messages_do_not_refresh_progress_and_still_expire() {
        let mut notifications = Notifications::new();
        let handle = notifications.start_progress("Starting Fast Server...");
        notifications.push("Starting Fast Server...", NotificationLevel::Info);
        assert_eq!(notifications.toasts.len(), 2);
        notifications.toasts[1].created = Instant::now() - Duration::from_secs(60);
        notifications.tick();
        assert_eq!(notifications.toasts.len(), 1);
        assert_eq!(notifications.toasts[0].progress, Some(handle));
        assert_eq!(progress_inset(true, 2.0), 40.0);
        assert_eq!(progress_inset(false, 2.0), 0.0);
    }

    #[test]
    fn measured_wrap_keeps_the_regression_line_inside_the_card() {
        let width = TOAST_WIDTH - ACCENT_WIDTH - TOAST_PADDING_X * 2.0;
        let measure =
            |text: &str| text.graphemes(true).count() as f32 * FONT_SIZE * 0.5859375;
        let lines = wrap_message_measured(
            "Neoism web UI is not installed. Rebuild with the web assets or set NEOISM_WEB_ROOT.",
            width,
            measure,
        );

        assert!(lines.iter().all(|line| measure(line) <= width));
        assert!(lines.len() >= 2);
    }

    #[test]
    fn hard_wrap_does_not_split_graphemes() {
        let family = "👨‍👩‍👧‍👦";
        let lines = wrap_message_measured(&family.repeat(3), 10.0, |text| {
            text.graphemes(true).count() as f32 * 10.0
        });

        assert_eq!(lines, vec![family, family, family]);
    }
}
