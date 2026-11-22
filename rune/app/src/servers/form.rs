//! The universal modal's form variant (`UniversalModal::open_form`, GPUiX `server_forms.rs`):
//! the modal card with a title and close mark, an optional description, Host / Join tabs, a
//! "Fast tunnel" switch, labelled fields (secret ones masked), then the submit and Cancel
//! rows. Tab / Shift+Tab and Up / Down move focus through tabs, switch, fields and buttons;
//! Enter submits (or flips the focused tab or switch); Esc closes.

use crate::app::App;
use crate::ui::theme::rgb;
use rune::{Color, El, Key, KeyEvent, TextEdit, Tx, col, el, row, text};

const WIDTH: f32 = 560.0;
const MARGIN_TOP: f32 = 92.0;
const RADIUS: f32 = 13.0;
const PAD: f32 = 18.0;
const TITLE: f32 = 15.0;
const BODY: f32 = 12.0;
const LINE_H: f32 = 19.0;
const ACTION: f32 = 13.0;
const ACTION_H: f32 = 32.0;
const FIELD_H: f32 = 32.0;

/// What the form edits; its draft survives tab and switch changes (`serverDraftValues`).
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Server(Draft),
    System,
}

/// `ServerDraft`: Host or Join, Fast tunnel, and every field's last value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub host: bool,
    pub fast: bool,
    pub directory: String,
    pub address: String,
    pub name: String,
    pub password: String,
    /// Editing a saved server.
    pub server_id: Option<String>,
}

pub struct Field {
    pub id: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub edit: TextEdit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tabs,
    Switch,
    Field(usize),
    Submit,
    Cancel,
}

pub struct Form {
    pub kind: Kind,
    pub title: &'static str,
    pub description: Option<&'static str>,
    pub fields: Vec<Field>,
    pub submit: &'static str,
    pub focus: Focus,
    pub error: Option<String>,
}

fn field(id: &'static str, label: &'static str, value: &str, placeholder: &'static str, secret: bool) -> Field {
    let mut edit = TextEdit::new(value);
    if secret { edit = edit.secure(); }
    Field { id, label, placeholder, edit }
}

impl Form {
    /// `server-form`: the target field by mode (project folder to host, address or invite to
    /// join, address when editing), then the optional name and the password.
    pub fn server(draft: Draft) -> Form {
        let edit = draft.server_id.is_some();
        let target = if edit {
            field("address", "Server address", &draft.address, "ws://192.168.1.20:7981/session", false)
        } else if draft.host {
            field(if draft.fast { "fast_host_dir" } else { "create_dir" }, "Project folder", &draft.directory, "~/code/myproject", false)
        } else if draft.fast {
            field("fast_join_address", "Server link", &draft.address, "Paste the Fast Server invite link", false)
        } else {
            field("address", "Server address", &draft.address, "ws://192.168.1.20:7981/session", false)
        };
        let password = if draft.fast && draft.host { "Password (required)" } else { "Password (optional)" };
        let fields = vec![
            target,
            field("name", "Server name (optional)", &draft.name, "Team server", false),
            field("token", password, &draft.password, "password", true),
        ];
        let submit = if edit { "Save server" } else if draft.host { "Host server" } else { "Join server" };
        let mut form = Form { title: if edit { "Edit Server" } else { "Add Server" }, description: None, fields, submit,
            focus: Focus::Field(0), error: None, kind: Kind::Server(draft) };
        form.focus_field(0);
        form
    }

    /// `system-form`.
    pub fn system() -> Form {
        let mut form = Form {
            kind: Kind::System,
            title: "Add system",
            description: Some("New systems are saved deactivated. Turn on Active in Systems when you want to connect."),
            fields: vec![
                field("system_target", "Computer", "", "laptop, hostname, or SSH alias", false),
                field("system_name", "Name (optional)", "", "Mac Studio", false),
            ],
            submit: "Add system",
            focus: Focus::Field(0),
            error: None,
        };
        form.focus_field(0);
        form
    }

    pub fn value(&self, id: &str) -> Option<&str> { self.fields.iter().find(|f| f.id == id).map(|f| f.edit.text.as_str()) }

    /// The draft with the fields' current values (`serverDraftValues`).
    pub fn draft(&self) -> Option<Draft> {
        let Kind::Server(draft) = &self.kind else { return None };
        let mut draft = draft.clone();
        if let Some(dir) = self.value("fast_host_dir").or(self.value("create_dir")) { draft.directory = dir.to_owned(); }
        if let Some(address) = self.value("fast_join_address").or(self.value("address")) { draft.address = address.to_owned(); }
        if let Some(name) = self.value("name") { draft.name = name.to_owned(); }
        if let Some(password) = self.value("token") { draft.password = password.to_owned(); }
        Some(draft)
    }

    fn has_tabs(&self) -> bool { matches!(&self.kind, Kind::Server(d) if d.server_id.is_none()) }

    /// Rebuild for another mode, keeping what was typed (`form-mode`).
    fn remode(&mut self, host: bool, fast: bool) {
        let Some(mut draft) = self.draft() else { return };
        draft.host = host;
        draft.fast = fast;
        let focus = self.focus;
        *self = Form::server(draft);
        self.set_focus(focus);
    }

    fn order(&self) -> Vec<Focus> {
        let mut out = Vec::new();
        if self.has_tabs() { out.extend([Focus::Tabs, Focus::Switch]); }
        out.extend((0..self.fields.len()).map(Focus::Field));
        out.extend([Focus::Submit, Focus::Cancel]);
        out
    }

    fn focus_field(&mut self, index: usize) { self.set_focus(Focus::Field(index)); }

    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        for (i, f) in self.fields.iter_mut().enumerate() { f.edit.focused = focus == Focus::Field(i); }
    }

    /// The field Rune focused (a click into it), else the keyboard's place.
    pub fn current_focus(&self) -> Focus {
        self.fields.iter().position(|f| f.edit.focused).map_or(self.focus, Focus::Field)
    }

    fn sync_focus(&mut self) { self.focus = self.current_focus(); }

    fn step(&mut self, forward: bool) {
        let order = self.order();
        let at = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward { (at + 1) % order.len() } else { (at + order.len() - 1) % order.len() };
        self.set_focus(order[next]);
    }
}

impl App {
    /// Keys while a form is open. Returns true when taken (all of them: it's modal).
    pub fn form_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(form) = &mut self.form else { return false };
        form.sync_focus();
        match event.key {
            Key::Escape => self.form = None,
            Key::Tab => form.step(!event.mods.shift),
            Key::Up => form.step(false),
            Key::Down => form.step(true),
            Key::Left | Key::Right if form.focus == Focus::Tabs => {
                let host = matches!(&form.kind, Kind::Server(d) if d.host);
                let fast = matches!(&form.kind, Kind::Server(d) if d.fast);
                form.remode(!host, fast);
            }
            Key::Enter => match form.focus {
                Focus::Tabs => {
                    let (host, fast) = match &form.kind { Kind::Server(d) => (d.host, d.fast), _ => (false, false) };
                    form.remode(!host, fast);
                }
                Focus::Switch => {
                    let (host, fast) = match &form.kind { Kind::Server(d) => (d.host, d.fast), _ => (false, false) };
                    form.remode(host, !fast);
                }
                Focus::Cancel => self.form = None,
                Focus::Field(_) | Focus::Submit => self.submit_form(tx),
            },
            // Text keys belong to the focused field.
            _ => return false,
        }
        true
    }
}

fn wash(app: &App, on: bool) -> Color { if on { rgb(app.pal.hover) } else { Color::TRANSPARENT } }

pub fn view(app: &App) -> El {
    let Some(form) = &app.form else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let focus = form.current_focus();
    let mut body = col().w_full().mt(10.0 * s).gap(10.0 * s);
    if let Some(description) = form.description {
        body = body.child(text(description).font(app.ty.sized(BODY).lh(LINE_H * s)).color(rgb(p.dim)).wrap().w_full());
    }
    if let Kind::Server(draft) = &form.kind && form.has_tabs() {
        // Host | Join, and the Fast tunnel switch.
        let tab = |label: &'static str, on: bool, host: bool| {
            row().key(format!("form-tab:{label}")).h(28.0 * s).px(14.0 * s).center().radius(6.0 * s)
                .fill(if on { rgb(p.accent).alpha(0.18) } else { rgb(p.surface) })
                .child(text(label).font(if on { app.ty.sized_bold(ACTION) } else { app.ty.sized(ACTION) }).color(rgb(if on { p.fg } else { p.dim })))
                .on_press(move |a: &mut App| {
                    if let Some(form) = &mut a.form { let fast = matches!(&form.kind, Kind::Server(d) if d.fast); form.remode(host, fast); form.set_focus(Focus::Tabs); }
                })
        };
        let tabs = row().gap(6.0 * s).padding(2.0 * s).radius(8.0 * s).border(if focus == Focus::Tabs { s } else { 0.0 }, rgb(p.accent))
            .child(tab("Host", draft.host, true)).child(tab("Join", !draft.host, false));
        let on = draft.fast;
        let knob = 16.0 * s;
        let switch = row().key("form-switch").gap(10.0 * s).center_y().px(6.0 * s).py(3.0 * s).radius(6.0 * s).fill(wash(app, focus == Focus::Switch))
            .child(el().w(40.0 * s).h(22.0 * s).radius(11.0 * s).fill(rgb(if on { p.accent } else { p.border }))
                .child(el().w(knob).h(knob).radius(knob / 2.0).fill(rgb(if on { p.fg } else { p.dim })).absolute(if on { 21.0 * s } else { 3.0 * s }, 3.0 * s)))
            .child(text("Fast tunnel").font(app.ty.sized(ACTION)).color(rgb(p.fg)))
            .on_press(|a: &mut App| {
                if let Some(form) = &mut a.form {
                    let host = matches!(&form.kind, Kind::Server(d) if d.host);
                    let fast = matches!(&form.kind, Kind::Server(d) if d.fast);
                    form.remode(host, !fast);
                    form.set_focus(Focus::Switch);
                }
            });
        body = body.child(row().w_full().center_y().justify_between().child(tabs).child(switch));
    }
    for (index, f) in form.fields.iter().enumerate() {
        let focused = focus == Focus::Field(index);
        let input = rune::text_input(&f.edit, app.ty.sized(ACTION)).key(format!("form-field:{}", f.id))
            .placeholder(f.placeholder.to_owned(), rgb(p.muted)).color(rgb(p.fg)).w_full().single_line()
            .caret(rgb(p.fg), crate::ui::cursor::selection(app))
            .on_edit(move |a: &mut App| &mut a.form.as_mut().expect("form open").fields[index].edit)
            .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(form) = &mut a.form { form.error = None; } })
            .on_input_key(|a: &mut App, e: &KeyEvent, tx: &mut Tx| a.form_key(e, tx))
            .on_submit(|a: &mut App, tx: &mut Tx| a.submit_form(tx));
        body = body.child(col().w_full().gap(5.0 * s)
            .child(text(f.label).font(app.ty.sized(BODY)).color(rgb(p.dim)))
            .child(row().w_full().h(FIELD_H * s).px(10.0 * s).center_y().radius(6.0 * s).fill(rgb(p.surface))
                .border(s, rgb(if focused { p.accent } else { p.border }))
                .on_press(move |a: &mut App| { if let Some(form) = &mut a.form { form.set_focus(Focus::Field(index)); } })
                .child(input)));
    }
    if let Some(error) = &form.error {
        body = body.child(text(error.clone()).font(app.ty.sized(BODY).lh(LINE_H * s)).color(rgb(p.red)).wrap().w_full());
    }
    let action = |label: &str, key: &'static str, on: bool, press: fn(&mut App, &mut Tx)| {
        row().w_full().h(ACTION_H * s).px(10.0 * s).center_y().justify_between().radius(6.0 * s).fill(wash(app, on))
            .child(text(label.to_owned()).font(app.ty.sized(ACTION)).color(rgb(p.fg)))
            .child(text(key).font(app.ty.sized(BODY)).color(rgb(p.muted)))
            .on_press_tx(press)
    };
    let actions = col().w_full().mt(14.0 * s).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).pt(8.0 * s)
        .child(action(form.submit, "Enter", focus == Focus::Submit, |a, tx| a.submit_form(tx)))
        .child(action("Cancel", "Esc", focus == Focus::Cancel, |a, _| a.form = None));
    let card = col().key("form-modal").w((WIDTH * s).min(app.window[0] - 16.0)).padding(PAD * s).radius(RADIUS * s)
        .fill(rgb(p.bg)).border(1.0, rgb(p.border))
        .on_press(|_: &mut App| {})
        .child(row().w_full().center_y().justify_between()
            .child(text(form.title).font(app.ty.sized_bold(TITLE)).color(rgb(p.fg)))
            .child(el().w(24.0 * s).h(24.0 * s).center()
                .child(text("\u{00d7}").font(app.ty.sized(18.0)).color(rgb(p.muted)))
                .on_press(|a: &mut App| a.form = None)))
        .child(body)
        .child(actions);
    col().key("form-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .pt(MARGIN_TOP * s)
        .on_press(|a: &mut App| a.form = None)
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_keep_what_was_typed_and_pick_their_fields() {
        let mut form = Form::server(Draft { host: true, ..Draft::default() });
        assert_eq!(form.fields[0].id, "create_dir");
        assert_eq!(form.submit, "Host server");
        form.fields[0].edit.replace_text("~/code/app");
        form.fields[1].edit.replace_text("Team");
        form.remode(false, false);
        assert_eq!((form.fields[0].id, form.submit), ("address", "Join server"));
        assert_eq!(form.value("name"), Some("Team"));
        form.remode(true, true);
        assert_eq!((form.fields[0].id, form.value("fast_host_dir")), ("fast_host_dir", Some("~/code/app")));
        assert_eq!(form.fields[2].label, "Password (required)");
        let edit = Form::server(Draft { server_id: Some("s".into()), address: "ws://h/session".into(), ..Draft::default() });
        assert_eq!((edit.title, edit.submit, edit.has_tabs()), ("Edit Server", "Save server", false));
    }

    #[test]
    fn focus_cycles_through_tabs_switch_fields_and_buttons() {
        let mut form = Form::server(Draft::default());
        assert_eq!(form.focus, Focus::Field(0));
        assert!(form.fields[0].edit.focused);
        form.step(true);
        form.step(true);
        form.step(true);
        assert_eq!(form.focus, Focus::Submit);
        assert!(form.fields.iter().all(|f| !f.edit.focused));
        form.step(true);
        form.step(true);
        assert_eq!(form.focus, Focus::Tabs);
        form.step(false);
        assert_eq!(form.focus, Focus::Cancel);
    }
}
