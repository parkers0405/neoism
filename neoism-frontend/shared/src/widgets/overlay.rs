// Generic overlay scaffolding shared by context menus, popovers, modals,
// and the LSP completion menu. Owns the open/closed lifecycle, a fade
// (some methods aren't called yet — the widget is a shared library for
// the next migration pass; muting dead_code keeps the unused-method
// warning channel clean for actual problems).
#![allow(dead_code)]

// Owns the open/closed lifecycle, a fade
// animation phase, scrim alpha curve, and the ESC / click-outside
// dismissal primitives. Visuals (rounded rect, rows, anchor math) are
// the consumer's job — `Overlay<T>` only manages ephemeral state.
//
// The point isn't to render anything; it's to stop every overlay panel
// from reinventing "am I open?", "what's my fade timer?", "did the user
// press Esc?". Callers wrap their content `T` in an `Overlay<T>`, ask it
// for `anim_t()` when they paint, and forward keys + clicks through the
// dismissal helpers.

use web_time::Instant;

use crate::primitives::paint::Paint;

use crate::primitives::IdeTheme;

// TODO(wave-cutover): unify with crate::event::UiEvent::Key — the
// native code used `neoism_window::keyboard::{Key, NamedKey}` (winit
// types). For the lifted widget we mirror only the keys the original
// overlay/menu matched against, as a POD with no platform deps. The
// host translates incoming UiEvents into this enum before calling
// `handle_key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    Escape,
    Enter,
    Tab,
    ArrowUp,
    ArrowDown,
    PageUp,
    PageDown,
}

// TODO(wave-cutover): unify with crate::event::UiEvent::Key — see
// `NamedKey` above. `Character` lets us forward typed shortcut chars
// (e.g. y/n prompts) for `Menu::match_shortcut` without dragging in
// winit's `SmolStr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Named(NamedKey),
    Character(String),
}

/// Default fade-in / fade-out duration (milliseconds). Matches the
/// existing diagnostics popup so all overlays feel the same.
pub const DEFAULT_ANIM_MS: f32 = 160.0;

/// Default scrim alpha when fully open. 0.0 = no scrim, 1.0 = fully
/// opaque black. Most overlays use 0.0 (no scrim) so the default is
/// "off"; modals override.
pub const DEFAULT_SCRIM_ALPHA: f32 = 0.0;

/// Z-order the scrim renders at. Below the overlay body so the body
/// always sits on top.
const SCRIM_ORDER: u8 = 22;
const SCRIM_DEPTH: f32 = 0.05;

/// Lifecycle phase of an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayState {
    Closed,
    Opening,
    Open,
    Closing,
}

/// Result of a key event handed to the overlay. The consumer uses this
/// to know whether to forward the key further or stop propagation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKeyAction {
    /// The overlay consumed the key (e.g. ESC closed it).
    Consumed,
    /// The overlay ignored the key — caller should keep routing it.
    Ignored,
}

/// Generic overlay wrapper. `T` is the consumer's payload (menu items,
/// popover content, modal spec — anything). Overlay only manages the
/// open/closed/anim machinery; visuals are still up to the consumer.
pub struct Overlay<T> {
    content: T,
    state: OverlayState,
    phase_started: Instant,
    anim_ms: f32,
    scrim_alpha_open: f32,
}

impl<T> Overlay<T> {
    pub fn new(content: T) -> Self {
        Self {
            content,
            state: OverlayState::Closed,
            phase_started: Instant::now(),
            anim_ms: DEFAULT_ANIM_MS,
            scrim_alpha_open: DEFAULT_SCRIM_ALPHA,
        }
    }

    /// Override the open/close animation length. Useful for modals
    /// (slower fade) vs popovers (snappy).
    pub fn with_anim_ms(mut self, ms: f32) -> Self {
        self.anim_ms = ms.max(1.0);
        self
    }

    /// Override the scrim alpha at full open. Set to 0.0 (default) for
    /// no scrim; modals typically use ~0.35.
    pub fn with_scrim(mut self, alpha: f32) -> Self {
        self.scrim_alpha_open = alpha.clamp(0.0, 1.0);
        self
    }

    pub fn content(&self) -> &T {
        &self.content
    }

    pub fn content_mut(&mut self) -> &mut T {
        &mut self.content
    }

    pub fn replace_content(&mut self, content: T) {
        self.content = content;
    }

    pub fn state(&self) -> OverlayState {
        self.state
    }

    /// True for Opening | Open | Closing — anything that still draws.
    pub fn is_visible(&self) -> bool {
        self.state != OverlayState::Closed
    }

    /// True only when fully open and accepting input. Clicks during a
    /// fade should fall through to whatever's underneath, otherwise a
    /// mid-fade overlay could swallow taps that the user expected to
    /// land on the editor.
    pub fn is_interactive(&self) -> bool {
        self.state == OverlayState::Open
    }

    /// True while an animation is in flight — the renderer's
    /// `needs_redraw()` keeps ticking us until the fade lands.
    pub fn is_animating(&self) -> bool {
        matches!(self.state, OverlayState::Opening | OverlayState::Closing)
    }

    /// Snap to fully open immediately. Used by overlays that want no
    /// fade-in (legacy behavior in some panels). Most consumers should
    /// call `open()` instead.
    pub fn open_instant(&mut self) {
        self.state = OverlayState::Open;
        self.phase_started = Instant::now();
    }

    pub fn open(&mut self) {
        if self.state == OverlayState::Open {
            return;
        }
        // If we're mid-close, restart the open from the current alpha
        // so we don't jump. Cheap approximation: just reset the timer.
        self.state = OverlayState::Opening;
        self.phase_started = Instant::now();
    }

    pub fn close(&mut self) {
        if matches!(self.state, OverlayState::Closed | OverlayState::Closing) {
            return;
        }
        self.state = OverlayState::Closing;
        self.phase_started = Instant::now();
    }

    /// Snap shut without animating. Cleanup-only — preferred over
    /// `close()` when the host is tearing the widget down (e.g. closing
    /// the entire window).
    pub fn close_instant(&mut self) {
        self.state = OverlayState::Closed;
    }

    /// Advance the animation. Idempotent — safe to call every frame
    /// regardless of state.
    pub fn tick(&mut self, _dt: f32) {
        // We don't use `dt` because the phase timer is wall-clock based
        // (matches the existing panels and survives frame drops). The
        // parameter stays in the API for future spring-based motion.
        let elapsed_ms = Instant::now()
            .saturating_duration_since(self.phase_started)
            .as_secs_f32()
            * 1000.0;
        match self.state {
            OverlayState::Opening if elapsed_ms >= self.anim_ms => {
                self.state = OverlayState::Open;
            }
            OverlayState::Closing if elapsed_ms >= self.anim_ms => {
                self.state = OverlayState::Closed;
            }
            _ => {}
        }
    }

    /// Eased animation progress. 0.0 closed, 1.0 fully open. Use this
    /// to multiply colors / offsets so the overlay fades smoothly.
    pub fn anim_t(&self) -> f32 {
        let elapsed_ms = Instant::now()
            .saturating_duration_since(self.phase_started)
            .as_secs_f32()
            * 1000.0;
        let raw = match self.state {
            OverlayState::Opening => (elapsed_ms / self.anim_ms).clamp(0.0, 1.0),
            OverlayState::Open => 1.0,
            OverlayState::Closing => (1.0 - elapsed_ms / self.anim_ms).clamp(0.0, 1.0),
            OverlayState::Closed => 0.0,
        };
        ease_out_cubic(raw)
    }

    /// Convenience: current scrim alpha (0..1) given the configured
    /// open-alpha and the current eased animation progress.
    pub fn scrim_alpha(&self) -> f32 {
        self.scrim_alpha_open * self.anim_t()
    }

    /// Hand a key to the overlay. Returns `Consumed` when the overlay
    /// reacted (ESC always closes); otherwise the caller is responsible
    /// for routing the key to the content.
    pub fn handle_key(&mut self, key: &Key) -> OverlayKeyAction {
        if !self.is_visible() {
            return OverlayKeyAction::Ignored;
        }
        if matches!(key, Key::Named(NamedKey::Escape)) {
            self.close();
            return OverlayKeyAction::Consumed;
        }
        OverlayKeyAction::Ignored
    }

    /// Called when the host receives a click. If the click falls
    /// outside `own_rect`, the overlay starts closing and returns
    /// `true` (caller should treat the click as consumed by dismissal).
    /// Returns `false` for clicks inside the rect — caller routes them
    /// to the content normally.
    pub fn handle_click_outside(&mut self, point: [f32; 2], own_rect: [f32; 4]) -> bool {
        if !self.is_interactive() {
            return false;
        }
        let [px, py] = point;
        let [rx, ry, rw, rh] = own_rect;
        let inside = px >= rx && px <= rx + rw && py >= ry && py <= ry + rh;
        if !inside {
            self.close();
            return true;
        }
        false
    }

    /// Render the scrim behind the overlay body. No-op when the
    /// configured scrim alpha is 0. `viewport` = `[x, y, w, h]` of the
    /// area to dim (usually the whole window).
    pub fn draw_scrim(
        &self,
        sugarloaf: &mut dyn Paint,
        viewport: [f32; 4],
        theme: &IdeTheme,
    ) {
        let alpha = self.scrim_alpha();
        if alpha <= 0.001 {
            return;
        }
        let [x, y, w, h] = viewport;
        sugarloaf.rect(
            None,
            x,
            y,
            w,
            h,
            theme.f32_alpha(theme.black, alpha),
            SCRIM_DEPTH,
            SCRIM_ORDER,
        );
    }
}

impl<T: Default> Default for Overlay<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}
