//! Centered pickers: the `Alt+P` command palette and the theme picker with its live preview card
//! (`reference/14`, `23`, `24`). One query field, a ranked list, keyboard and pointer selection.

mod backgrounds;
pub(crate) mod commands;
mod view;

pub use view::view;

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use crate::ui::theme::{PALETTES, palette};
use commands::{Action, COMMANDS, command_score, fuzzy_score};
use rune::{TextEdit, Tx};
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Commands,
    Themes,
    Backgrounds,
    /// `Alt+D`: `cd` into a workspace directory.
    Directories,
}

pub struct Picker {
    pub mode: Mode,
    pub query: TextEdit,
    pub selected: usize,
    /// The Backgrounds catalog, read when the picker opens.
    pub backgrounds: Vec<backgrounds::Background>,
    /// Directory hits for the `cd` query, newest search only.
    pub directories: Vec<String>,
    directory_request: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Command(Action),
    Close(crate::tabs::CloseCommand),
    Theme(&'static str),
    Background(usize),
    Directory(usize),
}

pub struct Row {
    pub icon: Option<&'static str>,
    pub title: String,
    pub shortcut: &'static str,
    pub target: Target,
}

/// `nvchad_one` is "One Dark"; everything else title-cases its id.
pub fn theme_display_name(name: &str) -> String {
    if name == "nvchad_one" { return "One Dark".into(); }
    name.split(['_', '-']).filter(|p| !p.is_empty()).map(|part| {
        let mut chars = part.chars();
        chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

impl Picker {
    fn new(mode: Mode) -> Picker {
        let mut query = TextEdit::new("");
        query.focused = true;
        let backgrounds = if mode == Mode::Backgrounds {
            let dir = crate::config::dir();
            backgrounds::catalog(&dir, crate::config::background_image().as_deref())
        } else { Vec::new() };
        if mode == Mode::Directories { query.replace_text("cd "); }
        Picker { mode, query, selected: 0, backgrounds, directories: Vec::new(), directory_request: 0 }
    }

    /// Ranked rows for the current query; ties keep catalog order.
    pub fn rows(&self) -> Vec<Row> {
        let query = self.query.text.trim();
        if self.mode == Mode::Commands {
            if let Some(command) = crate::tabs::CloseCommand::parse(query) {
                let title = if command.workspace { "Close Workspace" } else { "Close Tab" };
                return vec![Row { icon: Some(commands::Service::Neoism.icon()),
                    title: format!("neoism: {title}{}", if command.discard { " (discard changes)" } else { "" }),
                    shortcut: "", target: Target::Close(command) }];
            }
        }
        let mut scored: Vec<(i32, Row)> = match self.mode {
            Mode::Commands => COMMANDS.iter().filter_map(|c| {
                let score = command_score(query, c)?;
                Some((score, Row { icon: Some(c.service.icon()), title: format!("{}: {}", c.service.prefix(), c.title), shortcut: c.shortcut, target: Target::Command(c.action) }))
            }).collect(),
            Mode::Backgrounds => self.backgrounds.iter().enumerate().filter_map(|(i, b)| {
                let score = fuzzy_score(query, &b.name).or_else(|| fuzzy_score(query, &b.description).map(|s| s - 4))?;
                Some((score, Row { icon: None, title: b.name.clone(), shortcut: "", target: Target::Background(i) }))
            }).collect(),
            // The daemon ranks directories; rows keep its order.
            Mode::Directories => return self.directories.iter().enumerate().map(|(i, path)| {
                Row { icon: Some(crate::ui::icons::FOLDER_CLOSED), title: path.clone(), shortcut: "", target: Target::Directory(i) }
            }).collect(),
            Mode::Themes => PALETTES.iter().filter_map(|p| {
                let title = theme_display_name(p.name);
                let score = fuzzy_score(query, &title)?;
                Some((score, Row { icon: None, title, shortcut: "", target: Target::Theme(p.name) }))
            }).collect(),
        };
        if !query.is_empty() { scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score)); }
        scored.into_iter().map(|(_, row)| row).collect()
    }
}

impl App {
    pub fn open_picker(&mut self, mode: Mode) {
        let mut picker = Picker::new(mode);
        if mode == Mode::Themes {
            picker.selected = PALETTES.iter().position(|p| std::ptr::eq(p, self.pal)).unwrap_or(0);
        }
        self.picker = Some(picker);
        if mode == Mode::Directories { self.search_directories(); }
    }

    /// Search directories for the `cd …` query (`ChromeActionsController.searchDirectories`).
    pub fn search_directories(&mut self) {
        let Some(picker) = &mut self.picker else { return };
        if picker.mode != Mode::Directories { return; }
        picker.directory_request += 1;
        let query = picker.query.text.strip_prefix("cd").unwrap_or(&picker.query.text).trim().to_owned();
        let message = json!({"SearchDirectories": {"req_id": picker.directory_request, "query": query, "cwd": ""}});
        if let Some(daemon) = &self.daemon { daemon.send_scoped(Family::Search, message, &self.cwd); }
    }

    pub(crate) fn apply_directory_hits(&mut self, req_id: u64, hits: Vec<String>) {
        let Some(picker) = &mut self.picker else { return };
        if picker.mode != Mode::Directories || req_id != picker.directory_request { return; }
        picker.directories = hits;
        picker.selected = 0;
    }

    pub fn move_picker(&mut self, delta: isize) {
        let Some(picker) = &mut self.picker else { return };
        let count = picker.rows().len();
        if count == 0 { return; }
        picker.selected = (picker.selected as isize + delta).rem_euclid(count as isize) as usize;
        self.palette_follow.request();
    }

    /// Run the selected (or clicked) row and close the picker.
    pub fn accept_picker(&mut self, index: Option<usize>, tx: &mut Tx) {
        let Some(picker) = self.picker.take() else { return };
        let rows = picker.rows();
        let Some(row) = rows.get(index.unwrap_or(picker.selected)) else { return };
        match row.target {
            Target::Close(command) => self.run_close_command(command),
            Target::Command(action) => self.run_action(action, tx),
            Target::Theme(name) => self.apply_theme(name),
            Target::Directory(index) => {
                if let Some(dir) = picker.directories.get(index) {
                    let path = std::path::Path::new(&self.cwd).join(dir).display().to_string();
                    self.change_directory(&path);
                }
            }
            Target::Background(index) => {
                if let Some(background) = picker.backgrounds.get(index) { self.apply_background(&background.choice); }
            }
        }
    }

    pub(crate) fn run_action(&mut self, action: Action, tx: &mut Tx) {
        let count = self.tabs.len();
        match action {
            Action::NewWorkspace => self.new_workspace(),
            Action::NewTab => { self.open_tab(TabKind::Home); }
            Action::CloseTab => self.close_tab(self.active),
            Action::CloseWorkspace => self.close_workspace(self.workspace_active),
            Action::CloseOtherTabs => {
                let keep = self.tabs[self.active].id;
                for index in (0..count).rev() {
                    if self.tabs[index].id != keep { self.close_tab(index); if self.modal.is_some() { break; } }
                }
                if let Some(index) = self.tabs.iter().position(|t| t.id == keep) { self.select_tab(self.tabs[index].id); }
            }
            Action::NextTab => self.step_tab(true),
            Action::PreviousTab => self.step_tab(false),
            Action::SplitRight => self.split(crate::panes::Axis::Horizontal),
            Action::SplitDown => self.split(crate::panes::Axis::Vertical),
            Action::NextSplit => { self.focus_split(true); }
            Action::PreviousSplit => { self.focus_split(false); }
            Action::CloseSplitOrTab => self.close_tab(self.active),
            Action::IncreaseFontSize => self.set_font_size(self.ty.size + 1.0),
            Action::DecreaseFontSize => self.set_font_size(self.ty.size - 1.0),
            Action::ResetFontSize => self.set_font_size(crate::config::DEFAULT_FONT_SIZE),
            Action::ThemePicker => self.open_picker(Mode::Themes),
            Action::OpenSettings => self.open_settings(),
            Action::Backgrounds => self.open_picker(Mode::Backgrounds),
            Action::SearchFiles => self.open_finder(crate::finder::Mode::Files),
            Action::SearchWords => self.open_finder(crate::finder::Mode::Grep),
            Action::SearchGitChanges => self.open_finder(crate::finder::Mode::Git),
            Action::OpenAgent => { self.open_tab(TabKind::Agent { session: None }); }
            Action::ToggleConversations => self.toggle_conversations(),
            Action::Quit => tx.quit(),
        }
    }

    fn set_setting(&self, key: &str, value: serde_json::Value) {
        if let Some(daemon) = &self.daemon { daemon.send(Family::Config, json!({"SetSetting": {"key": key, "value": value}})); }
    }

    /// Font zoom shows at once; the config write follows (as the GPUiX host did).
    fn set_font_size(&mut self, size: f32) {
        self.ty.size = size.clamp(8.0, 40.0);
        self.set_setting("appearance.fonts.size", json!(self.ty.size));
    }

    /// Persist a background choice (`ui.window.background-image[-disabled]`).
    pub fn apply_background(&mut self, choice: &backgrounds::Choice) {
        let image = match choice {
            backgrounds::Choice::Image(path) => json!({"path": path, "opacity": 1.0, "blur": 0.0, "fit": "cover", "position-x": 0.5, "position-y": 0.5}),
            _ => serde_json::Value::Null,
        };
        self.set_setting("ui.window.background-image-disabled", json!(*choice == backgrounds::Choice::None));
        self.set_setting("ui.window.background-image", image.clone());
        // Show the choice now rather than after the daemon's write lands on disk.
        self.wallpaper = crate::ui::wallpaper::Wallpaper::from_config(&json!({"ui": {"window": {
            "background-image-disabled": *choice == backgrounds::Choice::None, "background-image": image }}}));
    }

    /// The preview image for a background path, once decoded. The first ask starts a decode on
    /// a worker thread; `take_decoded_previews` (from `poll`) turns the pixels into an image.
    pub fn background_preview(&self, path: &std::path::Path) -> Option<rune::Image> {
        if let Some(image) = self.background_previews.borrow().get(path) { return image.clone(); }
        self.background_previews.borrow_mut().insert(path.to_owned(), None);
        let (queue, wake, path) = (self.decoded_previews.clone(), self.waker.clone(), path.to_owned());
        std::thread::spawn(move || {
            if let Some(pixels) = backgrounds::decode(&path) { queue.lock().unwrap().push((path, pixels)); }
            if let Some(wake) = wake { wake(); }
        });
        None
    }

    /// Move decoded previews into the image cache. Returns whether any arrived.
    pub fn take_decoded_previews(&mut self) -> bool {
        let decoded: Vec<_> = self.decoded_previews.lock().unwrap().drain(..).collect();
        let Some(images) = &self.images else { return false };
        for (path, (w, h, bytes)) in &decoded {
            self.background_previews.borrow_mut().insert(path.clone(), Some(images.rgba(*w, *h, bytes.clone())));
        }
        !decoded.is_empty()
    }

    pub fn apply_theme(&mut self, name: &str) {
        self.pal = palette(name);
        let colors = self.terminal_colors();
        for term in self.terminals.values_mut() { term.set_colors(&colors); }
        if let Some(images) = &self.images { self.wordmark.load(images, self.pal.fg); }
        self.set_setting("appearance.theme", json!(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_close_aliases_never_choose_fuzzy_quit() {
        let mut picker = Picker::new(Mode::Commands);
        for (workspace, names) in [(false, &["q", "quit", "quite", "close"][..]), (true, &["qa", "qall", "quitall"][..])] {
            for name in names { for prefix in ["", ":"] { for suffix in ["", "!"] {
                picker.query.replace_text(&format!("{prefix}{name}{suffix}"));
                let rows = picker.rows();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].target, Target::Close(crate::tabs::CloseCommand { workspace, discard: suffix == "!" }));
                assert!(!rows[0].title.contains("Quit"));
            } } }
        }
        picker.query.replace_text("qalligator");
        assert!(picker.rows().iter().all(|r| !matches!(r.target, Target::Close(_))));
    }

    #[test]
    fn themes_keep_curated_order_then_filter() {
        let mut picker = Picker::new(Mode::Themes);
        let names: Vec<String> = picker.rows().iter().take(4).map(|r| r.title.clone()).collect();
        assert_eq!(names, ["Pastel Dark", "One Dark", "Tokyo Night", "Catppuccin Mocha"]);
        picker.query.replace_text("tokyo");
        assert_eq!(picker.rows()[0].title, "Tokyo Night");
    }

    #[test]
    fn commands_are_namespaced() {
        let picker = Picker::new(Mode::Commands);
        assert_eq!(picker.rows()[0].title, "neoism: New Tab");
    }
}
