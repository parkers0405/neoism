//! Right-click menus for the sidebars (GPUiX `SidebarContext`, `tree-context-actions.ts`,
//! `AgentSidePanelView.tsx`): a file or note row (Edit / Open, New File, New Folder, Rename,
//! Delete), the blank space under them (New File, New Folder at the root), and a chat row
//! (Pin / Unpin, Rename, Delete). Each row shows the letter that picks it. Anything that needs
//! a name or a yes opens a small card over the window first (`Prompt`).
//!
//! What a menu acts on is captured when it opens (the tree, the path, the chat): a reply or
//! a click elsewhere in between changes nothing about it.

use super::theme::{panel_bg, rgb};
use crate::app::{App, Sidebar};
use crate::daemon::Family;
use neoism_protocol::agent::AgentClientMessage;
use rune::{Color, El, Key, KeyEvent, TextEdit, Tx, col, el, row, text};

const WIDTH: f32 = 196.0;
const ITEM_H: f32 = 26.0;
const PAD_Y: f32 = 4.0;
const FONT: f32 = 12.5;
const RADIUS: f32 = 8.0;

/// What a menu was opened on.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A row of the file or notes tree (`path`), or the tree itself (`None`). `directory` is
    /// where a new file lands: the row when it is a folder, else its folder.
    Tree { notes: bool, path: Option<String>, is_dir: bool, directory: String },
    Chat { session: String, title: String, pinned: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command { Open, NewFile, NewFolder, Rename, Delete, Pin, RenameChat, DeleteChat }

impl Command {
    fn label(self, target: &Target) -> &'static str {
        match self {
            Command::Open => "Edit / Open",
            Command::NewFile => "New File",
            Command::NewFolder => "New Folder",
            Command::Rename => "Rename",
            Command::Delete => "Delete",
            Command::Pin => if matches!(target, Target::Chat { pinned: true, .. }) { "Unpin Chat" } else { "Pin Chat" },
            Command::RenameChat => "Rename Chat",
            Command::DeleteChat => "Delete Chat",
        }
    }

    fn hint(self) -> char {
        match self {
            Command::Open => 'e',
            Command::NewFile => 'n',
            Command::NewFolder => 'f',
            Command::Rename | Command::RenameChat => 'r',
            Command::Delete | Command::DeleteChat => 'd',
            Command::Pin => 'p',
        }
    }
}

pub struct Menu {
    at: [f32; 2],
    target: Target,
    items: Vec<Command>,
    selected: usize,
}

/// A name to type or a deletion to confirm, for one command on one target.
pub struct Prompt {
    title: String,
    body: String,
    meta: String,
    accept: &'static str,
    input: Option<TextEdit>,
    target: Target,
    command: Command,
}

fn parent(path: &str) -> &str { path.rsplit_once('/').map_or("", |(dir, _)| dir) }
fn file_name(path: &str) -> &str { path.rsplit('/').next().unwrap_or(path) }
fn join(dir: &str, name: &str) -> String { if dir.is_empty() { name.to_owned() } else { format!("{dir}/{name}") } }

/// A name with no path in it (`tree-context-actions.ts` `name`).
fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

impl App {
    /// Right-click on a tree row (`index`) or on the tree's blank space (`None`).
    pub fn open_tree_context(&mut self, index: Option<usize>, at: [f32; 2]) {
        let Some(sidebar) = self.sidebar.filter(|s| matches!(s, Sidebar::Files | Sidebar::Notes)) else { return };
        let notes = sidebar == Sidebar::Notes;
        let tree = if notes { &mut self.notes } else { &mut self.tree };
        if notes && tree.root.is_none() { return; }
        let row = index.and_then(|i| tree.rows().get(i).cloned());
        if let (Some(i), Some(_)) = (index, &row) { tree.selected = i; }
        let target = match &row {
            Some(row) => Target::Tree { notes, path: Some(row.path.clone()), is_dir: row.entry.is_dir,
                directory: if row.entry.is_dir { row.path.clone() } else { parent(&row.path).to_owned() } },
            None => Target::Tree { notes, path: None, is_dir: false, directory: String::new() },
        };
        let items = match row {
            Some(_) => vec![Command::Open, Command::NewFile, Command::NewFolder, Command::Rename, Command::Delete],
            None => vec![Command::NewFile, Command::NewFolder],
        };
        self.focus = crate::app::Focus::Sidebar;
        self.context = Some(Menu { at, target, items, selected: 0 });
    }

    /// Right-click on a conversation row.
    pub fn open_chat_context(&mut self, session: &str, at: [f32; 2]) {
        let Some(thread) = self.threads.iter().find(|t| t.session_id == session) else { return };
        let target = Target::Chat { session: session.to_owned(), title: thread.title.clone(), pinned: thread.pinned };
        self.context = Some(Menu { at, target, items: vec![Command::Pin, Command::RenameChat, Command::DeleteChat], selected: 0 });
    }

    fn run_context(&mut self, index: usize) {
        let Some(menu) = self.context.take() else { return };
        let Some(command) = menu.items.get(index).copied() else { return };
        let target = menu.target;
        let field = |value: &str| { let mut edit = TextEdit::new(value); edit.focused = true; Some(edit) };
        self.prompt = match (&target, command) {
            (Target::Tree { path: Some(path), is_dir, .. }, Command::Open) => {
                let (path, is_dir) = (path.clone(), *is_dir);
                self.open_tree_path(&target, &path, is_dir);
                None
            }
            (Target::Tree { directory, .. }, Command::NewFile | Command::NewFolder) => Some(Prompt {
                title: if command == Command::NewFile { "New File" } else { "New Folder" }.into(),
                body: format!("Create in {}", if directory.is_empty() { self.tree_root_label(&target) } else { directory.clone() }),
                meta: String::new(), accept: "Create", input: field(""), target, command }),
            (Target::Tree { path: Some(path), .. }, Command::Rename) => Some(Prompt {
                title: "Rename".into(), body: format!("Rename \u{201c}{path}\u{201d}."), meta: String::new(), accept: "Rename",
                input: field(file_name(path)), target, command }),
            (Target::Tree { path: Some(path), is_dir, .. }, Command::Delete) => Some(Prompt {
                title: "Delete?".into(), body: format!("Delete \u{201c}{path}\u{201d}{}?", if *is_dir { " and its contents" } else { "" }),
                meta: "This cannot be undone.".into(), accept: "Delete", input: None, target, command }),
            (Target::Chat { session, pinned, .. }, Command::Pin) => {
                let (session, pinned) = (session.clone(), !*pinned);
                if let Some(thread) = self.threads.iter_mut().find(|t| t.session_id == session) { thread.pinned = pinned; }
                self.send_agent_message(AgentClientMessage::SetPinned { session_id: session, pinned });
                None
            }
            (Target::Chat { title, .. }, Command::RenameChat) => Some(Prompt {
                title: "Rename Chat".into(), body: format!("Rename \u{201c}{title}\u{201d}."), meta: "Changes the conversation title everywhere.".into(),
                accept: "Rename", input: field(title), target, command }),
            (Target::Chat { title, .. }, Command::DeleteChat) => Some(Prompt {
                title: "Delete Chat?".into(), body: format!("Delete \u{201c}{title}\u{201d} and its conversation history?"),
                meta: "This cannot be undone.".into(), accept: "Delete", input: None, target, command }),
            _ => None,
        };
    }

    fn send_agent_message(&self, message: AgentClientMessage) {
        #[cfg(test)]
        self.agent_sent.borrow_mut().push(serde_json::to_string(&message).unwrap_or_default());
        if let Some(daemon) = &self.daemon { daemon.agent(&message); }
    }

    fn tree_root_label(&self, target: &Target) -> String {
        match target {
            Target::Tree { notes: true, .. } => self.notes.root.clone().unwrap_or_default(),
            _ => self.cwd.clone(),
        }
    }

    fn open_tree_path(&mut self, target: &Target, path: &str, is_dir: bool) {
        let Target::Tree { notes, .. } = target else { return };
        let tree = if *notes { &self.notes } else { &self.tree };
        let Some(index) = tree.rows().iter().position(|row| row.path == path) else { return };
        if is_dir { self.tree_activate(Some(index)); } else { let file = tree.open_path(path); self.open_file(&file, None); }
    }

    /// The prompt's accept button (Enter): do what it was opened for.
    pub fn accept_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        let value = prompt.input.as_ref().map(|edit| edit.text.trim().to_owned()).unwrap_or_default();
        match (&prompt.target, prompt.command) {
            (Target::Tree { notes, directory, .. }, Command::NewFile | Command::NewFolder) => {
                if !valid_name(&value) { self.prompt = Some(prompt); return; }
                self.tree_create(*notes, directory, &value, prompt.command == Command::NewFolder);
            }
            (Target::Tree { notes, path: Some(path), .. }, Command::Rename) => {
                if !valid_name(&value) { self.prompt = Some(prompt); return; }
                if value != file_name(path) { self.tree_move_path(*notes, path, &join(parent(path), &value)); }
            }
            (Target::Tree { notes, path: Some(path), .. }, Command::Delete) => {
                let root = if *notes { self.notes.root.clone() } else { Some(self.cwd.clone()) };
                if let (Some(root), Some(daemon)) = (root, &self.daemon) {
                    daemon.send_scoped(Family::Files, serde_json::json!({"Delete": {"path": path}}), &root);
                }
            }
            (Target::Chat { session, .. }, Command::RenameChat) => {
                if value.is_empty() { self.prompt = Some(prompt); return; }
                if let Some(thread) = self.threads.iter_mut().find(|t| &t.session_id == session) { thread.title = value.clone(); }
                self.send_agent_message(AgentClientMessage::SetTitle { session_id: session.clone(), title: value });
            }
            (Target::Chat { session, .. }, Command::DeleteChat) => {
                let session = session.clone();
                self.threads.retain(|t| t.session_id != session);
                self.send_agent_message(AgentClientMessage::DeleteThread { session_id: session });
            }
            _ => {}
        }
    }

    /// Keys while a menu is open: Up/Down move, Enter or a row's letter picks, Escape closes.
    pub fn context_key(&mut self, event: &KeyEvent) -> bool {
        let Some(menu) = &mut self.context else { return false };
        let count = menu.items.len();
        match &event.key {
            Key::Up => menu.selected = (menu.selected + count - 1) % count,
            Key::Down => menu.selected = (menu.selected + 1) % count,
            Key::Enter => { let index = menu.selected; self.run_context(index); }
            Key::Escape => self.context = None,
            Key::Text(typed) => {
                let Some(index) = typed.chars().next().and_then(|c| menu.items.iter().position(|item| item.hint() == c.to_ascii_lowercase())) else { return true };
                self.run_context(index);
            }
            _ => {}
        }
        true
    }

    /// Keys while a prompt is open and its field (if any) did not take them.
    pub fn context_prompt_key(&mut self, event: &KeyEvent) -> bool {
        if self.prompt.is_none() { return false; }
        match event.key {
            Key::Enter => self.accept_prompt(),
            Key::Escape => self.prompt = None,
            _ => {}
        }
        true
    }
}

/// The menu card at the pointer, kept inside the window.
pub fn menu_view(app: &App) -> El {
    let Some(menu) = &app.context else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let height = ITEM_H * s * menu.items.len() as f32 + PAD_Y * s * 2.0;
    let x = menu.at[0].min((app.window[0] - WIDTH * s - 4.0).max(0.0));
    let y = menu.at[1].min((app.window[1] - height - 4.0).max(0.0));
    let last = menu.items.len() - 1;
    let mut rows = col().w_full().py(PAD_Y * s);
    for (index, item) in menu.items.iter().enumerate() {
        let selected = index == menu.selected;
        let radius = if index == 0 || index == last { RADIUS * s * 0.6 } else { 0.0 };
        rows = rows.child(row().key(format!("context-row:{index}")).w_full().h(ITEM_H * s).px(4.0 * s)
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if let (true, Some(menu)) = (inside, &mut a.context) { menu.selected = index; }
            })
            .on_press(move |a: &mut App| a.run_context(index))
            .child(row().w_full().h_full().px(8.0 * s).center_y().justify_between().radius(radius)
                .fill(if selected { rgb(p.hover) } else { Color::TRANSPARENT })
                .child(text(item.label(&menu.target)).font(app.ty.sized(FONT)).color(rgb(if matches!(item, Command::Delete | Command::DeleteChat) { p.red } else { p.fg })))
                .child(text(item.hint().to_string()).font(app.ty.sized(FONT)).color(rgb(p.muted)))));
    }
    el().key("context-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.context = None)
        .on_secondary_press(|a: &mut App, _: rune::PointerPress, _: &mut Tx| a.context = None)
        .child(col().key("context-menu").w(WIDTH * s).h(height).radius(RADIUS * s).fill(rgb(panel_bg(p)))
            .border(s.max(1.0), rgb(p.border))
            .on_press(|_: &mut App| {})
            .absolute(x, y)
            .child(rows))
}

/// The name-or-confirm card, in the universal modal's place and look.
pub fn prompt_view(app: &App) -> El {
    let Some(prompt) = &app.prompt else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let mut card = col().key("context-prompt").w((420.0 * s).min(app.window[0] - 16.0)).padding(18.0 * s).radius(13.0 * s).gap(8.0 * s)
        .fill(rgb(p.bg)).border(1.0, rgb(p.border))
        .on_press(|_: &mut App| {})
        .child(text(prompt.title.clone()).font(app.ty.sized_bold(15.0)).color(rgb(p.fg)))
        .child(text(prompt.body.clone()).font(app.ty.sized(12.0)).color(rgb(p.dim)).wrap().w_full());
    if !prompt.meta.is_empty() {
        card = card.child(text(prompt.meta.clone()).font(app.ty.sized(12.0)).color(rgb(p.muted)).wrap().w_full());
    }
    if let Some(edit) = &prompt.input {
        let input = rune::text_input(edit, app.ty.sized(13.0)).key("context-prompt-field")
            .placeholder("name".to_owned(), rgb(p.muted)).color(rgb(p.fg)).w_full().single_line()
            .caret(rgb(p.fg), crate::ui::cursor::selection(app))
            .on_edit(|a: &mut App| a.prompt.as_mut().and_then(|prompt| prompt.input.as_mut()).expect("prompt field"))
            .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| matches!(e.key, Key::Escape) && a.context_prompt_key(e))
            .on_submit(|a: &mut App, _: &mut Tx| a.accept_prompt());
        card = card.child(row().w_full().h(32.0 * s).px(10.0 * s).center_y().radius(6.0 * s).fill(rgb(p.surface)).child(input));
    }
    let button = |label: &str, key: &str, ink: u32, accept: bool| row().h(30.0 * s).px(12.0 * s).gap(8.0 * s).center_y().radius(6.0 * s)
        .fill(if accept { rgb(p.hover) } else { Color::TRANSPARENT }).hover(rgb(p.hover))
        .child(text(label.to_owned()).font(app.ty.sized(13.0)).color(rgb(ink)))
        .child(text(key.to_owned()).font(app.ty.sized(12.0)).color(rgb(p.muted)))
        .on_press(move |a: &mut App| if accept { a.accept_prompt() } else { a.prompt = None });
    let danger = matches!(prompt.command, Command::Delete | Command::DeleteChat);
    card = card.child(row().w_full().justify_end().gap(6.0 * s).mt(6.0 * s)
        .child(button("Cancel", "Esc", p.dim, false))
        .child(button(prompt.accept, "Enter", if danger { p.red } else { p.fg }, true)));
    col().key("context-prompt-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .fill(Color::BLACK.alpha(0.35))
        .on_press(|a: &mut App| a.prompt = None)
        .child(el().h(92.0 * s))
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_app() -> App {
        let mut app = App::new();
        app.sidebar = Some(Sidebar::Files);
        let entry = |name: &str, is_dir: bool| serde_json::json!({"name": name, "is_dir": is_dir});
        app.apply_dir_listing(".", &serde_json::json!([entry("src", true), entry("a.md", false)]));
        app
    }

    fn key(key: Key) -> KeyEvent { KeyEvent { key, mods: Default::default() } }

    #[test]
    fn a_row_and_blank_space_offer_what_they_can_do() {
        let mut app = tree_app();
        app.open_tree_context(Some(1), [10.0, 10.0]);
        let menu = app.context.as_ref().unwrap();
        assert_eq!(menu.items, [Command::Open, Command::NewFile, Command::NewFolder, Command::Rename, Command::Delete]);
        assert_eq!(menu.target, Target::Tree { notes: false, path: Some("a.md".into()), is_dir: false, directory: String::new() });
        assert_eq!(app.tree.selected, 1, "the row under the pointer is the selection");
        // A folder is where its own new files go.
        app.open_tree_context(Some(0), [10.0, 10.0]);
        assert!(matches!(&app.context.as_ref().unwrap().target, Target::Tree { directory, .. } if directory == "src"));
        // Blank space: only what makes sense at the root.
        app.open_tree_context(None, [10.0, 10.0]);
        assert_eq!(app.context.as_ref().unwrap().items, [Command::NewFile, Command::NewFolder]);
    }

    #[test]
    fn a_letter_picks_its_row_and_a_name_must_be_a_name() {
        let mut app = tree_app();
        app.open_tree_context(Some(0), [0.0, 0.0]);
        assert!(app.context_key(&key(Key::Text("n".into()))));
        assert!(app.context.is_none());
        let prompt = app.prompt.as_ref().expect("a name is asked for");
        assert_eq!((prompt.title.as_str(), prompt.body.as_str()), ("New File", "Create in src"));
        // A path is not a name: the card stays.
        app.prompt.as_mut().unwrap().input.as_mut().unwrap().replace_text("../x");
        app.accept_prompt();
        assert!(app.prompt.is_some());
        app.prompt.as_mut().unwrap().input.as_mut().unwrap().replace_text("notes.md");
        app.accept_prompt();
        assert!(app.prompt.is_none());
        // Escape closes a card without doing anything.
        app.open_tree_context(Some(1), [0.0, 0.0]);
        app.context_key(&key(Key::Text("d".into())));
        assert_eq!(app.prompt.as_ref().unwrap().title, "Delete?");
        app.context_prompt_key(&key(Key::Escape));
        assert!(app.prompt.is_none());
    }

    #[test]
    fn a_chat_is_pinned_at_once_and_deleted_only_after_a_yes() {
        let mut app = App::new();
        app.load_fixture("s", "Plan the release", "[]");
        app.open_chat_context("s", [0.0, 0.0]);
        assert_eq!(app.context.as_ref().unwrap().items, [Command::Pin, Command::RenameChat, Command::DeleteChat]);
        app.context_key(&key(Key::Text("p".into())));
        assert!(app.threads[0].pinned);
        assert!(app.agent_sent.borrow().iter().any(|m| m.contains("SetPinned")));
        app.open_chat_context("s", [0.0, 0.0]);
        app.context_key(&key(Key::Text("d".into())));
        assert_eq!(app.threads.len(), 1, "nothing is deleted before the yes");
        app.accept_prompt();
        assert!(app.threads.is_empty());
        assert!(app.agent_sent.borrow().iter().any(|m| m.contains("DeleteThread")));
    }
}
