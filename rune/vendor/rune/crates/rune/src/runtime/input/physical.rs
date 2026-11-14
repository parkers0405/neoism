use super::*;
use crate::{PhysicalInput, PhysicalKeyCode};

pub(super) struct Fallback { key: KeyEvent, base: Option<char>, unhandled: Option<Unhandled> }

/// What a window does with a logical key no handler took: close or minimize as its title-bar
/// button would, or run a menu item. A standard ⌘W or ⌘M item, or an app's item marked
/// `MenuItem::keys_first`, asks for it, so an app's own binding wins. On Windows, Alt+Space
/// opens the window menu.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone)]
pub(crate) enum Unhandled { Close, Minimize, Menu(std::sync::Arc<dyn Fn() + Send + Sync>), WindowMenu }

impl std::fmt::Debug for Unhandled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::Close => "Close", Self::Minimize => "Minimize", Self::Menu(_) => "Menu", Self::WindowMenu => "WindowMenu" })
    }
}

/// Whether an activation key (Enter, Escape, Space) is held down: its physical code (native and
/// physical input) or logical key (`Runtime::key`), and the focus its first press landed on.
#[derive(Default)]
pub(super) enum Activation {
    #[default]
    Up,
    Held { physical: Option<PhysicalKeyCode>, key: Option<crate::Key>, focus: Option<String> },
}

impl<S: 'static> Runtime<S> {
    /// Queue physical input without a logical fallback (also useful for headless drivers).
    /// A modifier key still gets its `Key::Modifier` when modifier keys are on.
    pub fn physical_input(&mut self, event: PhysicalInput) {
        self.observe_mods(&event);
        if self.cancel_drag_key(&event) { return; }
        self.queue.push(Action::Physical(event, None));
    }

    fn cancel_drag_key(&mut self, event: &PhysicalInput) -> bool {
        matches!(event, PhysicalInput::Key { key: PhysicalKeyCode::Escape, pressed: true, .. })
            && self.cancel_drag()
    }
    /// Shift held now decides whether the next press extends a text selection, ahead of the
    /// queue the press itself skips. A key press hides a tooltip.
    fn observe_mods(&mut self, event: &PhysicalInput) {
        match event {
            PhysicalInput::Modifiers(m) => { self.text_sel.shift = m.shift; self.held_mods = *m; }
            PhysicalInput::FocusLost => { self.text_sel.shift = false; self.held_mods = Default::default(); self.dismiss_help(); }
            PhysicalInput::Key { pressed: true, mods, .. } => { self.held_mods = *mods; self.dismiss_help(); }
            PhysicalInput::Key { mods, .. } => self.held_mods = *mods,
        }
    }
    /// Queue a key press with its physical code and logical key, as a native host sends them
    /// (a script's `{"type": "key", "code": …}`), and its `Tx::key_base`.
    pub(crate) fn physical_key_input(&mut self, event: PhysicalInput, key: KeyEvent, base: Option<char>) {
        self.observe_mods(&event);
        self.physical_then(event, Some(key), base, None);
    }
    /// Return whether the native host needs to request a frame for this event.
    #[cfg_attr(any(target_os = "windows", target_os = "ios", target_os = "android"), allow(dead_code))]
    pub(crate) fn physical_with_fallback(&mut self, event: PhysicalInput, key: Option<KeyEvent>) -> bool {
        self.physical_then(event, key, None, None)
    }
    /// `physical_with_fallback`, with the key's unmodified character (`Tx::key_base`), and
    /// `unhandled` happens when no handler takes the logical key.
    pub(crate) fn physical_then(&mut self, event: PhysicalInput, key: Option<KeyEvent>, base: Option<char>, unhandled: Option<Unhandled>) -> bool {
        if self.cancel_drag_key(&event) { return true; }
        if key.is_none() && matches!(event, PhysicalInput::Modifiers(_) | PhysicalInput::Key { pressed: false, .. }) {
            return self.physical_observation(event);
        }
        if !matches!(event, PhysicalInput::Key { pressed: false, .. }) { self.dismiss_help(); }
        self.queue.push(Action::Physical(event, key.map(|key| Fallback { key, base, unhandled })));
        true
    }
    /// Modifier/release events have no logical action. When input is already queued,
    /// its requested frame will process this observation in order after clicks/keys.
    /// At rest, only a consuming handler invalidates the view and requests a frame.
    pub(crate) fn physical_observation(&mut self, event: PhysicalInput) -> bool {
        let busy = !self.queue.is_empty();
        self.observe_mods(&event);
        // The release of a modifier key the app hears needs a frame, in order like a press.
        // So does one while state changed since the last frame (a covered window keeps it
        // changed until it uncovers): the focused handler is the new view's.
        if self.modifier_pending(&event) || busy || self.dirty {
            self.queue.push(Action::Physical(event, None));
            return !busy;
        }
        let mut tx = Tx::default();
        let consumed = self.dispatch_physical(event, None, &mut tx);
        let live_only = std::mem::take(&mut tx.live_only);
        self.latch_requests(&mut tx);
        merge(&mut self.pending, tx);
        if consumed && live_only {
            self.live_only_skips += 1;
            self.live_dirty = true;
        } else {
            self.dirty |= consumed;
        }
        consumed
    }
    /// Remember an activation key's first press and the focus it landed on; true drops a
    /// repeat of that same key once focus has moved. Any other first press replaces it.
    pub(super) fn held_gate(&mut self, physical: Option<PhysicalKeyCode>, key: Option<&crate::Key>, repeat: bool) -> bool {
        use crate::{Key, PhysicalKeyCode as Code};
        if !repeat {
            // The OS keeps repeating a held key through modifier presses.
            if physical.is_some_and(|c| matches!(c, Code::ShiftLeft | Code::ShiftRight | Code::ControlLeft
                | Code::ControlRight | Code::AltLeft | Code::AltRight | Code::SuperLeft | Code::SuperRight
                | Code::CapsLock | Code::Fn)) || key.is_some_and(|k| matches!(k, Key::Modifier { .. })) { return false; }
            let activation = physical.is_some_and(|c| matches!(c, Code::Enter | Code::NumpadEnter | Code::Escape | Code::Space))
                || key.is_some_and(|k| matches!(k, Key::Enter | Key::Escape) || matches!(k, Key::Text(t) if t == " "));
            self.held = if activation { Activation::Held { physical, key: key.cloned(), focus: self.focus.owned() } } else { Activation::Up };
            return false;
        }
        let Activation::Held { physical: held, key: held_key, focus } = &self.held else { return false };
        let same = match physical { Some(code) => *held == Some(code), None => held_key.as_ref() == key };
        same && focus.as_deref() != self.focus.path()
    }
    pub(super) fn dispatch_physical(&mut self, event: PhysicalInput, fallback: Option<Fallback>, tx: &mut Tx) -> bool {
        use crate::PhysicalKeyCode as Code;
        let is_mod = |c: Code| matches!(c, Code::ShiftLeft | Code::ShiftRight | Code::ControlLeft
            | Code::ControlRight | Code::AltLeft | Code::AltRight | Code::SuperLeft | Code::SuperRight
            | Code::CapsLock | Code::Fn);
        match event {
            PhysicalInput::Key { key, pressed: true, repeat, .. } => {
                if self.held_gate(Some(key), fallback.as_ref().map(|f| &f.key.key), repeat) { return false; }
                if !is_mod(key) { self.note_key_down(repeat); }
            }
            PhysicalInput::Key { key, pressed: false, .. } => {
                if matches!(&self.held, Activation::Held { physical, .. } if *physical == Some(key)) { self.held = Activation::Up; }
                if fallback.is_none() && !is_mod(key) && self.composing() {
                    self.ime_keys_down = self.ime_keys_down.saturating_sub(1);
                }
            }
            PhysicalInput::FocusLost => {
                self.key_focus = None;
                self.keys_held = 0;
                self.ime_keys_down = 0;
                self.ime_eat_key_up = false;
            }
            PhysicalInput::Modifiers(_) => {}
        }
        let repeat = matches!(event, PhysicalInput::Key { repeat: true, .. });
        let code = match event { PhysicalInput::Key { key, .. } => Some(key), _ => None };
        // Live-only when every handler that took the event said so (`Tx::live_only`).
        let mut live = None;
        let consumed = self.offer_physical(&event, tx, &mut live);
        let modifier = self.modifier_dispatch(&event, consumed, tx, &mut live);
        let ran = consumed || modifier | match fallback {
            // A release's logical key, for `on_key_up`.
            Some(Fallback { key, base, .. }) if matches!(event, PhysicalInput::Key { pressed: false, .. }) => {
                let mut one = Tx { physical_key: code, key_base: base, ..self.transaction() };
                let ran = self.route_key_up(&key, &mut one);
                fold_live(&mut live, ran, &one);
                merge(tx, one);
                ran
            }
            Some(Fallback { key, base, unhandled }) => {
                if key.edit_command() == Some('v') && self.accepts_paste() {
                    // Capture the target in event order, before another queued click can
                    // change focus. An empty/image clipboard still consumes the command.
                    self.paste_request = Some(self.paste_target());
                    false
                } else {
                    let mut one = Tx { physical_key: code, key_base: base, ..self.transaction() };
                    let ran = self.route_key(&key, repeat, &mut one);
                    // A held key repeats a menu item, as AppKit's own equivalents do, but closes once.
                    let unhandled = unhandled.filter(|u| !ran && (!repeat || matches!(u, Unhandled::Menu(_))));
                    let fell = unhandled.is_some();
                    match unhandled {
                        Some(Unhandled::Close) => one.close_window(),
                        Some(Unhandled::Minimize) => one.minimize(),
                        Some(Unhandled::Menu(run)) => run(),
                        Some(Unhandled::WindowMenu) => self.request_window_menu(),
                        None => {}
                    }
                    fold_live(&mut live, ran || fell, &one);
                    merge(tx, one);
                    ran || fell
                }
            }
            None => false,
        };
        let ran = self.dispatch_mods(tx, &mut live) || ran;
        tx.live_only = ran && live == Some(true);
        ran
    }
    /// `App::on_modifiers`, when the held modifiers changed since it last heard them.
    fn dispatch_mods(&mut self, tx: &mut Tx, live: &mut Option<bool>) -> bool {
        let Some(f) = self.on_mods.filter(|_| self.held_mods != self.mods_heard) else { return false };
        self.mods_heard = self.held_mods;
        let mut one = self.transaction();
        let state = match &mut self.playground {
            Some(playground) => playground.key_state(&mut self.state),
            None => Some(&mut self.state),
        };
        let Some(state) = state else { return false };
        f(state, self.mods_heard, &mut one);
        fold_live(live, true, &one);
        merge(tx, one);
        true
    }
    /// The focused element's `on_physical_input` handler; true if it consumed `event`.
    fn offer_physical(&mut self, event: &PhysicalInput, tx: &mut Tx, live: &mut Option<bool>) -> bool {
        let Some(handler) = self.focus.path().and_then(|p| self.node(p))
            .and_then(|n| n.focus.physical.clone()) else { return false };
        let mut one = Tx { key_repeat: matches!(event, PhysicalInput::Key { repeat: true, .. }), ..self.transaction() };
        let consumed = Tx::took(handler(states!(self), event, &mut one), &mut one);
        fold_live(live, consumed, &one);
        merge(tx, one);
        consumed
    }
    /// Focus moving off an element inside the window: its handler hears `FocusLost`, and
    /// modifier keys stay held.
    pub(super) fn physical_focus_lost(&mut self) -> bool {
        let mut tx = Tx::default();
        let changed = self.offer_physical(&PhysicalInput::FocusLost, &mut tx, &mut None);
        self.latch_requests(&mut tx);
        merge(&mut self.pending, tx);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cx, El, Env, Key, Mods, PhysicalKeyCode, TextEdit, TextStyle, col, el, text_input};

    #[derive(Default)]
    struct State { capture: bool, physical: usize, logical: usize, dismissed: usize, edit: TextEdit, gone: bool }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let recorder = el().key("recorder").size(100.0, 40.0).focusable(true)
            .on_press(|s: &mut State| s.capture = true)
            .on_physical_input(|s: &mut State, _, _| {
                if s.capture { s.physical += 1; true } else { false }
            });
        col().key("modal").size(300.0, 200.0).modal(false, |s: &mut State| s.dismissed += 1)
            .children((!s.gone).then_some(recorder))
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("edit").size(200.0, 40.0)
                .on_edit(|s: &mut State| &mut s.edit))
    }
    #[test]
    fn physical_dispatch_precedes_modal_app_and_paste_after_same_frame_click() {
        let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env { system_dark: true, scale: 1.0 });
        rt.on_key = Some(|s, _, _| { s.logical += 1; true });
        rt.frame(0.0);
        let (x, y) = rt.find("recorder").unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
        let mods = Mods { cmd: true, ..Mods::default() };
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::KeyV, mods, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Command('v'), mods }));
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Escape, mods: Mods::default(), pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        rt.frame(0.0);
        assert_eq!((rt.state.physical, rt.state.logical, rt.state.dismissed), (2, 0, 0));
        assert_eq!(rt.state.edit.text, "");
        assert!(rt.paste_request.is_none(), "capture must not read the clipboard");
        rt.state.capture = false;
        let (x, y) = rt.find("edit").unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::KeyV, mods, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Command('v'), mods }));
        rt.frame(0.0);
        let target = rt.paste_request.take().expect("paste requested");
        rt.paste_result(target, crate::ClipboardContent::new("paste"));
        rt.frame(0.0);
        assert_eq!(rt.state.edit.text, "paste");
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Escape, mods: Mods::default(), pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        rt.frame(0.0);
        assert_eq!(rt.state.dismissed, 1);
    }

    #[test]
    fn modifier_and_release_redraws_require_consumption_and_keep_queued_focus_order() {
        let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let modifiers = PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() });
        let release = PhysicalInput::Key { key: PhysicalKeyCode::ShiftLeft, mods: Mods::default(), pressed: false, repeat: false };
        for event in [modifiers, release] {
            assert!(!rt.physical_with_fallback(event, None));
            assert!(rt.queue.is_empty() && !rt.dirty, "unused input must not schedule work");
        }
        click(&mut rt, "recorder");
        // The click already requested a frame. Do not deliver modifiers to the old
        // focus or request another frame before that queued click starts capture.
        assert!(!rt.physical_with_fallback(modifiers, None));
        assert_eq!(rt.state.physical, 0);
        rt.frame(0.0);
        assert_eq!(rt.state.physical, 1);
        for event in [modifiers, release] {
            assert!(rt.physical_with_fallback(event, None));
            assert!(rt.dirty, "consumption must commit the handler's state changes");
            rt.frame(0.0);
        }
        assert_eq!(rt.state.physical, 3);
        rt.state.capture = false;
        assert!(!rt.physical_with_fallback(release, None), "a declining handler needs no frame");
        rt.state.capture = true;
        click(&mut rt, "edit");
        assert!(!rt.physical_with_fallback(modifiers, None));
        rt.frame(0.0);
        let delivered = rt.state.physical; // FocusLost may notify the old recorder.
        assert!(!rt.physical_with_fallback(release, None));
        assert_eq!(rt.state.physical, delivered);
        assert!(rt.queue.is_empty() && !rt.dirty);
    }

    /// A modifier change while state changed since the last frame, as a covered window's does
    /// until it uncovers, goes to the focused handler of the view that state builds, not the
    /// last frame's.
    #[test]
    fn a_modifier_change_after_an_update_waits_for_the_new_view() {
        let mut rt = Runtime::new(State::default(), view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        click(&mut rt, "recorder");
        rt.frame(0.0);
        assert!(rt.state.capture);
        let heard = rt.state.physical;
        rt.update(|s, _| s.gone = true);
        let modifiers = PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() });
        assert!(rt.physical_with_fallback(modifiers, None), "it asks for the frame that builds the new view");
        assert_eq!(rt.state.physical, heard, "the last frame's recorder doesn't hear it");
        rt.frame(0.0);
        assert!(rt.find("recorder").is_none());
        assert_eq!(rt.state.physical, heard, "and the new view has none");
    }

    #[derive(Default)]
    struct Fields { first: TextEdit, second: TextEdit, secure: bool, keys: usize, app_keys: usize }
    fn fields(s: &Fields, _: &mut Cx<Fields>) -> El {
        let first = text_input(&s.first, TextStyle::new(14.0, 400.0)).single_line()
            .key("first").size(100.0, 32.0).on_edit(|s: &mut Fields| &mut s.first)
            .on_input_key(|s: &mut Fields, _, _| { s.keys += 1; false });
        col().size(300.0, 200.0)
            .child(if s.secure { first.secure() } else { first })
            .child(text_input(&s.second, TextStyle::new(14.0, 400.0)).key("second")
                .size(100.0, 32.0).on_edit(|s: &mut Fields| &mut s.second))
    }
    fn click<S: 'static>(rt: &mut Runtime<S>, key: &str) {
        let (x, y) = rt.find(key).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y);
    }
    fn key<S: 'static>(rt: &mut Runtime<S>, physical: PhysicalKeyCode, key: Key) {
        let mods = Mods { cmd: matches!(key, Key::Command(_)), ..Mods::default() };
        rt.physical_with_fallback(PhysicalInput::Key { key: physical, mods, pressed: true, repeat: false },
            Some(KeyEvent { key, mods }));
    }
    fn fields_runtime(secure: bool) -> Runtime<Fields> {
        let mut rt = Runtime::new(Fields { secure, ..Fields::default() }, fields, (300.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, _, _| { s.app_keys += 1; true });
        rt.frame(0.0); rt
    }

    #[test]
    fn paste_is_swallowed_without_text_and_target_survives_a_later_queued_click() {
        let mut rt = fields_runtime(false);
        key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
        rt.frame(0.0);
        assert!(rt.paste_request.is_none(), "no focused editor or paste handler: no clipboard read");
        assert_eq!(rt.state.app_keys, 1);
        click(&mut rt, "first");
        key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
        rt.frame(0.0);
        let first = rt.paste_request.take().unwrap();
        assert!(first.is_some());
        assert_eq!((rt.state.keys, rt.state.app_keys), (0, 1));
        // Image-only clipboard sends no reply. An empty text reply also never falls
        // through to either key handler.
        rt.paste_result(first.clone(), crate::ClipboardContent::new(""));
        rt.frame(0.0);
        assert_eq!((rt.state.keys, rt.state.app_keys), (0, 1));
        key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
        click(&mut rt, "second");
        rt.frame(0.0);
        let target = rt.paste_request.take().unwrap();
        assert_eq!(target, first, "target captured before the next click, not at worker flush");
        assert_ne!(target, rt.paste_target());
        rt.paste_result(target, crate::ClipboardContent::new("late"));
        rt.frame(0.0);
        assert!(rt.state.first.text.is_empty() && rt.state.second.text.is_empty());
    }

    #[test]
    fn physical_fallback_edits_single_line_and_preserves_secure_copy_protection() {
        for secure in [false, true] {
            let mut rt = fields_runtime(secure);
            click(&mut rt, "first");
            key(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()));
            rt.frame(0.0);
            assert_eq!(rt.state.first.text, "x");
            assert_eq!(rt.has_secure_focus(), secure);
            key(&mut rt, PhysicalKeyCode::KeyA, Key::Command('a'));
            key(&mut rt, PhysicalKeyCode::KeyC, Key::Command('c'));
            rt.frame(0.0);
            assert_eq!(rt.copy_request.is_none(), secure);
            key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
            rt.frame(0.0);
            let target = rt.paste_request.take().unwrap();
            rt.paste_result(target, crate::ClipboardContent::new("replacement"));
            rt.frame(0.0);
            assert_eq!(rt.state.first.text, "replacement");
            if secure {
                let elements = rt.elements();
                let field = elements.iter().find(|e| e.key.as_deref() == Some("first")).unwrap();
                assert!(field.text.is_none());
            }
        }
    }

    #[derive(Default)]
    struct Held { edit: TextEdit, other: TextEdit, input: Vec<bool>, app: Vec<(Key, bool)> }
    fn held(s: &Held, _: &mut Cx<Held>) -> El {
        col().size(300.0, 200.0)
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).single_line().key("field").size(200.0, 32.0)
                .on_edit(|s: &mut Held| &mut s.edit)
                .on_input_key(|s: &mut Held, _, tx| { s.input.push(tx.key_repeat()); false }))
            .child(text_input(&s.other, TextStyle::new(14.0, 400.0)).single_line().key("other").size(200.0, 32.0)
                .on_edit(|s: &mut Held| &mut s.other))
    }
    fn focused(rt: &Runtime<Held>) -> Option<String> {
        rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
    }
    fn press<S: 'static>(rt: &mut Runtime<S>, physical: PhysicalKeyCode, key: Key, repeat: bool) {
        let mods = Mods::default();
        rt.physical_with_fallback(PhysicalInput::Key { key: physical, mods, pressed: true, repeat }, Some(KeyEvent { key, mods }));
        rt.frame(0.0);
    }

    #[test]
    fn key_handlers_see_a_held_keys_repeats_and_editing_keeps_repeating() {
        let mut rt = Runtime::new(Held::default(), held, (300.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, tx| { s.app.push((ev.key.clone(), tx.key_repeat())); true });
        rt.frame(0.0);
        // No focus: the app handler gets first refusal, so it sees both presses.
        press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, false);
        press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, true);
        assert_eq!(rt.state.app, [(Key::Enter, false), (Key::Enter, true)]);
        click(&mut rt, "field");
        rt.frame(0.0);
        press(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()), false);
        press(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()), true);
        press(&mut rt, PhysicalKeyCode::KeyX, Key::Text("x".into()), true);
        assert_eq!(rt.state.input, [false, true, true]);
        assert_eq!(rt.state.edit.text, "xxx", "a held letter keeps typing");
        // Held Tab keeps moving focus, and Rune consumes it before the app handler.
        press(&mut rt, PhysicalKeyCode::Tab, Key::Tab, true);
        assert_eq!(focused(&rt).as_deref(), Some("other"));
        press(&mut rt, PhysicalKeyCode::Tab, Key::Tab, true);
        assert_eq!(focused(&rt).as_deref(), Some("field"));
        assert_eq!(rt.state.app.len(), 2);
    }

    #[test]
    fn a_queued_repeat_reaches_the_app_handler_with_the_flag_set() {
        let mut rt = Runtime::new(Held::default(), held, (300.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, tx| { s.app.push((ev.key.clone(), tx.key_repeat())); true });
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
        rt.repeat_key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
        rt.key(KeyEvent { key: Key::Text("s".into()), mods: Mods::default() });
        rt.frame(0.0);
        let s = Key::Text("s".into());
        assert_eq!(rt.state.app, [(s.clone(), false), (s.clone(), true), (s, false)]);
    }

    #[test]
    fn key_handlers_see_the_physical_key_whatever_the_layout_types() {
        #[derive(Default)]
        struct Codes {
            edit: TextEdit,
            input: Vec<Option<PhysicalKeyCode>>,
            app: Vec<(Key, Option<PhysicalKeyCode>, bool)>,
            ups: Vec<Option<PhysicalKeyCode>>,
            change: Vec<Option<PhysicalKeyCode>>,
        }
        let view = |s: &Codes, _: &mut Cx<Codes>| col().size(300.0, 200.0)
            .on_key_up(|s: &mut Codes, _, tx| { s.ups.push(tx.physical_key()); true })
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 32.0)
                .on_edit(|s: &mut Codes| &mut s.edit)
                .on_input_change(|s: &mut Codes, tx| s.change.push(tx.physical_key()))
                .on_input_key(|s: &mut Codes, _, tx| { s.input.push(tx.physical_key()); false }));
        let mut rt = Runtime::new(Codes::default(), view, (300.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, tx| { s.app.push((ev.key.clone(), tx.physical_key(), tx.key_repeat())); true });
        rt.frame(0.0);
        click(&mut rt, "field");
        rt.frame(0.0);
        // ⌘⇧7 on a German layout types '/', on a US one '&'; both are the 7 key (press, repeat, key-up).
        let mods = Mods { cmd: true, shift: true, ..Mods::default() };
        for (c, repeat) in [('/', false), ('&', true)] {
            rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Digit7, mods, pressed: true, repeat },
                Some(KeyEvent { key: Key::Command(c), mods }));
            rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Digit7, mods, pressed: false, repeat: false },
                Some(KeyEvent { key: Key::Command(c), mods }));
            rt.frame(0.0);
        }
        let seven = Some(PhysicalKeyCode::Digit7);
        assert_eq!(rt.state.input, [seven, seven]);
        assert_eq!(rt.state.app, [(Key::Command('/'), seven, false), (Key::Command('&'), seven, true)]);
        assert_eq!(rt.state.ups, [seven, seven]);
        // A logical key with no physical origin has none, and non-key callbacks (`on_input_change`)
        // never inherit the key handler's physical key.
        rt.key(KeyEvent { key: Key::Command('&'), mods });
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::KeyA, mods: Mods::default(), pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() }));
        rt.frame(0.0);
        assert_eq!(rt.state.app.last(), Some(&(Key::Command('&'), None, false)));
        assert_eq!(rt.state.change, [None]);
    }

    #[test]
    fn a_format_chord_the_field_does_not_allow_goes_to_the_app() {
        #[derive(Default)]
        struct Rich { edit: TextEdit, app: Vec<Key> }
        let view = |s: &Rich, _: &mut Cx<Rich>| col().size(300.0, 200.0)
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 32.0).on_edit(|s: &mut Rich| &mut s.edit));
        let edit = TextEdit::new("ab").rich().allow_marks([crate::Mark::Bold]);
        let mut rt = Runtime::new(Rich { edit, app: Vec::new() }, view, (300.0, 200.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, ev, _| { s.app.push(ev.key.clone()); true });
        rt.frame(0.0);
        click(&mut rt, "field");
        rt.frame(0.0);
        let cmd = |c| KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } };
        for c in ['a', 'u', 'b'] { rt.key(cmd(c)); }
        rt.frame(0.0);
        assert_eq!(rt.state.app, [Key::Command('u')]);
        assert!(rt.state.edit.marks().iter().all(|m| m.mark == crate::Mark::Bold) && !rt.state.edit.marks().is_empty());
    }

    fn release<S: 'static>(rt: &mut Runtime<S>, physical: PhysicalKeyCode) {
        rt.physical_with_fallback(PhysicalInput::Key { key: physical, mods: Mods::default(), pressed: false, repeat: false }, None);
        rt.frame(0.0);
    }

    #[test]
    fn a_held_arrow_keeps_repeating_after_an_escape_tap_moves_focus() {
        let mut rt = fields_runtime(false);
        click(&mut rt, "first");
        press(&mut rt, PhysicalKeyCode::ArrowDown, Key::Down, false);
        press(&mut rt, PhysicalKeyCode::Escape, Key::Escape, false);
        release(&mut rt, PhysicalKeyCode::Escape);
        assert!(!rt.has_focus(), "Escape blurred the field");
        let before = rt.state.app_keys;
        press(&mut rt, PhysicalKeyCode::ArrowDown, Key::Down, true);
        press(&mut rt, PhysicalKeyCode::ArrowDown, Key::Down, true);
        assert_eq!(rt.state.app_keys, before + 2, "the held arrow's repeats still arrive");
        // The same without a release, from a script: only Escape's own repeats are dropped.
        click(&mut rt, "first");
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }); rt.frame(0.0);
        rt.repeat_key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.repeat_key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.app_keys, before + 3);
    }

    #[test]
    fn a_released_key_and_a_pointer_focus_change_leave_later_repeats_alone() {
        let mut rt = fields_runtime(false);
        click(&mut rt, "second");
        press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, false);
        release(&mut rt, PhysicalKeyCode::Enter);
        click(&mut rt, "first");
        rt.frame(0.0);
        rt.repeat_key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.repeat_key(KeyEvent { key: Key::Enter, mods: Mods::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.keys, 2, "scripted repeats reach the newly focused field");
    }

    #[derive(Default)]
    struct Opener { modal: bool, go_repeats: usize, sink: usize }
    fn opener(s: &Opener, _: &mut Cx<Opener>) -> El {
        let mut root = col().size(300.0, 200.0)
            .child(el().key("go").size(40.0, 30.0).focusable(true).role("button")
                .on_press(|s: &mut Opener| s.modal = true)
                .on_physical_input(|s: &mut Opener, e, _| {
                    if matches!(e, PhysicalInput::Key { repeat: true, .. }) { s.go_repeats += 1; }
                    false
                }))
            .child(el().key("grab").size(40.0, 30.0).focusable(true)
                .on_physical_input(|s: &mut Opener, e, _| match e {
                    PhysicalInput::Key { key: PhysicalKeyCode::Enter, pressed: true, repeat: false, .. } => { s.modal = true; true }
                    _ => false,
                }));
        if s.modal {
            root = root.child(col().key("modal").absolute(100.0, 0.0).size(100.0, 100.0)
                .modal(true, |s: &mut Opener| s.modal = false)
                .child(el().key("sink").size(40.0, 30.0).focusable(true)
                    .on_physical_input(|s: &mut Opener, e, _| {
                        if matches!(e, PhysicalInput::Key { pressed: true, .. }) { s.sink += 1; }
                        true
                    })));
        }
        root
    }
    fn opener_focused_on(tabs: usize) -> Runtime<Opener> {
        let mut rt = Runtime::new(Opener::default(), opener, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..tabs { rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() }); rt.frame(0.0); }
        rt
    }
    fn focused_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
        rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
    }

    #[test]
    fn repeats_never_reach_physical_handlers_after_their_key_moved_focus() {
        // A built-in press opens the modal: the new focus's physical handler sees no repeat.
        let mut rt = opener_focused_on(1);
        assert_eq!(focused_key(&rt).as_deref(), Some("go"));
        press(&mut rt, PhysicalKeyCode::Space, Key::Text(" ".into()), false);
        assert_eq!(focused_key(&rt).as_deref(), Some("sink"));
        press(&mut rt, PhysicalKeyCode::Space, Key::Text(" ".into()), true);
        press(&mut rt, PhysicalKeyCode::Space, Key::Text(" ".into()), true);
        assert_eq!(rt.state.sink, 0);
        // A physical handler consumes the first press and opens it: still recorded and dropped.
        let mut rt = opener_focused_on(2);
        assert_eq!(focused_key(&rt).as_deref(), Some("grab"));
        press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, false);
        assert_eq!(focused_key(&rt).as_deref(), Some("sink"));
        press(&mut rt, PhysicalKeyCode::Enter, Key::Enter, true);
        assert_eq!(rt.state.sink, 0);
    }

    #[test]
    fn a_repeat_queued_in_the_same_frame_sees_the_focus_its_press_moved() {
        let mut rt = opener_focused_on(1);
        let mods = Mods::default();
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Space, mods, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Text(" ".into()), mods }));
        rt.physical_with_fallback(PhysicalInput::Key { key: PhysicalKeyCode::Space, mods, pressed: true, repeat: true },
            Some(KeyEvent { key: Key::Text(" ".into()), mods }));
        rt.frame(0.0);
        assert_eq!(focused_key(&rt).as_deref(), Some("sink"));
        assert_eq!((rt.state.go_repeats, rt.state.sink), (0, 0), "the repeat went nowhere");
    }

    /// A `keys_first` menu item's key: the focused field, then `on_key`, see it before the item,
    /// which runs only when neither takes it (and on each repeat, as a menu equivalent does).
    #[test]
    fn a_menu_item_handing_over_its_key_runs_only_when_no_handler_takes_it() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let mut rt = fields_runtime(false);
        rt.on_key = Some(|s, ev, _| { s.app_keys += 1; ev.is_command(']') });
        let ran = Arc::new(AtomicUsize::new(0));
        let item = { let ran = ran.clone(); Unhandled::Menu(Arc::new(move || { ran.fetch_add(1, Ordering::Relaxed); })) };
        let mods = Mods { cmd: true, ..Mods::default() };
        let chord = |rt: &mut Runtime<Fields>, code, c, repeat| {
            rt.physical_then(PhysicalInput::Key { key: code, mods, pressed: true, repeat },
                Some(KeyEvent { key: Key::Command(c), mods }), None, Some(item.clone()));
            rt.frame(0.0);
            ran.load(Ordering::Relaxed)
        };
        assert_eq!(chord(&mut rt, PhysicalKeyCode::BracketRight, ']', false), 0, "the app's on_key took it");
        assert_eq!(chord(&mut rt, PhysicalKeyCode::BracketLeft, '[', false), 1, "nobody took it: the item runs");
        assert_eq!(chord(&mut rt, PhysicalKeyCode::BracketLeft, '[', true), 2, "and repeats");
        assert_eq!(rt.state.app_keys, 3);
        // A field that takes the key (⌘A selects all) keeps it from the item and from on_key.
        click(&mut rt, "first");
        rt.frame(0.0);
        assert_eq!(chord(&mut rt, PhysicalKeyCode::KeyA, 'a', false), 2);
        assert_eq!(rt.state.app_keys, 3);
    }

    #[test]
    fn physical_paste_uses_app_handler_without_a_focused_editor() {
        fn view(_: &String, _: &mut Cx<String>) -> El {
            el().size(100.0, 100.0).on_paste(|s: &mut String, text, _| *s = text.into())
        }
        let mut rt = Runtime::new(String::new(), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        key(&mut rt, PhysicalKeyCode::KeyV, Key::Command('v'));
        rt.frame(0.0);
        assert_eq!(rt.paste_request, Some(None));
        let target = rt.paste_request.take().unwrap();
        rt.paste_result(target, crate::ClipboardContent::new("app"));
        rt.frame(0.0);
        assert_eq!(rt.state, "app");
    }
}
