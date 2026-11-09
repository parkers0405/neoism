//! Centered pickers: the `Alt+P` command palette and the theme picker with its live preview card
//! (`reference/14`, `23`, `24`). One query field, a ranked list, keyboard and pointer selection.

mod commands;
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
}

pub struct Picker {
    pub mode: Mode,
    pub query: TextEdit,
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Command(Action),
    Theme(&'static str),
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
        Picker { mode, query, selected: 0 }
    }

    /// Ranked rows for the current query; ties keep catalog order.
    pub fn rows(&self) -> Vec<Row> {
        let query = self.query.text.trim();
        let mut scored: Vec<(i32, Row)> = match self.mode {
            Mode::Commands => COMMANDS.iter().filter_map(|c| {
                let score = command_score(query, c)?;
                Some((score, Row { icon: Some(c.service.icon()), title: format!("{}: {}", c.service.prefix(), c.title), shortcut: c.shortcut, target: Target::Command(c.action) }))
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
    }

    pub fn move_picker(&mut self, delta: isize) {
        let Some(picker) = &mut self.picker else { return };
        let count = picker.rows().len();
        if count == 0 { return; }
        picker.selected = (picker.selected as isize + delta).rem_euclid(count as isize) as usize;
    }

    /// Run the selected (or clicked) row and close the picker.
    pub fn accept_picker(&mut self, index: Option<usize>, tx: &mut Tx) {
        let Some(picker) = self.picker.take() else { return };
        let rows = picker.rows();
        let Some(row) = rows.get(index.unwrap_or(picker.selected)) else { return };
        match row.target {
            Target::Command(action) => self.run_action(action, tx),
            Target::Theme(name) => self.apply_theme(name),
        }
    }

    fn run_action(&mut self, action: Action, tx: &mut Tx) {
        let count = self.tabs.len();
        match action {
            Action::NewTab => { self.open_tab(TabKind::Home); }
            Action::CloseTab => self.close_tab(self.active),
            Action::CloseOtherTabs => {
                let keep = self.tabs[self.active].id;
                for index in (0..count).rev() {
                    if self.tabs[index].id != keep { self.close_tab(index); }
                }
                self.active = 0;
            }
            Action::NextTab => self.active = (self.active + 1) % count,
            Action::PreviousTab => self.active = (self.active + count - 1) % count,
            Action::IncreaseFontSize => self.set_font_size(self.ty.size + 1.0),
            Action::DecreaseFontSize => self.set_font_size(self.ty.size - 1.0),
            Action::ResetFontSize => self.set_font_size(crate::config::DEFAULT_FONT_SIZE),
            Action::ThemePicker => self.open_picker(Mode::Themes),
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
