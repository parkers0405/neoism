//! Keyboard, IME, focused-field editing and clipboard paste routing.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Offer a logical key that already passed `held_gate` to the app, the focused
    /// input's handler, focus traversal and the focused editor, in that order.
    pub(super) fn route_key(&mut self, ev: &KeyEvent, repeat: bool, one: &mut Tx) -> bool {
        let physical_key = one.physical_key;
        one.focused = self.focused_key();
        if matches!(ev.key, crate::Key::Modifier { .. }) { return !repeat && self.route_modifier(ev, one); }
        one.key_repeat = repeat;
        let shown = self.focus.keyboard() || self.focus.hovered();
        if focus::navigates(ev) { self.focus.step(focus_machine::FocusEvent::Navigated); }
        // With no focused element, app-owned navigation (e.g. a toolbar)
        // gets first refusal before Rune starts a new Tab traversal.
        let app_first = self.focus.path().is_none();
        // Return and Escape that reach a field mid-composition belong to the input method: they
        // confirm or cancel it, and submit, blur and press nothing.
        if matches!(ev.key, crate::Key::Enter | crate::Key::Escape) && self.composing() { return true; }
        if self.element_key(ev, one) { return true; }
        one.physical_key = None;
        if self.default_button_key(ev, one) { return true; }
        one.physical_key = physical_key;
        if self.playground_nav(ev, one) { return true; }
        if app_first && !self.playground_typing() && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) { return true; }
        if self.focused_input().is_some() {
            self.input_click.clear_last();
        }
        if let Some(f) = self.focused_input().and_then(|input| input.key.clone())
            && Tx::took(f(states!(self), ev, one), one)
        {
            return true;
        }
        one.physical_key = None;
        // Inside a list item or code block, Tab (and Shift+Tab in lists) edits the document by default.
        if ev.key == crate::edit::Key::Tab
            && !ev.mods.cmd
            && !ev.mods.alt
            && !ev.mods.ctrl
            && self.focused_input().is_some_and(|i| {
                !i.single_line
                    && !i.edit.is_secure()
                    && (i.edit.selection_in_list() || (!ev.mods.shift && i.edit.active_block() == crate::edit::Block::Code))
            })
        {
            let layout = self.focused_layout();
            if self.with_focused(|e| e.key(ev, layout.as_deref()), one) {
                return true;
            }
        }
        if self.focus_key(ev, shown, one) { return true; }
        let input_handles = match &ev.key {
            crate::edit::Key::Function(_) | crate::edit::Key::ContextMenu | crate::edit::Key::Tab | crate::edit::Key::PageUp | crate::edit::Key::PageDown
            | crate::edit::Key::Insert | crate::edit::Key::PrintScreen | crate::edit::Key::ScrollLock | crate::edit::Key::Pause => false,
            crate::edit::Key::Command(_) => match ev.edit_command() {
                Some('a' | 'c' | 'x' | 'z') => true,
                Some('y') => !cfg!(target_os = "macos"),
                Some('b' | 'i' | 'u') => self.focused_input().is_some_and(|i| i.edit.format_chord(ev).is_some()),
                _ => false,
            },
            // Alt+Space is the window menu's, so a field leaves it to the shell.
            _ if ev.is_window_menu_chord() => false,
            _ => !ev.is_command_chord() || ev.control_binding().is_some(),
        };
        if input_handles {
            let layout = self.focused_layout();
            // A rich field copies its selection's formatting too, read before a cut removes it.
            let mut html = None;
            let copies = matches!(ev.edit_command(), Some('c' | 'x'));
            let handled = self.with_focused(|e| {
                if copies && e.is_rich() && !e.is_secure() {
                    html = Some(e.selected_html()).filter(|h| !h.is_empty());
                }
                e.key(ev, layout.as_deref())
            }, one);
            if let Some(html) = html
                && let Some(text) = one.copy.as_ref().filter(|w| !w.is_secret()).map(|w| w.content().text().to_owned())
            {
                one.copy_rich(crate::ClipboardContent::new(text).with_html(html));
            }
            if handled { return true; }
        }
        one.physical_key = physical_key;
        if !app_first && !self.playground_typing() && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) {
            log_event!(self, "key {:?}", ev.key);
            return true;
        }
        one.physical_key = None;
        // Selected text takes the Cmd+A and Cmd+C nothing else took, as AppKit's responder chain
        // gives a key equivalent to the menu bar before the text view: an app's shortcut wins.
        if !repeat && self.selection_key(ev, one) { return true; }
        if !repeat && self.context_menu_key(ev, one) { return true; }
        self.page_scroll(ev) || self.playground_key(ev, one)
    }

    /// `AppSpec::on_key`, given the stage's state while the playground shows one of the app's.
    /// The playground's list and side column have already taken the keys they use.
    pub(super) fn app_key(&mut self, f: fn(&mut S, &KeyEvent, &mut Tx) -> bool, ev: &KeyEvent, one: &mut Tx) -> bool {
        let state = match &mut self.playground {
            Some(playground) => playground.key_state(&mut self.state),
            None => Some(&mut self.state),
        };
        state.is_some_and(|state| app_call!(f, state, ev, one))
    }

    /// Page Up and Page Down scroll by most of a viewport, and Home and End to the top and
    /// bottom, as an NSScrollView does, when nothing else took the key. The scroll view is the
    /// nearest one around the focused element, else around the pointer, else the first in the
    /// tree, skipping any whose content fits (nothing to scroll): a chat's short composer passes
    /// the keys to the message list beside it. A view that overflows keeps them even at the edge
    /// a key moves toward, where it does nothing, as an NSTextView does; an app that wants
    /// another view paged there routes the key itself with `Tx::scroll_page`.
    pub(super) fn page_scroll(&mut self, ev: &KeyEvent) -> bool {
        use crate::Key::*;
        if !matches!(ev.key, PageUp | PageDown | Home | End) || ev.mods.cmd || ev.mods.alt || ev.mods.ctrl { return false; }
        // A focused field in a virtual row that scrolled out has no node: page its list.
        let focused = self.focus.path().and_then(|path| {
            std::iter::successors(Some(path), |p| p.rsplit_once('/').map(|(head, _)| head))
                .find_map(|p| self.nodes.iter().position(|n| n.path == p))
        });
        // A step's key (`Play::press`) pages only the stage, never the playground around it.
        let staged = self.playground.as_ref().is_some_and(|p| p.pressing);
        let pageable = |i: usize| self.pageable(i) && (!staged || self.under(i, crate::playground::is_stage));
        let around = |from: usize| std::iter::successors(Some(from), |&i| self.nodes[i].parent).find(|&i| pageable(i));
        let i = focused.and_then(around)
            .or_else(|| self.pointer.and_then(|(x, y)| self.scroll_at(x, y)).and_then(|id| self.node_index.get(&id).copied()).and_then(around))
            .or_else(|| (0..self.nodes.len()).find(|&i| pageable(i)));
        let Some(i) = i else { return false };
        if !self.page_to(i, |offset, max, page| match ev.key { PageUp => offset - page, PageDown => offset + page, Home => 0.0, _ => max }) { return false; }
        log_event!(self, "page {:?}", ev.key);
        true
    }

    /// Whether node `i` is a live scroll view whose content overflows it.
    pub(super) fn pageable(&self, i: usize) -> bool {
        self.nodes[i].scroll.is_some() && !self.subtree_exiting(i) && self.allowed(i)
            && self.scrolls.get(&self.nodes[i].id).is_some_and(|st| st.max > 0.5)
    }

    /// Moves scroll view `i` to `to(offset, max, page)`, clamped, a page being 90% of its
    /// height. The hand moved it: any fling or glide stops, and a `stick_bottom` view that
    /// lands on its end follows it again.
    pub(super) fn page_to(&mut self, i: usize, to: impl FnOnce(f64, f64, f64) -> f64) -> bool {
        self.release_hover_hold();
        self.drop_scroll_holds();
        let (id, page, stick) = (self.nodes[i].id, f64::from(self.nodes[i].local.h * 0.9), self.nodes[i].scroll.is_some_and(|s| s.stick_bottom));
        let Some(st) = self.scrolls.get_mut(&id) else { return false };
        st.offset = to(st.offset, st.max, page).clamp(0.0, st.max);
        st.direct = true;
        st.land(stick);
        true
    }

    pub(super) fn focused_layout(&mut self) -> Option<Rc<Layout<Ink>>> {
        let path = self.focus.owned()?;
        let s = self.env.scale;
        let n = self.node(&path)?;
        let (mut spec, w) = (n.text.clone()?, n.target[2] - n.target[0] - n.inset[0] - n.inset[2]);
        spec.replace_content(n.input.as_ref()?.edit.display().0);
        Some(self.text.layout_for_spec(&spec, s, spec.wrap.then_some(w * s), line_alignment(&spec)))
    }

    pub(super) fn with_focused(&mut self, f: impl FnOnce(&mut crate::edit::TextEdit) -> EditOutcome, tx: &mut Tx) -> bool {
        self.with_focused_state(|state, access, _| f(access(state)), tx)
    }

    /// [`Self::with_focused`] for an edit that needs the app state (an app's paste hook).
    pub(super) fn with_focused_state(&mut self, f: impl FnOnce(crate::states::States<'_>, &crate::el::EditFn, &mut Tx) -> EditOutcome, tx: &mut Tx) -> bool {
        let Some(path) = self.focus.owned() else { return false };
        let Some((access, submit, change, animate_layout, single_line, secure, revealed_override)) =
            self.focused_input().map(|i| (i.access.clone(), i.submit.clone(), i.change.clone(), i.animate_layout, i.single_line, i.edit.is_secure(), i.revealed_override))
        else {
            return false;
        };
        let Some(access) = access else { return false };
        self.input_click.clear_last();
        {
            let edit = access(states!(self));
            edit.set_single_line(single_line);
            if secure && !edit.is_secure() { edit.set_secure(true); }
            if let Some(rev) = revealed_override { edit.set_revealed(rev); }
        }
        let before = change.as_ref().map(|_| zeroize::Zeroizing::new(access(states!(self)).text.clone()));
        let mut out = f(states!(self), &access, tx);
        // Submit and Escape's blur act once per press; held keys still type, move and delete.
        if tx.key_repeat { out.submit = false; out.blur = false; }
        // Typing is the user's own hand: no animation.
        tx.instant = !animate_layout;
        self.caret_follow = true;
        if let Some(change) = change
            && before.as_ref().map(|s| s.as_str()) != Some(access(states!(self)).text.as_str())
        {
            change(states!(self), tx);
            access(states!(self)).mark_edited();
        }
        if let Some(c) = out.copy {
            tx.copy(c);
        }
        if out.blur {
            self.set_focus(None, self.focus.keyboard());
        }
        if out.submit
            && let Some(sub) = submit
        {
            log_event!(self, "submit {path}");
            tx.instant = false;
            sub(states!(self), tx);
        } else if out.submit {
            // A field with nothing to submit passes Return to the default button, as in a sheet.
            self.press_default_button(tx);
        }
        true
    }

    /// Whether the focused field has an input method composition (preedit) open.
    pub(super) fn composing(&mut self) -> bool {
        let Some(access) = self.focused_input().map(|i| i.access.clone()) else { return self.app_ime.composing() };
        match access {
            Some(access) => access(states!(self)).preedit().is_some(),
            None => self.focused_input().is_some_and(|i| i.edit.preedit().is_some()),
        }
    }

    pub(super) fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.iter().enumerate().find(|(i, n)| n.path == path && !self.subtree_exiting(*i) && !self.subtree_unmounted(*i)).map(|(_, n)| n)
    }

    /// `El::on_key` handlers on the focused element and its ancestors, innermost first. A
    /// focused text field's keys stay its own.
    pub(super) fn element_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(i) = self.focused_index() else { return false };
        if !self.can_focus(i) || self.focused_input().is_some() { return false; }
        let handlers: Vec<_> = std::iter::successors(Some(i), |&j| self.nodes[j].parent)
            .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
            .filter_map(|j| self.nodes[j].focus.key.clone())
            .collect();
        handlers.into_iter().any(|f| Tx::took(f(states!(self), ev, tx), tx))
    }

    pub(super) fn focused_input(&self) -> Option<&InputSpec> {
        let path = self.focus.path()?;
        self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.virtual_inputs.get(path).map(Box::as_ref))
    }

    /// What [`Tx::focused`] reports: the focused link's id, else the focused element's key,
    /// else its path's last segment (a field in a virtual row that scrolled out has no node).
    pub(super) fn focused_key(&self) -> Option<String> {
        let path = self.focus.path()?;
        if let Some(id) = self.focused_index().and_then(|i| self.focused_link_id(i)) { return Some(id); }
        Some(self.focused_index().and_then(|i| self.nodes[i].key.clone())
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned()))
    }

    /// The focused element's index. Unlike [`Runtime::node`], this finds it while it exits.
    pub(super) fn focused_index(&self) -> Option<usize> {
        let path = self.focus.path()?;
        self.nodes.iter().position(|n| n.path == path)
    }

    pub fn key(&mut self, ev: KeyEvent) {
        self.queue_key(ev, false);
    }

    /// [`Runtime::key`]; `stage` routes it to the open playground's stage, for `Play::press`.
    pub(super) fn queue_key(&mut self, ev: KeyEvent, stage: bool) {
        self.dismiss_help();
        // Escape during a drag cancels it and goes no further.
        if ev.key == crate::Key::Escape && self.cancel_drag() { return; }
        self.queue.push(Action::Key { ev, repeat: false, stage });
    }

    /// Queue an auto-repeat of a held key, as the OS sends after the first press. Handlers see
    /// `Tx::key_repeat` true for it.
    pub fn repeat_key(&mut self, ev: KeyEvent) {
        self.queue.push(Action::Key { ev, repeat: true, stage: false });
    }

    pub fn ime(&mut self, ev: Ime) {
        self.queue.push(Action::Ime(ev));
    }

    /// End native composition on window deactivation without discarding text or field focus.
    pub fn cancel_composition(&mut self) {
        self.ime(Ime::Preedit(String::new(), None));
        self.ime_reset = true;
    }

    pub fn paste(&mut self, text: impl Into<String>) {
        self.queue.push(Action::Paste(crate::ClipboardContent::new(text), None));
    }
    /// [`Runtime::paste`] with several forms, as from another app's clipboard: its HTML is
    /// sanitised as a real paste's is. For tests and scripts.
    pub fn paste_content(&mut self, content: crate::ClipboardContent) {
        self.queue.push(Action::Paste(content.untrusted(), None));
    }

    pub(super) fn app_paste_handler(&self) -> Option<crate::el::PasteFn> {
        self.nodes.get(self.active_modal().unwrap_or_else(|| self.app_root()))?.handlers.on_paste.clone()
    }
    pub(crate) fn accepts_paste(&self) -> bool {
        self.paste_target().is_some() || self.app_paste_handler().is_some()
    }
    pub(crate) fn paste_target(&self) -> Option<String> {
        self.focused_input().filter(|i| i.access.is_some()).and(self.focus.owned())
    }
    /// What the paste target hears: every form, images too, for a field with
    /// `on_input_paste` or a root `on_paste_content` handler; text, HTML and RTF for a rich
    /// [`TextEdit`](crate::TextEdit); only the clipboard's text for anything else (including a
    /// plain field or a plain `on_paste`).
    pub(crate) fn paste_forms(&self, target: Option<&str>) -> crate::clipboard::Forms {
        use crate::clipboard::Forms;
        match target {
            Some(path) => match self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.virtual_inputs.get(path).map(Box::as_ref)) {
                Some(input) if input.paste.is_some() => Forms::All,
                Some(input) if input.edit.is_rich() && !input.edit.is_secure() => Forms::Rich,
                _ => Forms::Text,
            },
            None if self.app_paste_handler().is_some_and(|p| p.rich) => Forms::All,
            None => Forms::Text,
        }
    }
    pub(crate) fn paste_result(&mut self, target: Option<String>, content: crate::ClipboardContent) {
        self.queue.push(Action::Paste(content, Some(target)));
    }
    pub(crate) fn read_result(&mut self, id: u64, content: crate::ClipboardContent) {
        self.queue.push(Action::ClipboardRead(id, content));
    }

    /// Whether the focused editor requires secure native keyboard input.
    pub fn has_secure_focus(&self) -> bool {
        self.focused_input().is_some_and(|i| i.edit.is_secure())
    }

    pub fn has_focus(&self) -> bool {
        self.focus.path().is_some()
    }
}
