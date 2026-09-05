//! Web-parity input policy exports: IME composition, touch gestures,
//! mobile soft-keyboard, and the remote-presence store.
//!
//! Every DECISION here lives in the shared Rust crates
//! (`neoism_ui::ime_state`, `neoism_ui::touch_policy`,
//! `neoism_ui::editor::crdt::remote_presence`) — this file is only the
//! wasm-bindgen surface that lets the TS host (and a future Capacitor
//! iOS shell) call the same policy the desktop fork runs, instead of
//! maintaining a hand-mirrored TypeScript copy.
//!
//! JS-facing shapes deliberately match the historical TS mirrors so
//! the host adapters stay thin:
//!
//! * touch actions serialize to the `{ kind: "start-simulated-left-click",
//!   ... }` discriminated union `web/src/services/touchPolicy.ts`
//!   defined;
//! * `ime_commit_dispatch` returns `{ text, useBracketedPaste }`;
//! * `PresenceStoreBridge.cursors_for` returns the `CrdtPeerPresence`
//!   wire objects render code already consumes, and
//!   `avatar_peers_by_buffer` returns the exact `set_presence_index`
//!   feed shape.

use wasm_bindgen::prelude::*;

use neoism_protocol::crdt::{CrdtPeerPresence, CrdtServerMessage};
use neoism_ui::editor::crdt::{
    peer_presence_to_wire, PeerCursor, PeerSelection, PresenceColor, PresencePublisher,
    RemotePresenceStore, PRESENCE_HEARTBEAT_INTERVAL_MS,
    PRESENCE_PUBLISH_MIN_INTERVAL_MS,
};
use neoism_ui::ime_state;
use neoism_ui::lifecycle_policy::FontSizeAction;
use neoism_ui::touch_policy::{
    self, classify_long_press, classify_touch_end, classify_touch_move,
    classify_touch_start_zoned, MobileInputContext, TouchAction, TouchLayoutSize,
    TouchPhase, TouchPoint, TouchPurpose, TouchZone,
};

// ---------------------------------------------------------------------------
// IME composition decisions
// ---------------------------------------------------------------------------

/// JS shape of [`ime_state::CommitDispatch`]; field casing matches the
/// historical `imePolicy.ts` mirror so call sites stay unchanged.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WireCommitDispatch {
    text: String,
    use_bracketed_paste: bool,
}

/// Classify an IME commit: raw keystroke path for single-char commits,
/// bracketed paste for multi-char commits. Returns
/// `{ text, useBracketedPaste }`.
#[wasm_bindgen]
pub fn ime_commit_dispatch(text: &str) -> JsValue {
    let dispatch = ime_state::commit_dispatch(text);
    serde_wasm_bindgen::to_value(&WireCommitDispatch {
        text: dispatch.text,
        use_bracketed_paste: dispatch.use_bracketed_paste,
    })
    .unwrap_or(JsValue::NULL)
}

/// Mode-locking during compose: `true` while a preedit is in flight,
/// i.e. the host must swallow real key events.
#[wasm_bindgen]
pub fn ime_should_drop_keys_during_compose(has_preedit: bool) -> bool {
    ime_state::should_drop_keys_during_compose(has_preedit)
}

/// `true` when a `keydown` was fired by the IME mid-composition
/// (`isComposing` flag, or the legacy `keyCode === 229` path).
#[wasm_bindgen]
pub fn ime_key_event_is_composing(is_composing: bool, key_code: u32) -> bool {
    ime_state::key_event_is_ime_composing(is_composing, key_code)
}

/// `true` when the assistant overlay owns the keyboard and IME events
/// must not reach the focused surface below.
#[wasm_bindgen]
pub fn ime_assistant_blocks(assistant_active: bool) -> bool {
    ime_state::assistant_blocks_ime(assistant_active)
}

// ---------------------------------------------------------------------------
// Touch gesture policy
// ---------------------------------------------------------------------------

fn touch_zone_from_tag(tag: &str) -> TouchZone {
    match tag {
        "chrome-panel" => TouchZone::ChromePanel,
        "editor-area" => TouchZone::EditorArea,
        _ => TouchZone::TerminalBody,
    }
}

/// JS mirror of the shared [`TouchAction`], serialized to the same
/// discriminated union the TS classifier historically produced.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum WireTouchAction {
    None,
    StartSimulatedLeftClick { x: usize, y: usize },
    Scroll { dx: f64, dy: f64, x: f64, y: f64 },
    UpdateMousePosition { x: usize, y: usize },
    ChangeFontSize { direction: &'static str },
    EndSimulatedLeftClick { x: usize, y: usize },
    EndSelect,
    EndScroll,
    PromoteTapToScroll,
    SelectWord { x: usize, y: usize },
    ExtendWordSelection { x: usize, y: usize },
    EndWordSelection,
    TwoFingerScroll { dx: f64, dy: f64 },
    SuppressNativeGesture,
}

fn wire_touch_action(action: TouchAction) -> JsValue {
    let wire = match action {
        TouchAction::None => WireTouchAction::None,
        TouchAction::StartSimulatedLeftClick { x, y } => {
            WireTouchAction::StartSimulatedLeftClick { x, y }
        }
        TouchAction::Scroll { dx, dy, x, y } => WireTouchAction::Scroll { dx, dy, x, y },
        TouchAction::UpdateMousePosition { x, y } => {
            WireTouchAction::UpdateMousePosition { x, y }
        }
        TouchAction::ChangeFontSize(direction) => WireTouchAction::ChangeFontSize {
            direction: match direction {
                FontSizeAction::Increase => "increase",
                FontSizeAction::Decrease => "decrease",
                FontSizeAction::Reset => "reset",
            },
        },
        TouchAction::EndSimulatedLeftClick { x, y } => {
            WireTouchAction::EndSimulatedLeftClick { x, y }
        }
        TouchAction::EndSelect => WireTouchAction::EndSelect,
        TouchAction::EndScroll => WireTouchAction::EndScroll,
        TouchAction::PromoteTapToScroll => WireTouchAction::PromoteTapToScroll,
        TouchAction::SelectWord { x, y } => WireTouchAction::SelectWord { x, y },
        TouchAction::ExtendWordSelection { x, y } => {
            WireTouchAction::ExtendWordSelection { x, y }
        }
        TouchAction::EndWordSelection => WireTouchAction::EndWordSelection,
        TouchAction::TwoFingerScroll { dx, dy } => {
            WireTouchAction::TwoFingerScroll { dx, dy }
        }
        TouchAction::SuppressNativeGesture => WireTouchAction::SuppressNativeGesture,
    };
    serde_wasm_bindgen::to_value(&wire).unwrap_or(JsValue::NULL)
}

/// `performance.now()` is fractional; the shared policy models time in
/// whole millis with `0` meaning "no timestamp / long-press disabled".
/// Clamp real-but-tiny timestamps up to 1 so a touch landing in the
/// page's first millisecond doesn't accidentally opt out.
fn touch_time_ms(time_ms: f64) -> u64 {
    if time_ms <= 0.0 {
        0
    } else {
        (time_ms as u64).max(1)
    }
}

fn touch_layout(width: f64, height: f64) -> TouchLayoutSize {
    TouchLayoutSize::new(width.max(0.0), height.max(0.0))
}

/// Stateful touch-gesture classifier — one per canvas/panel. The host
/// feeds `touchstart` / `touchmove` / `touchend` samples plus a
/// long-press tick and applies the returned actions; every gesture
/// decision (tap vs select vs scroll vs pinch vs two-finger pan vs
/// long-press) is the shared `neoism_ui::touch_policy` state machine.
#[wasm_bindgen]
#[derive(Default)]
pub struct TouchGesturePolicy {
    purpose: TouchPurpose,
}

#[wasm_bindgen]
impl TouchGesturePolicy {
    #[wasm_bindgen(constructor)]
    pub fn new() -> TouchGesturePolicy {
        Self::default()
    }

    /// Reset to the idle state (canvas lost focus / host takeover of
    /// the gesture, e.g. the mobile tab-strip pan path).
    pub fn reset(&mut self) {
        self.purpose = TouchPurpose::None;
    }

    /// True when at least one finger is currently tracked.
    pub fn is_active(&self) -> bool {
        !matches!(self.purpose, TouchPurpose::None)
    }

    /// Feed a `touchstart` sample with its zone tag
    /// (`"terminal-body" | "chrome-panel" | "editor-area"`).
    pub fn start(
        &mut self,
        id: f64,
        x: f64,
        y: f64,
        time_ms: f64,
        zone: &str,
    ) -> JsValue {
        let touch = TouchPoint::new_at(
            id as u64,
            x,
            y,
            TouchPhase::Started,
            touch_time_ms(time_ms),
        );
        wire_touch_action(classify_touch_start_zoned(
            &mut self.purpose,
            touch,
            touch_zone_from_tag(zone),
        ))
    }

    /// Feed a `touchmove` sample. Promotion actions
    /// (`start-simulated-left-click` / `promote-tap-to-scroll`)
    /// require the host to re-feed the same sample, exactly like the
    /// desktop fork's recursive `on_touch_motion` pattern.
    #[wasm_bindgen(js_name = "move")]
    pub fn move_touch(
        &mut self,
        id: f64,
        x: f64,
        y: f64,
        time_ms: f64,
        width: f64,
        height: f64,
    ) -> JsValue {
        let touch = TouchPoint::new_at(
            id as u64,
            x,
            y,
            TouchPhase::Moved,
            touch_time_ms(time_ms),
        );
        wire_touch_action(classify_touch_move(
            &mut self.purpose,
            touch,
            touch_layout(width, height),
        ))
    }

    /// Feed a `touchend` / `touchcancel` sample. Hosts should feed the
    /// same sample through `move` first (trailing-delta parity with the
    /// desktop fork), then apply this end action.
    pub fn end(
        &mut self,
        id: f64,
        x: f64,
        y: f64,
        time_ms: f64,
        width: f64,
        height: f64,
    ) -> JsValue {
        let touch = TouchPoint::new_at(
            id as u64,
            x,
            y,
            TouchPhase::Ended,
            touch_time_ms(time_ms),
        );
        wire_touch_action(classify_touch_end(
            &mut self.purpose,
            touch,
            touch_layout(width, height),
        ))
    }

    /// Drive on a timer/RAF loop with `now_ms = performance.now()`.
    /// Fires `select-word` exactly once per gesture when the
    /// long-press threshold is crossed.
    pub fn tick_long_press(&mut self, now_ms: f64, width: f64, height: f64) -> JsValue {
        wire_touch_action(classify_long_press(
            &mut self.purpose,
            touch_time_ms(now_ms),
            touch_layout(width, height),
        ))
    }
}

/// Whether the platform's native back/forward swipe-from-edge should
/// be suppressed for a touch starting in `zone`.
#[wasm_bindgen]
pub fn touch_should_suppress_swipe_back(zone: &str) -> bool {
    touch_policy::should_suppress_swipe_back(touch_zone_from_tag(zone))
}

// ---------------------------------------------------------------------------
// Mobile soft-keyboard policy
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WireKeyboardInset {
    bottom: f64,
    keyboard_open: bool,
}

/// Soft-keyboard inset from the host's viewport metrics. Returns
/// `{ bottom, keyboardOpen }` (the `MobileKeyboardInsets` shape).
#[wasm_bindgen]
pub fn mobile_keyboard_inset(
    inner_height: f64,
    viewport_height: f64,
    viewport_offset_top: f64,
) -> JsValue {
    let inset =
        touch_policy::keyboard_inset(inner_height, viewport_height, viewport_offset_top);
    serde_wasm_bindgen::to_value(&WireKeyboardInset {
        bottom: inset.bottom,
        keyboard_open: inset.keyboard_open,
    })
    .unwrap_or(JsValue::NULL)
}

#[derive(serde::Serialize)]
struct WireInputAttributes {
    autocapitalize: &'static str,
    autocorrect: &'static str,
    spellcheck: &'static str,
    inputmode: &'static str,
    enterkeyhint: &'static str,
}

/// Capture-element attributes for one input context tag
/// (`"code" | "text" | "url" | "search" | "editor"`).
#[wasm_bindgen]
pub fn mobile_input_attributes(context: &str, toolbar_visible: bool) -> JsValue {
    let attributes = touch_policy::mobile_input_attributes(
        MobileInputContext::from_tag(context),
        toolbar_visible,
    );
    serde_wasm_bindgen::to_value(&WireInputAttributes {
        autocapitalize: attributes.autocapitalize,
        autocorrect: attributes.autocorrect,
        spellcheck: attributes.spellcheck,
        inputmode: attributes.inputmode,
        enterkeyhint: attributes.enterkeyhint,
    })
    .unwrap_or(JsValue::NULL)
}

/// PTY byte sequence for one soft-toolbar / navigation key, or
/// `undefined` when the toolbar doesn't own the key.
#[wasm_bindgen]
pub fn mobile_named_key_bytes(key: &str) -> Option<Vec<u8>> {
    touch_policy::mobile_named_key_bytes(key).map(<[u8]>::to_vec)
}

/// Ctrl-chord byte for a latched-Ctrl character (`Ctrl+c` = 3), or
/// `undefined` for non-letters (host forwards raw text instead).
#[wasm_bindgen]
pub fn mobile_ctrl_chord_byte(text: &str) -> Option<u8> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    touch_policy::mobile_ctrl_chord_byte(first)
}

#[wasm_bindgen]
pub fn mobile_direct_insert_mode(coarse_pointer: bool, max_touch_points: u32) -> bool {
    touch_policy::mobile_direct_insert_mode(coarse_pointer, max_touch_points)
}

// ---------------------------------------------------------------------------
// Remote presence store
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct WireAvatarPeer {
    peer_id: String,
    display_name: String,
    color: [u8; 3],
    rainbow: bool,
}

#[derive(serde::Serialize)]
struct WireAvatarBuffer {
    buffer_id: String,
    peers: Vec<WireAvatarPeer>,
}

/// The shared inbound presence plane
/// (`neoism_ui::editor::crdt::RemotePresenceStore`) exposed to the JS
/// host. Feed every presence-bearing `CrdtServerMessage` the daemon
/// pushes; query remote cursors / avatar indexes per buffer. This
/// retires the hand-mirrored TS `RemotePresenceStore`.
#[wasm_bindgen]
#[derive(Default)]
pub struct PresenceStoreBridge {
    store: RemotePresenceStore,
}

#[wasm_bindgen]
impl PresenceStoreBridge {
    #[wasm_bindgen(constructor)]
    pub fn new() -> PresenceStoreBridge {
        Self::default()
    }

    /// Defensive self-filter: entries matching the local peer id are
    /// dropped so a misbehaving relay can't paint a ghost caret.
    pub fn set_local_peer_id(&mut self, peer_id: String) {
        self.store.set_local_peer_id(peer_id);
    }

    /// Fold one daemon push (a JSON-parsed `CrdtServerMessage` object)
    /// into the store. Returns `true` when remote presence changed and
    /// a redraw of the affected pane is due. Non-presence or malformed
    /// traffic returns `false` untouched.
    pub fn apply_server_message(&mut self, message: JsValue) -> bool {
        let Ok(message) = serde_wasm_bindgen::from_value::<CrdtServerMessage>(message)
        else {
            return false;
        };
        self.store.apply_server_message(&message)
    }

    /// Remote cursors for one buffer as `CrdtPeerPresence` wire
    /// objects (local peer already excluded).
    pub fn cursors_for(&self, buffer_id: &str) -> JsValue {
        let peers: Vec<CrdtPeerPresence> = self
            .store
            .cursors_for(buffer_id)
            .cloned()
            .map(peer_presence_to_wire)
            .collect();
        serde_wasm_bindgen::to_value(&peers).unwrap_or(JsValue::NULL)
    }

    /// True when `buffer_id` has at least one REMOTE cursor.
    pub fn has_remote_cursors(&self, buffer_id: &str) -> bool {
        self.store.has_remote_cursors(buffer_id)
    }

    /// True when ANY remote peer broadcasts the rainbow cursor preset
    /// (hosts keep repainting while idle so the animation ticks).
    pub fn any_rainbow(&self) -> bool {
        self.store.any_rainbow()
    }

    /// True when ANY remote peer is present on any buffer.
    pub fn has_any_peers(&self) -> bool {
        self.store.has_any_peers()
    }

    /// Per-buffer avatar peers in the exact `set_presence_index` feed
    /// shape: `[{ buffer_id, peers: [{ peer_id, display_name,
    /// color: [r, g, b], rainbow }] }]`. Push only on presence CHANGE.
    pub fn avatar_peers_by_buffer(&self) -> JsValue {
        let buffers: Vec<WireAvatarBuffer> = self
            .store
            .avatar_peers_by_buffer()
            .into_iter()
            .map(|(buffer_id, peers)| WireAvatarBuffer {
                buffer_id,
                peers: peers
                    .into_iter()
                    .map(|peer| WireAvatarPeer {
                        peer_id: peer.peer_id,
                        display_name: peer.display_name,
                        color: peer.color,
                        rainbow: peer.rainbow,
                    })
                    .collect(),
            })
            .collect();
        serde_wasm_bindgen::to_value(&buffers).unwrap_or(JsValue::NULL)
    }

    /// Client-side staleness backstop mirroring the daemon TTL.
    /// Returns `true` when anything fell out.
    pub fn prune_stale(&mut self, now_ms: f64, ttl_ms: f64) -> bool {
        self.store
            .prune_stale(now_ms.max(0.0) as u64, ttl_ms.max(0.0) as u64)
    }

    /// Drop every remote cursor (daemon reconnect). Returns `true`
    /// when the store held peers.
    pub fn clear(&mut self) -> bool {
        self.store.clear()
    }
}

// ---------------------------------------------------------------------------
// Presence publisher (outbound coalescing state machine)
// ---------------------------------------------------------------------------

/// JS shape of the publisher's `tick` target — matches the TS
/// `ActivePresenceTarget` interface (camelCase `bufferId`, wire-shaped
/// cursor/selection).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireActiveTarget {
    buffer_id: String,
    cursor: WireCursorPosition,
    #[serde(default)]
    selection: Option<WireSelectionRange>,
    #[serde(default)]
    insert: bool,
}

#[derive(serde::Deserialize)]
struct WireCursorPosition {
    line: u32,
    column: u32,
    #[serde(default)]
    offset: Option<u32>,
}

#[derive(serde::Deserialize)]
struct WireSelectionRange {
    anchor: WireCursorPosition,
    head: WireCursorPosition,
}

fn peer_cursor_from_wire(cursor: WireCursorPosition) -> PeerCursor {
    let base = PeerCursor::new(cursor.line, cursor.column);
    match cursor.offset {
        Some(offset) => base.with_offset(offset),
        None => base,
    }
}

/// The shared OUTBOUND presence plane
/// (`neoism_ui::editor::crdt::PresencePublisher`) exposed to the JS
/// host: a pure coalescing state machine — rate-limited publishes
/// (~13Hz), TTL keep-alive heartbeats, `ClearPresence` on buffer
/// switch/close. This retires the hand-mirrored TS `PresencePublisher`.
#[wasm_bindgen]
pub struct PresencePublisherBridge {
    publisher: PresencePublisher,
}

#[wasm_bindgen]
impl PresencePublisherBridge {
    /// `peer_id` is the stable per-device identity; `display_name` is
    /// what peers see next to the caret. The interval overrides exist
    /// for tests; hosts pass `undefined` to keep the shared defaults.
    #[wasm_bindgen(constructor)]
    pub fn new(
        peer_id: String,
        display_name: String,
        min_interval_ms: Option<f64>,
        heartbeat_interval_ms: Option<f64>,
    ) -> PresencePublisherBridge {
        let publisher = PresencePublisher::new(peer_id, display_name).with_intervals(
            min_interval_ms
                .map_or(PRESENCE_PUBLISH_MIN_INTERVAL_MS, |ms| ms.max(0.0) as u64),
            heartbeat_interval_ms
                .map_or(PRESENCE_HEARTBEAT_INTERVAL_MS, |ms| ms.max(0.0) as u64),
        );
        PresencePublisherBridge { publisher }
    }

    pub fn peer_id(&self) -> String {
        self.publisher.peer_id().to_string()
    }

    /// Publish under the LOCAL THEME'S cursor color.
    pub fn set_color(&mut self, r: u8, g: u8, b: u8) {
        self.publisher.set_color(PresenceColor { r, g, b });
    }

    /// Publish the rainbow-preset flag (peers animate locally).
    pub fn set_rainbow(&mut self, rainbow: bool) {
        self.publisher.set_rainbow(rainbow);
    }

    /// Coalesce the local cursor into at most a couple of wire
    /// messages. `active` is the `ActivePresenceTarget` object (or
    /// `null`/`undefined` when no daemon-backed buffer is focused).
    /// Returns a `CrdtClientMessage[]` array in the exact wire shape
    /// the daemon expects — usually empty.
    pub fn tick(&mut self, active: JsValue, now_ms: f64) -> JsValue {
        let target = if active.is_null() || active.is_undefined() {
            None
        } else {
            match serde_wasm_bindgen::from_value::<WireActiveTarget>(active) {
                Ok(target) => Some(target),
                // Malformed target: treat as "no active buffer" so a
                // stale publish is cleared rather than repeated.
                Err(_) => None,
            }
        };
        let messages = match target {
            None => self.publisher.tick(None, now_ms.max(0.0) as u64),
            Some(target) => {
                let cursor = peer_cursor_from_wire(target.cursor);
                let selection = target.selection.map(|selection| {
                    PeerSelection::new(
                        peer_cursor_from_wire(selection.anchor),
                        peer_cursor_from_wire(selection.head),
                    )
                });
                self.publisher.tick(
                    Some((target.buffer_id.as_str(), cursor, selection, target.insert)),
                    now_ms.max(0.0) as u64,
                )
            }
        };
        serde_wasm_bindgen::to_value(&messages).unwrap_or(JsValue::NULL)
    }
}
