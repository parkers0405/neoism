//! The universal modal (`shared/src/widgets/modal.rs`): a 560 pt card 92 pt below the top, 13 pt
//! corners, 18 pt padding, a 15 pt title with a close mark, 12 pt dim body lines on 19 pt, then
//! 32 pt action rows with their key hints. About uses it; confirmations and forms can too.

use super::theme::rgb;
use crate::app::App;
use rune::{Color, El, Key, KeyEvent, Tx, col, el, row, text};

const WIDTH: f32 = 560.0;
const MARGIN_TOP: f32 = 92.0;
const RADIUS: f32 = 13.0;
const PAD: f32 = 18.0;
const TITLE: f32 = 15.0;
const BODY: f32 = 12.0;
const LINE_H: f32 = 19.0;
const ACTION: f32 = 13.0;
const ACTION_H: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Close,
    DiscardTab(u64),
    SaveTab(u64),
    /// Stop the server connection in progress.
    CancelConnect,
    /// Remove the server or system awaiting confirmation.
    ConfirmRemove,
    CopyServerInvite,
}

pub struct Button {
    pub label: String,
    pub key: String,
    pub action: Action,
}

pub struct Modal {
    pub title: String,
    pub body: String,
    pub buttons: Vec<Button>,
    pub selected: usize,
}

impl Modal {
    pub fn fast_server_ready(name: &str, invite: &str) -> Self {
        Self { title: "Fast Server ready".into(),
            body: format!("{name}\n\nInvite a collaborator with this link and your password:\n\n{invite}\n\nClick the link to copy the invite.\nStop hosting ends this temporary session; project files are kept."),
            buttons: vec![Button { label: "Copy invite".into(), key: "Enter".into(), action: Action::CopyServerInvite },
                Button { label: "Close".into(), key: "Esc".into(), action: Action::Close }], selected: 0 }
    }

    pub fn unsaved(tab: u64, title: &str, can_save: bool) -> Self {
        let mut buttons = vec![Button { label: "Keep editing".into(), key: "Esc".into(), action: Action::Close }];
        if can_save { buttons.push(Button { label: "Save and close".into(), key: String::new(), action: Action::SaveTab(tab) }); }
        buttons.push(Button { label: "Discard changes and close".into(), key: String::new(), action: Action::DiscardTab(tab) });
        Self { title: "Unsaved changes".into(), body: format!("{title} has changes that have not been saved."), buttons, selected: 0 }
    }

    /// A message with a single Close button.
    pub fn notice(title: &str, body: &str) -> Modal {
        Modal { title: title.into(), body: body.into(),
            buttons: vec![Button { label: "Close".into(), key: "Esc".into(), action: Action::Close }], selected: 0 }
    }

    /// "About Neoism": the release version and the commit this build came from.
    pub fn about() -> Modal {
        let manifest = include_str!("../../../../Cargo.toml");
        let version = manifest.split("[workspace.package]").nth(1)
            .and_then(|s| s.lines().find(|l| l.trim().starts_with("version =")))
            .and_then(|l| l.split('"').nth(1)).unwrap_or("dev");
        let commit = option_env!("GIT_HASH").unwrap_or("dev");
        Modal {
            title: "About Neoism".into(),
            body: format!("Neoism  v{version}\n\nA terminal-first workspace for code, notes,\nagents, and multiplayer editing.\n\nCommit\n{commit}"),
            buttons: vec![Button { label: "OK".into(), key: "Enter".into(), action: Action::Close }],
            selected: 0,
        }
    }
}

impl App {
    fn activate_modal_action(&mut self, action: Action, tx: &mut Tx) {
        if action == Action::CopyServerInvite {
            if let Some(invite) = self.server.ready_invite.clone() {
                tx.copy(invite.clone());
                self.clipboard = Some(invite);
                if let Some(modal) = &mut self.modal {
                    if let Some(button) = modal.buttons.iter_mut().find(|b| b.action == action) { button.label = "Invite copied".into(); }
                }
            }
        } else { self.run_modal_action(action); }
    }

    pub fn run_modal_action(&mut self, action: Action) {
        self.modal = None;
        match action {
            Action::Close => {},
            Action::DiscardTab(id) => { if let Some(i) = self.tabs.iter().position(|t| t.id == id) { self.close_tab_now(i); } }
            Action::SaveTab(id) => self.save_and_close(id),
            Action::CancelConnect => self.cancel_connect(),
            Action::ConfirmRemove => self.confirm_remove(),
            Action::CopyServerInvite => {},
        }
    }

    /// Esc, the close mark or a click outside: a connection in progress is cancelled with it.
    pub fn dismiss_modal(&mut self) {
        let cancels = self.modal.as_ref().is_some_and(|m| m.buttons.iter().any(|b| b.action == Action::CancelConnect));
        self.modal = None;
        self.server.confirm = None;
        if cancels { self.cancel_connect(); }
    }

    pub fn modal_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(modal) = &mut self.modal else { return false };
        let count = modal.buttons.len().max(1);
        match event.key {
            Key::Escape => self.dismiss_modal(),
            Key::Up => modal.selected = (modal.selected + count - 1) % count,
            Key::Down | Key::Tab => modal.selected = (modal.selected + 1) % count,
            Key::Enter => {
                let action = modal.buttons.get(modal.selected).map_or(Action::Close, |b| b.action);
                self.activate_modal_action(action, tx);
            }
            _ => {}
        }
        true
    }
}

pub fn view(app: &App) -> El {
    let Some(modal) = &app.modal else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let mut body = col().w_full().mt(10.0 * s);
    for line in modal.body.lines() {
        let invite = modal.buttons.iter().any(|b| b.action == Action::CopyServerInvite) && app.server.ready_invite.as_deref() == Some(line);
        if invite {
            body = body.child(el().w_full().min_h(LINE_H * s).center_y()
                .child(text(line.to_owned()).font(app.ty.sized(BODY).lh(LINE_H * s)).color(rgb(p.accent)).wrap().w_full())
                .on_press_tx(|a: &mut App, tx: &mut Tx| a.activate_modal_action(Action::CopyServerInvite, tx)));
            continue;
        }
        // A long line wraps inside the card (on the same 19 pt lines) instead of running out of it.
        body = body.child(el().w_full().min_h(LINE_H * s).center_y()
            .child(text(line.to_owned()).font(app.ty.sized(BODY).lh(LINE_H * s)).color(rgb(p.dim)).wrap().w_full()));
    }
    let mut actions = col().w_full().mt(14.0 * s).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).pt(8.0 * s);
    for (index, button) in modal.buttons.iter().enumerate() {
        let selected = index == modal.selected;
        let action = button.action;
        actions = actions.child(row().w_full().h(ACTION_H * s).px(10.0 * s).center_y().justify_between().radius(6.0 * s)
            .fill(if selected { rgb(p.hover) } else { Color::TRANSPARENT })
            .child(text(button.label.clone()).font(app.ty.sized(ACTION)).color(rgb(p.fg)))
            .child(text(button.key.clone()).font(app.ty.sized(BODY)).color(rgb(p.muted)))
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.activate_modal_action(action, tx)));
    }
    let card = col().key("modal").w((WIDTH * s).min(app.window[0] - 16.0)).padding(PAD * s).radius(RADIUS * s)
        .fill(rgb(p.bg)).border(1.0, rgb(p.border))
        .on_press(|_: &mut App| {})
        .child(row().w_full().center_y().justify_between()
            .child(text(modal.title.clone()).font(app.ty.sized_bold(TITLE)).color(rgb(p.fg)))
            .child(el().w(24.0 * s).h(24.0 * s).center()
                .child(text("\u{00d7}").font(app.ty.sized(18.0)).color(rgb(p.muted)))
                .on_press(|a: &mut App| a.dismiss_modal())))
        .child(body)
        .child(actions);
    col().key("modal-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .pt(MARGIN_TOP * s)
        .on_press(|a: &mut App| a.dismiss_modal())
        .child(card)
}

/// Swallow keys while a modal is open (the caller routes them here first).
pub fn keys(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool { app.modal_key(event, tx) }
