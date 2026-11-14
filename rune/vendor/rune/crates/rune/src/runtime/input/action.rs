//! The actions input and layout queue for the next frame's handlers, and how a frame runs them.

use super::*;
use crate::time::Instant;

pub(super) enum Action {
    WindowFocus(bool),
    Physical(crate::PhysicalInput, Option<physical_dispatch::Fallback>),
    FocusControl(String),
    /// The pointer entered a `focus_highlight` row (`Some`) or left every such row.
    HoverFocus(Option<String>),
    Dismiss(String),
    PointerMove(String, PointerEvent),
    Hover(String),
    HoverChange(String, bool),
    /// `El::on_selection_change`.
    SelectionChange(String, crate::SelectionChange),
    AtBottom(String, bool),
    FarFromBottom(String, bool),
    VisibleRows(String, std::ops::Range<usize>),
    Layout(String, [f32; 4]),
    ScreenLayout(String, [f32; 4]),
    /// `El::on_drawn_layout`; true when only a live value, scroll or animation moved it.
    DrawnLayout(String, [f32; 4], bool),
    /// `El::on_visible`: the element came on screen or left it.
    Visible(String, bool),
    Press(String, crate::PointerButton),
    DoublePress(String),
    Link(String, usize),
    /// `El::on_link_secondary`: the text, its link and the press.
    LinkSecondary(String, usize, PointerEvent),
    SecondaryPress(String, PointerEvent),
    Drag(String, Drag, crate::PointerButton),
    /// A held press's drag hearing the pointer again (`Tx::drag_repeat_after`).
    DragRepeat(String, Drag, crate::PointerButton),
    DragStart(String),
    DragEnd(String, crate::DragOutcome),
    /// A file drag from another app moved, left, or dropped (`drop_dispatch`).
    Files(drop_dispatch::FileEvent),
    /// `stage`: a playground step pressed it, for the stage (`Play::press`).
    Key { ev: KeyEvent, repeat: bool, stage: bool },
    KeyUp(KeyEvent),
    /// A wheel delta at window point (x, y) for the `on_wheel` handlers at the pointer,
    /// innermost first (`runtime/input/gesture.rs`).
    Wheel(u64, Vec<String>, crate::WheelEvent),
    Gesture(Vec<String>, gesture_dispatch::Gesture),
    Ime(Ime),
    Paste(crate::ClipboardContent, Option<Option<String>>),
    /// A `Tx::read_clipboard` callback's id and what the clipboard held.
    ClipboardRead(u64, crate::ClipboardContent),
    /// A `Tx::open_file` or `Tx::save_file` callback's id and the dialog's answer.
    FileDialog(u64, crate::file_dialog::FileDialogResult),
    Focus(String, f32, f32),
    /// A text field's menu: the field, the window point it opens at, and where in the field a
    /// press asked for it (none from the keyboard).
    FieldMenu(String, (f32, f32), Option<(f32, f32)>),
    Blur,
    /// What assistive technology asked of an element (`a11y/actions.rs`).
    A11y(String, A11yAct),
}

/// A queued action and when the input that queued it arrived (`Tx::input_time`): the arrival
/// time its host dispatched the input under (`input_time::scope`), none for what layout,
/// timers, updates and polls queue.
pub(super) struct Queued {
    pub(super) action: Action,
    pub(super) at: Option<Instant>,
}

/// The actions waiting for the next frame, in order, each with its input's arrival time.
#[derive(Default)]
pub(super) struct ActionQueue(Vec<Queued>);

impl ActionQueue {
    /// Queues `action` stamped with the input being dispatched now, if any.
    pub(super) fn push(&mut self, action: Action) { self.0.push(Queued { action, at: crate::input::time::current() }); }
    /// Queues an action again with its own stamp.
    pub(super) fn push_queued(&mut self, queued: Queued) { self.0.push(queued); }
    /// Puts `queued` ahead of everything queued.
    pub(super) fn prepend(&mut self, queued: Vec<Queued>) { self.0.splice(0..0, queued); }
    pub(super) fn iter(&self) -> impl Iterator<Item = &Action> { self.0.iter().map(|q| &q.action) }
    pub(super) fn len(&self) -> usize { self.0.len() }
    pub(super) fn is_empty(&self) -> bool { self.0.is_empty() }
    pub(super) fn retain(&mut self, mut keep: impl FnMut(&Action) -> bool) { self.0.retain(|q| keep(&q.action)); }
    pub(super) fn remove(&mut self, index: usize) -> Action { self.0.remove(index).action }
}

impl Extend<Action> for ActionQueue {
    fn extend<I: IntoIterator<Item = Action>>(&mut self, actions: I) {
        for action in actions { self.push(action); }
    }
}

impl Extend<Queued> for ActionQueue {
    fn extend<I: IntoIterator<Item = Queued>>(&mut self, queued: I) { self.0.extend(queued); }
}

impl FromIterator<Queued> for ActionQueue {
    fn from_iter<I: IntoIterator<Item = Queued>>(queued: I) -> Self { Self(queued.into_iter().collect()) }
}

impl IntoIterator for ActionQueue {
    type Item = Queued;
    type IntoIter = std::vec::IntoIter<Queued>;
    fn into_iter(self) -> Self::IntoIter { self.0.into_iter() }
}

/// The handler an event goes to, when its `Tx::live_only` can keep the event from committing.
/// Events Rune does its own work for (focus, editing, pastes, file drops) always commit.
pub(super) fn live_only_handler(action: &Action) -> Option<&'static str> {
    Some(match action {
        Action::WindowFocus(_) => "on_window_focus",
        Action::DrawnLayout(..) => "on_drawn_layout_tx",
        Action::Visible(..) => "on_visible",
        Action::Layout(..) => "on_layout_tx",
        Action::PointerMove(..) => "on_pointer_move_local",
        Action::Hover(_) => "on_hover",
        Action::HoverChange(..) => "on_hover_change",
        Action::SelectionChange(..) => "on_selection_change",
        Action::Press(..) => "on_press",
        Action::DoublePress(_) => "on_double_press",
        Action::Link(..) => "on_link",
        Action::LinkSecondary(..) => "on_link_secondary",
        Action::SecondaryPress(..) => "on_secondary_press",
        Action::Drag(..) | Action::DragRepeat(..) => "on_drag",
        Action::DragStart(_) => "on_drag_start",
        Action::DragEnd(..) => "on_drag_end",
        Action::Key { .. } => "on_key",
        Action::KeyUp(_) => "on_key_up",
        Action::Wheel(..) => "on_wheel",
        Action::Gesture(_, gesture_dispatch::Gesture::Magnify(_)) => "on_magnify",
        Action::Gesture(_, gesture_dispatch::Gesture::Rotate(_)) => "on_rotate",
        Action::Gesture(_, gesture_dispatch::Gesture::SmartMagnify(..)) => "on_smart_magnify",
        _ => return None,
    })
}

#[cfg(debug_assertions)]
pub(super) fn action_path(action: &Action) -> Option<&str> {
    match action {
        Action::PointerMove(path, _) | Action::DrawnLayout(path, ..) | Action::Layout(path, _) | Action::Hover(path) | Action::HoverChange(path, _) | Action::SelectionChange(path, _) | Action::Press(path, _) | Action::DoublePress(path)
        | Action::Link(path, _) | Action::LinkSecondary(path, ..) | Action::SecondaryPress(path, _) | Action::Drag(path, ..) | Action::DragRepeat(path, ..) | Action::DragStart(path)
        | Action::DragEnd(path, _) | Action::Visible(path, _) => Some(path),
        Action::Wheel(_, chain, _) | Action::Gesture(chain, _) => chain.first().map(String::as_str),
        _ => None,
    }
}

/// The handler an action runs, for the stall warning.
#[cfg(debug_assertions)]
fn handler_name(action: &Action) -> &'static str {
    live_only_handler(action).unwrap_or(match action {
        Action::Physical(..) => "on_key",
        Action::FocusControl(_) | Action::HoverFocus(_) | Action::Focus(..) | Action::Blur => "a focus change",
        Action::Dismiss(_) => "a modal's dismiss",
        Action::AtBottom(..) => "on_at_bottom",
        Action::FarFromBottom(..) => "on_far_from_bottom",
        Action::VisibleRows(..) => "on_visible_rows",
        Action::ScreenLayout(..) => "on_screen_layout",
        Action::Files(_) => "on_drop_files",
        Action::Ime(_) => "a text input's edit",
        Action::Paste(..) => "on_paste",
        Action::ClipboardRead(..) => "a `read_clipboard` callback",
        Action::FileDialog(..) => "a file dialog callback",
        _ => "a handler",
    })
}

#[cfg(debug_assertions)]
fn stall_path(action: &Action) -> Option<&str> {
    match action {
        Action::Dismiss(path) | Action::AtBottom(path, _) | Action::FarFromBottom(path, _) | Action::VisibleRows(path, _)
        | Action::Layout(path, _) | Action::ScreenLayout(path, _) | Action::Focus(path, ..) | Action::FocusControl(path) => Some(path),
        _ => action_path(action),
    }
}

pub(super) fn merge(tx: &mut Tx, one: Tx) {
    tx.pick_reads.extend(one.pick_reads);
    tx.paste |= one.paste;
    tx.clipboard_reads.extend(one.clipboard_reads);
    if let Some(one) = one.trace {
        let trace = tx.trace.get_or_insert_with(Default::default);
        trace.events.extend(one.events);
        trace.channels.extend(one.channels);
    }
    tx.scroll_to.extend(one.scroll_to);
    tx.scroll_bottom.extend(one.scroll_bottom);
    tx.scroll_pages.extend(one.scroll_pages);
    tx.scroll_offsets.extend(one.scroll_offsets);
    tx.keep_on_screen.extend(one.keep_on_screen);
    tx.evict_memos.extend(one.evict_memos);
    tx.wakes.extend(one.wakes);
    tx.text_flights.extend(one.text_flights);
    tx.launches.extend(one.launches);
    tx.unclipped_flights |= one.unclipped_flights;
    tx.popups.extend(one.popups);
    tx.file_dialogs.extend(one.file_dialogs);
    if let Some(request) = one.window_frame { tx.window_frame = Some(tx.window_frame.unwrap_or_default().merge(request)); }
    if one.clear_secret && tx.copy.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { tx.copy = None; }
    tx.clear_secret |= one.clear_secret;
    if one.copy.is_some() { tx.copy = one.copy; }
    tx.shell = tx.shell.max(one.shell);
    tx.minimize |= one.minimize;
    tx.toggle_maximized |= one.toggle_maximized;
    tx.toggle_full_screen |= one.toggle_full_screen;
    tx.keys.extend(one.keys);
    if one.title.is_some() { tx.title = one.title; }
    tx.playground.extend(one.playground);
    tx.open_urls.extend(one.open_urls);
    tx.srgb_blend = one.srgb_blend.or(tx.srgb_blend);
    tx.font_smoothing = one.font_smoothing.or(tx.font_smoothing);
    if one.font_families.is_some() { tx.font_families = one.font_families; }
    if one.rewind.is_some() { tx.rewind = one.rewind; }
    tx.blur |= one.blur;
    if one.focus_to.is_some() { tx.focus_to = one.focus_to; }
    if one.select.is_some() {
        tx.select = one.select;
        tx.select_from_change = one.select_from_change;
    }
    tx.instant |= one.instant;
    tx.settle |= one.settle;
    tx.follow_edge |= one.follow_edge;
    if one.motion.is_some() {
        tx.motion = one.motion;
        tx.motion_curve = one.motion_curve;
    }
    tx.color_motion = one.color_motion.or(tx.color_motion);
}

/// Slots the ends hosts reported since the last frame into this frame's actions. One OS drag
/// runs at a time and hosts report ends in order, so each belongs to the oldest open drag: it
/// runs before any later drag's start, and after its own start when both are queued.
pub(super) fn order_drag_ends(queue: ActionQueue, ended: Vec<(String, crate::DragOutcome)>, open: &mut usize) -> ActionQueue {
    let mut ended = ended.into_iter().map(|(path, outcome)| Queued { action: Action::DragEnd(path, outcome), at: None });
    let mut out = Vec::with_capacity(queue.len() + ended.len());
    for queued in queue {
        match &queued.action {
            Action::DragStart(_) => {
                while *open > 0 && let Some(end) = ended.next() {
                    out.push(end);
                    *open -= 1;
                }
                *open += 1;
            }
            Action::DragEnd(..) => *open = open.saturating_sub(1),
            _ => {}
        }
        out.push(queued);
    }
    for end in ended {
        *open = open.saturating_sub(1);
        out.push(end);
    }
    ActionQueue(out)
}

impl<S: 'static> Runtime<S> {
    /// Runs `action`'s handlers; debug builds warn when they block the main thread.
    pub(super) fn run_action(&mut self, action: Action, tx: &mut Tx) -> bool {
        #[cfg(debug_assertions)]
        let (name, path, at) = (handler_name(&action), stall_path(&action).map(str::to_owned), self.handler_location(&action));
        let stall = crate::tooling::perf_warn::Stall::start();
        let timed = self.interaction_note(&action).map(|note| (note, crate::time::Instant::now()));
        let ran = self.dispatch_action(action, tx);
        if ran && let Some((note, started)) = timed { self.interaction_ran(note, started); }
        #[cfg(debug_assertions)]
        if ran && name.starts_with("on_") { self.watch.meter.handler(name, path.as_deref(), at); }
        #[cfg(debug_assertions)]
        stall.end(|| match path {
            Some(path) => (crate::tooling::perf_warn::handler_site(name, &path), format!("`{name}` on {path:?}")),
            None => (name.to_owned(), format!("`{name}`")),
        });
        #[cfg(not(debug_assertions))]
        stall.end(|| unreachable!());
        ran
    }

    /// Where the app set the handler `action` runs, for the commit warnings.
    #[cfg(debug_assertions)]
    fn handler_location(&self, action: &Action) -> Option<&'static std::panic::Location<'static>> {
        let sites = |path: &str| self.node(path).map(|n| n.handlers.sites);
        match action {
            Action::Hover(p) => sites(p)?.hover,
            Action::HoverChange(p, _) => sites(p)?.hover_change,
            Action::PointerMove(p, _) => sites(p)?.pointer_move,
            Action::Layout(p, _) => sites(p)?.layout,
            Action::DrawnLayout(p, ..) => sites(p)?.drawn_layout,
            Action::VisibleRows(p, _) => sites(p)?.visible_rows,
            _ => None,
        }
    }

    fn dispatch_action(&mut self, action: Action, tx: &mut Tx) -> bool {
        let mut one = self.transaction();
        // Debug builds name the element too, for the stale-view warning.
        #[cfg(debug_assertions)]
        let live_label = live_only_handler(&action).map(|h| match action_path(&action) {
            Some(path) => format!("{h} on {path:?}"),
            None => h.to_owned(),
        });
        #[cfg(not(debug_assertions))]
        let live_label = live_only_handler(&action);
        let repeat = matches!(action, Action::DragRepeat(..));
        let ran = match action {
            Action::WindowFocus(focused) => {
                if !focused { self.held = physical_dispatch::Activation::Up; }
                let Some(f) = self.nodes.get(self.app_root()).and_then(|n| n.handlers.on_window_focus.clone()) else { return false };
                f(states!(self), focused, &mut one);
                true
            }
            Action::Physical(event, fallback) => {
                // The window lost key focus (an internal focus move also notifies handlers).
                if matches!(event, crate::PhysicalInput::FocusLost) { self.held = physical_dispatch::Activation::Up; }
                let ran = self.dispatch_physical(event, fallback, tx);
                let live_only = std::mem::take(&mut tx.live_only);
                return if ran && live_only { self.skip_view("a key handler") } else { ran };
            }
            Action::FocusControl(path) => {
                self.set_focus(Some(path), false);
                true
            }
            Action::HoverFocus(path) => match path {
                Some(path) => {
                    self.set_focus(Some(path), false);
                    self.focus.step(focus_machine::FocusEvent::Hover(true));
                    one.instant = true;
                    true
                }
                None => {
                    let lit = self.focus.hovered();
                    self.focus.step(focus_machine::FocusEvent::Hover(false));
                    lit
                }
            },
            Action::Dismiss(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.modal.as_ref().map(|m| m.dismiss.clone())) else { return false };
                self.set_focus(None, self.focus.keyboard());
                f(states!(self), &mut one);
                true
            }
            Action::PointerMove(path, point) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_pointer_move.clone()) else { return false };
                f(states!(self), point, &mut one);
                true
            }
            Action::AtBottom(path, at) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_at_bottom.clone()) else { return false };
                f(states!(self), at);
                true
            }
            Action::FarFromBottom(path, far) => {
                let Some((_, f)) = self.node(&path).and_then(|n| n.handlers.on_far_from_bottom.clone()) else { return false };
                f(states!(self), far);
                true
            }
            Action::VisibleRows(path, range) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_visible_rows.clone()) else { return false };
                f(states!(self), range)
            }
            Action::Layout(path, rect) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_layout.clone()) else { return false };
                f(states!(self), rect, &mut one);
                true
            }
            Action::ScreenLayout(path, rect) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_screen_layout.clone()) else { return false };
                f(states!(self), rect);
                true
            }
            Action::DrawnLayout(path, rect, _) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drawn_layout.clone()) else { return false };
                f(states!(self), rect, &mut one);
                true
            }
            Action::Visible(path, on) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_visible.clone()) else { return false };
                f(states!(self), on, &mut one);
                true
            }
            Action::Hover(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_hover.clone()) else { return false };
                f(states!(self), &mut one);
                true
            }
            Action::HoverChange(path, inside) => {
                let gone = self.hover_gone.iter().position(|g| g.0 == path).map(|i| self.hover_gone.remove(i).1);
                // A leave still reaches an element on its way out.
                let exiting = || self.nodes.iter().find(|n| !inside && n.path == path).and_then(|n| n.handlers.on_hover_change.clone());
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_hover_change.clone()).or(gone).or_else(exiting) else { return false };
                f(states!(self), inside, &mut one);
                true
            }
            Action::SelectionChange(path, change) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_selection_change.clone()) else { return false };
                f(states!(self), &change, &mut one);
                one.select_from_change = one.select.is_some();
                true
            }
            Action::Press(path, button) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_press.clone()) else { return false };
                log_event!(self, "press {path}");
                one.button = button;
                f(states!(self), &mut one);
                true
            }
            Action::DoublePress(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_double_press.clone()) else { return false };
                log_event!(self, "double press {path}");
                f(states!(self), &mut one);
                true
            }
            Action::Link(path, k) => self.press_link(&path, k, &mut one),
            Action::LinkSecondary(path, k, point) => self.secondary_press_link(&path, k, point, &mut one),
            Action::SecondaryPress(path, point) => {
                let Some(n) = self.node(&path).filter(|n| !self.is_exiting(n.id)) else { return false };
                let Some(f) = n.handlers.on_secondary_press.clone() else { return false };
                log_event!(self, "secondary press {path}");
                f(states!(self), point, &mut one);
                true
            }
            Action::Drag(path, mut d, button) | Action::DragRepeat(path, mut d, button) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drag.clone()) else { return false };
                if d.phase == Phase::Move {
                    one.instant = true;
                }
                one.button = button;
                one.drag_repeat = repeat;
                d.data = if d.phase == Phase::Down { 0.0 } else { self.drag_data };
                f(states!(self), &mut d, &mut one);
                self.drag_data = d.data;
                if let Some(after) = one.drag_repeat_after.take() {
                    self.arm_press_repeat(&path, after);
                }
                if d.phase != Phase::Move {
                    log_event!(self, "drag {:?} {path}", d.phase);
                }
                true
            }
            Action::DragStart(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drag_start.clone()) else { return false };
                f(states!(self), &mut one);
                true
            }
            Action::DragEnd(path, outcome) => {
                log_event!(self, "drag-end {outcome:?} {path}");
                let Some(f) = self.node(&path).and_then(|n| n.handlers.on_drag_end.clone()) else { return false };
                f(states!(self), outcome, &mut one);
                true
            }
            Action::Files(event) => self.run_files(event, &mut one),
            Action::Key { ev, repeat, stage } => {
                if self.held_gate(None, Some(&ev.key), repeat) { return false; }
                if ! matches!(ev.key, crate::Key::Modifier { .. }) { self.note_key_down(repeat); }
                if stage { self.route_stage_key(&ev, &mut one) } else { self.route_key(&ev, repeat, &mut one) }
            }
            Action::KeyUp(ev) => self.route_key_up(&ev, &mut one),
            Action::Wheel(gesture, chain, ev) => self.run_wheel(gesture, chain, ev, &mut one),
            Action::Gesture(chain, gesture) => self.run_gesture(chain, gesture, &mut one),
            Action::Ime(ev) => self.run_ime(ev, &mut one),
            Action::Paste(content, target) => {
                if target.is_some_and(|target| target != self.paste_target()) { return false; }
                let hook = self.focused_input().and_then(|i| i.paste.clone());
                let own_pills = if cfg!(not(target_arch = "wasm32")) || self.clipboard.is_some() {
                    content.has_own_pills()
                } else {
                    self.pill_copy.as_ref().is_some_and(|(t, pills)| {
                        t.as_str() == content.text()
                            && content.html().is_some_and(|h| {
                                let frag = crate::TextEdit::from_html(h);
                                frag.pills().count() == pills.len()
                                    && frag.pills().zip(pills).all(|((r1, id1), (r2, id2))| r1 == *r2 && id1 == id2)
                            })
                    })
                };
                let into_field = |mut state: crate::states::States<'_>, access: &crate::el::EditFn, tx: &mut Tx| {
                    if hook.is_some_and(|hook| hook(state.reborrow(), &content, tx)) { return EditOutcome::default(); }
                    let edit = access(state);
                    // A rich field keeps the paste's formatting. The HTML was sanitised when it
                    // was read, and `from_html` keeps only its own safe subset anyway.
                    if let Some(html) = content.html() && edit.is_rich() && !edit.is_secure() {
                        return if own_pills { edit.paste_html_with_pills(html, content.text()) } else { edit.paste_html(html, content.text()) };
                    }
                    if !content.has_plain() { return EditOutcome::default(); }
                    edit.paste(content.text())
                };
                if self.with_focused_state(into_field, &mut one) {
                    log_event!(self, "paste field");
                    true
                } else if let Some(f) = self.app_paste_handler() && (f.call)(states!(self), &content, &mut one) {
                    log_event!(self, "paste window");
                    true
                } else { false }
            }
            Action::ClipboardRead(id, content) => {
                let Some(i) = self.clipboard_reads.iter().position(|(read, _)| *read == id) else { return false };
                let (_, f) = self.clipboard_reads.remove(i);
                f(states!(self), &content, &mut one);
                true
            }
            Action::FileDialog(id, result) => self.run_file_dialog(id, result, &mut one),
            Action::Focus(path, x, y) => self.focus_input(path, x, y, &mut one),
            Action::FieldMenu(path, at, local) => self.field_menu(path, at, local, &mut one),
            Action::Blur => {
                if self.focus.path().is_none() { return false; }
                self.set_focus(None, self.focus.keyboard());
                true
            }
            Action::A11y(path, act) => self.run_a11y(path, act, &mut one),
        };
        let live_only = std::mem::take(&mut one.live_only);
        if live_only && live_label.is_some() {
            // A handler that asks for no commit cannot change another handler's animation.
            one.instant = false;
            one.settle = false;
            one.motion = None;
            one.motion_curve = None;
            one.follow_edge = false;
            one.color_motion = None;
        }
        merge(tx, one);
        if ran && live_only && let Some(label) = live_label {
            return self.skip_view(label);
        }
        ran
    }
}
