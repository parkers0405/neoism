//! The Settings page (`reference/27-settings.png`; `shared/src/panels/settings_page`): rows built
//! from the daemon's config schema and values, grouped into the original categories, searchable,
//! edited in place and written back with `SetSetting`.

pub mod keybinds;
pub mod rebind;
pub mod view;

pub use view::view;

use crate::app::App;
use crate::daemon::Family;
use neoism_protocol::config::{ConfigCategory, ConfigConstraints, ConfigControl, ConfigDescriptor, ConfigOption, ConfigValueKind};
use rune::TextEdit;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    General,
    Appearance,
    Editor,
    Terminal,
    Ui,
    Presence,
    Keybinds,
    Agent,
    Platform,
    Developer,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::General, Category::Appearance, Category::Editor, Category::Terminal, Category::Ui,
        Category::Presence, Category::Keybinds, Category::Agent, Category::Platform, Category::Developer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Appearance => "Appearance",
            Category::Editor => "Editor",
            Category::Terminal => "Terminal",
            Category::Ui => "UI",
            Category::Presence => "Presence",
            Category::Keybinds => "Keybinds",
            Category::Agent => "Agent",
            Category::Platform => "Platform",
            Category::Developer => "Developer",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Category::General => "\u{f013}",
            Category::Appearance => "\u{f1fc}",
            Category::Editor => "\u{f044}",
            Category::Terminal => "\u{f120}",
            Category::Ui => "\u{f108}",
            Category::Presence => "\u{f0c0}",
            Category::Keybinds => "\u{f11c}",
            Category::Agent => "\u{f06a9}",
            Category::Platform => "\u{f17a}",
            Category::Developer => "\u{f188}",
        }
    }

    fn from_protocol(category: ConfigCategory) -> Category {
        match category {
            ConfigCategory::General => Category::General,
            ConfigCategory::Appearance => Category::Appearance,
            ConfigCategory::Editor => Category::Editor,
            ConfigCategory::Terminal => Category::Terminal,
            ConfigCategory::Ui => Category::Ui,
            ConfigCategory::Presence => Category::Presence,
            ConfigCategory::Keybinds => Category::Keybinds,
            ConfigCategory::Agent => Category::Agent,
            ConfigCategory::Platform => Category::Platform,
            ConfigCategory::Renderer | ConfigCategory::Developer => Category::Developer,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle,
    Select,
    Text,
    Keybinding,
    /// A button that opens another surface instead of editing inline.
    Action { action: &'static str, button: &'static str },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub category: Category,
    pub path: String,
    pub label: String,
    pub description: String,
    pub kind: ConfigValueKind,
    pub default: Value,
    pub options: Vec<ConfigOption>,
    pub constraints: ConfigConstraints,
    pub extensible: bool,
    pub control: Control,
}

impl Row {
    fn from_descriptor(d: ConfigDescriptor) -> Row {
        let mut suggestions = d.static_suggestions;
        suggestions.extend(d.runtime_suggestions);
        suggestions.sort_by_key(|s| s.to_lowercase());
        suggestions.dedup();
        let mut options = d.options;
        for suggestion in &suggestions {
            let value = option_value(suggestion, d.value_kind);
            if !options.iter().any(|o| o.value == value) {
                options.push(ConfigOption { value, label: None, description: None });
            }
        }
        let control = match d.path.as_str() {
            "ui.window.background-image" => Control::Action { action: "backgrounds:pick", button: "Choose\u{2026}" },
            "agent.model" => Control::Action { action: "open-model", button: "Choose\u{2026}" },
            _ => match d.control {
                ConfigControl::Toggle => Control::Toggle,
                ConfigControl::Select | ConfigControl::FontFamily => Control::Select,
                ConfigControl::Keybinding => Control::Keybinding,
                ConfigControl::Number if !options.is_empty() => Control::Select,
                ConfigControl::Text | ConfigControl::Number | ConfigControl::Color | ConfigControl::StringList | ConfigControl::Object => Control::Text,
            },
        };
        Row {
            category: Category::from_protocol(d.category), path: d.path, label: d.label, description: d.description,
            kind: d.value_kind, default: d.default, options, constraints: d.constraints, extensible: d.extensible, control,
        }
    }

    /// The description plus the allowed range and unit (`min-max unit`).
    pub fn detail(&self) -> String {
        let mut detail = self.description.clone();
        let range = match (self.constraints.min, self.constraints.max) {
            (Some(min), Some(max)) => format!("{min}-{max}"),
            (Some(min), None) => format!(">= {min}"),
            (None, Some(max)) => format!("<= {max}"),
            (None, None) => return detail,
        };
        if !detail.is_empty() { detail.push_str("  |  "); }
        detail.push_str(&range);
        if let Some(unit) = &self.constraints.unit { detail.push(' '); detail.push_str(unit); }
        detail
    }
}

/// Text typed into a field, as the setting's JSON kind (`parse_edited_value`).
pub fn parse_value(text: &str, kind: ConfigValueKind) -> Option<Value> {
    match kind {
        ConfigValueKind::String => Some(Value::String(text.to_owned())),
        ConfigValueKind::Integer => text.trim().parse::<i64>().ok().map(Value::from),
        ConfigValueKind::Number => text.trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number),
        ConfigValueKind::Boolean => text.trim().parse::<bool>().ok().map(Value::Bool),
        ConfigValueKind::Array | ConfigValueKind::Object => serde_json::from_str(text).ok(),
    }
}

fn option_value(text: &str, kind: ConfigValueKind) -> Value {
    parse_value(text, kind).unwrap_or_else(|| Value::String(text.to_owned()))
}

/// How a value shows in a field or dropdown.
pub fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

pub fn option_label(option: &ConfigOption) -> String {
    option.label.clone().unwrap_or_else(|| display(&option.value))
}

/// The open page's state.
pub struct Settings {
    pub rows: Vec<Row>,
    pub values: Value,
    pub category: Category,
    pub search: TextEdit,
    /// The keyboard row among the visible ones.
    pub focused: usize,
    /// The row whose dropdown is open, and its highlighted option.
    pub dropdown: Option<(usize, usize)>,
    /// The row being edited as text.
    pub editing: Option<(usize, TextEdit)>,
    pub loading: bool,
    pub rebinding: Option<usize>,
    pub binding_error: Option<String>,
    /// The page's scroll (offset, max, viewport), and a keyboard move to reveal.
    pub scroll: std::rc::Rc<std::cell::Cell<(f32, f32, f32)>>,
    pub reveal: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        let mut search = TextEdit::new("");
        search.focused = true;
        Settings { rows: Vec::new(), values: Value::Null, category: Category::General, search, focused: 0, dropdown: None, editing: None, loading: true, rebinding: None, binding_error: None, scroll: Default::default(), reveal: false }
    }
}

impl Settings {
    /// Visible schema rows: settings-visible, no wildcard paths, ordered by category then label.
    pub fn set_descriptors(&mut self, descriptors: Vec<ConfigDescriptor>) {
        self.rows = descriptors.into_iter()
            .filter(|d| d.settings_visible && !d.path.split('.').any(|seg| seg == "*"))
            .map(Row::from_descriptor)
            .collect();
        let rank = |c: Category| Category::ALL.iter().position(|x| *x == c);
        self.rows.sort_by(|a, b| rank(a.category).cmp(&rank(b.category))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path)));
        self.loading = false;
        if !self.categories().contains(&self.category) { self.category = Category::General; }
    }

    /// The sidebar: every category, Platform only when some row lives there.
    pub fn categories(&self) -> Vec<Category> {
        Category::ALL.into_iter().filter(|c| *c != Category::Platform || self.rows.iter().any(|r| r.category == *c)).collect()
    }

    pub fn searching(&self) -> bool { !self.search.text.trim().is_empty() }

    /// Row indices shown now: the category's rows, or every match while searching.
    pub fn visible(&self) -> Vec<usize> {
        let query = self.search.text.trim().to_lowercase();
        self.rows.iter().enumerate().filter(|(_, r)| {
            let matches = query.is_empty() || r.label.to_lowercase().contains(&query) || r.path.to_lowercase().contains(&query) || r.description.to_lowercase().contains(&query);
            matches && (!query.is_empty() || r.category == self.category)
        }).map(|(i, _)| i).collect()
    }

    pub fn value(&self, path: &str) -> Option<&Value> {
        path.split('.').try_fold(&self.values, |v, seg| v.get(seg))
    }

    pub fn bool_at(&self, row: &Row) -> bool {
        self.value(&row.path).and_then(Value::as_bool).unwrap_or_else(|| row.default.as_bool().unwrap_or(false))
    }

    pub fn string_at(&self, row: &Row) -> String {
        self.value(&row.path).map(display).unwrap_or_else(|| if row.default.is_null() { String::new() } else { display(&row.default) })
    }

    /// Dropdown choices: the row's options (and every theme for `appearance.theme`), sorted.
    pub fn options(&self, row: &Row) -> Vec<(String, Value)> {
        let mut options: Vec<(String, Value)> = row.options.iter().map(|o| (option_label(o), o.value.clone())).collect();
        if row.path == "appearance.theme" {
            for palette in crate::ui::theme::PALETTES {
                if !options.iter().any(|(_, v)| v.as_str() == Some(palette.name)) {
                    options.push((palette.name.to_owned(), json!(palette.name)));
                }
            }
        }
        options.sort_by_key(|(label, _)| label.to_lowercase());
        options.dedup_by(|a, b| a.0 == b.0);
        options
    }

    /// Mirror a write locally so the page shows it before the daemon confirms.
    pub fn set_local(&mut self, path: &str, value: Value) {
        if !self.values.is_object() { self.values = json!({}); }
        let segments: Vec<&str> = path.split('.').collect();
        let Some((leaf, parents)) = segments.split_last() else { return };
        let mut node = &mut self.values;
        for seg in parents {
            if !node.get(*seg).is_some_and(Value::is_object) { node[*seg] = json!({}); }
            node = &mut node[*seg];
        }
        node[*leaf] = value;
    }

    /// The live combo for a keybind row: config override, else this platform's default.
    pub fn keybind(&self, def: &keybinds::KeybindDef) -> String {
        if !def.literal.is_empty() { return def.literal.to_owned(); }
        if !def.action.is_empty() {
            let overridden = self.value("keybinds.keys").and_then(Value::as_array).and_then(|keys| keys.iter()
                .find(|k| k["action"].as_str() == Some(def.action))
                .map(|k| (k["with"].as_str().unwrap_or_default().to_owned(), k["key"].as_str().unwrap_or_default().to_owned())));
            if let Some((with, key)) = overridden { return keybinds::format_combo(&with, &key); }
        }
        let (mods, key) = if cfg!(target_os = "macos") { (def.mac_mods, def.mac_key) } else { (def.other_mods, def.other_key) };
        if mods.is_empty() && key.is_empty() { "Not bound".into() } else { keybinds::format_combo(mods, key) }
    }
}

impl App {
    /// Open the page and ask the daemon for values and schema.
    pub fn open_settings(&mut self) {
        self.menu = None;
        self.picker = None;
        self.settings = Some(Settings::default());
        if let Some(daemon) = &self.daemon {
            daemon.send(Family::Config, json!("GetConfig"));
            daemon.send(Family::Config, json!("GetConfigSchema"));
        }
    }

    /// The header's document button: close the page and edit `config.json` in an editor tab,
    /// as the original does through the daemon's config document (saved with its revision).
    pub fn open_config_document(&mut self) {
        self.settings = None;
        self.open_config_editor();
    }

    pub(crate) fn apply_config_reply(&mut self, request_id: u64, message: &Value) {
        self.apply_config_document_reply(request_id, message);
        if let Some(config) = message.get("Config") { self.keybindings = config["value"]["keybinds"]["keys"].clone(); }
        let Some(settings) = &mut self.settings else { return };
        if let Some(config) = message.get("Config") {
            settings.values = config["value"].clone();
        } else if let Some(schema) = message.get("ConfigSchema") {
            if let Ok(descriptors) = serde_json::from_value(schema["descriptors"].clone()) { settings.set_descriptors(descriptors); }
        }
    }

    /// Write one setting, mirror it locally, and apply what the app shows live.
    pub fn write_setting(&mut self, path: &str, value: Value) {
        if path == "keybinds.keys" { self.keybindings = value.clone(); }
        if let Some(settings) = &mut self.settings { settings.set_local(path, value.clone()); }
        match path {
            "appearance.theme" => if let Some(name) = value.as_str() { self.apply_theme(name); return },
            "appearance.fonts.size" => if let Some(size) = value.as_f64() { self.ty.size = (size as f32).clamp(8.0, 40.0); },
            "ui.window.background-image" | "ui.window.background-image-disabled" => {}
            _ => {}
        }
        if let Some(daemon) = &self.daemon {
            daemon.send(Family::Config, json!({"SetSetting": {"key": path, "value": value}}));
        }
        if path.starts_with("ui.window.background-image") { self.reload_wallpaper_from(self.settings.as_ref().map(|s| s.values.clone())); }
    }

    pub fn toggle_setting(&mut self, index: usize) {
        let Some(settings) = &self.settings else { return };
        let Some(row) = settings.rows.get(index).cloned() else { return };
        match &row.control {
            Control::Toggle => { let next = !settings.bool_at(&row); self.write_setting(&row.path, json!(next)); }
            Control::Select => {
                let current = settings.string_at(&row);
                let highlight = settings.options(&row).iter().position(|(label, _)| *label == current).unwrap_or(0);
                if let Some(s) = &mut self.settings { s.dropdown = if s.dropdown.is_some_and(|(r, _)| r == index) { None } else { Some((index, highlight)) }; }
            }
            Control::Text => {
                let mut edit = TextEdit::new(&settings.string_at(&row));
                edit.focused = true;
                if let Some(s) = &mut self.settings { s.editing = Some((index, edit)); }
            }
            Control::Action { action, .. } => match *action {
                "backgrounds:pick" => { self.settings = None; self.open_picker(crate::palette::Mode::Backgrounds); }
                _ => {}
            },
            Control::Keybinding => {}
        }
    }

    pub fn choose_option(&mut self, index: usize, option: usize) {
        let Some(settings) = &mut self.settings else { return };
        settings.dropdown = None;
        let Some(row) = settings.rows.get(index).cloned() else { return };
        let Some((_, value)) = settings.options(&row).into_iter().nth(option) else { return };
        self.write_setting(&row.path, value);
    }

    pub fn commit_setting_edit(&mut self) {
        let Some(settings) = &mut self.settings else { return };
        let Some((index, edit)) = settings.editing.take() else { return };
        let Some(row) = settings.rows.get(index).cloned() else { return };
        let text = edit.text.trim().to_owned();
        let value = if text.is_empty() && row.constraints.nullable { Some(Value::Null) } else { parse_value(&text, row.kind) };
        if let Some(value) = value { self.write_setting(&row.path, value); }
    }

    /// Keys while the page is open. Returns whether the page took the key.
    /// After a keyboard move, glide the page so the focused row's control is in view (a row's
    /// height of room above and below), on the shared scroll engine.
    pub fn settings_reveal(&mut self, tx: &mut rune::Tx) {
        let Some(settings) = self.settings.as_mut().filter(|s| s.reveal) else { return };
        settings.reveal = false;
        let Some(&index) = settings.visible().get(settings.focused) else { return };
        let key = view::scroll_key(settings);
        let (Some(control), Some(page)) = (self.rects.get(&format!("settings-control:{index}")).copied(), self.rects.get(&key).copied()) else { return };
        let (offset, max, _) = settings.scroll.get();
        let room = 60.0 * self.ty.scale();
        let top = control[1] - page[1];
        let bottom = top + control[3];
        let target = if top < room { offset + top - room } else if bottom > page[3] - room { offset + bottom - (page[3] - room) } else { return };
        let target = target.clamp(0.0, max.max(0.0));
        let mirror = settings.scroll.clone();
        let source: crate::ui::scroll::Source = std::rc::Rc::new(move || { let (o, m, _) = mirror.get(); (o, m) });
        self.scrollers.glide_to(&key, offset, target, page[3] * 1.25, source, tx);
    }

    pub fn settings_key(&mut self, event: &rune::KeyEvent) -> bool {
        use rune::Key;
        let Some(settings) = &mut self.settings else { return false };
        if let Some((index, option)) = settings.dropdown {
            let count = settings.rows.get(index).map(|r| settings.options(r).len()).unwrap_or(0).max(1);
            match event.key {
                Key::Up => settings.dropdown = Some((index, (option + count - 1) % count)),
                Key::Down => settings.dropdown = Some((index, (option + 1) % count)),
                Key::Enter => self.choose_option(index, option),
                Key::Escape => settings.dropdown = None,
                _ => return false,
            }
            return true;
        }
        if settings.editing.is_some() {
            match event.key {
                Key::Enter => self.commit_setting_edit(),
                Key::Escape => settings.editing = None,
                _ => return false,
            }
            return true;
        }
        let visible = settings.visible();
        let categories = settings.categories();
        let at = categories.iter().position(|c| *c == settings.category).unwrap_or(0);
        match event.key {
            Key::Up => { settings.focused = settings.focused.saturating_sub(1); settings.reveal = true; }
            Key::Down => { settings.focused = (settings.focused + 1).min(visible.len().saturating_sub(1)); settings.reveal = true; }
            Key::Left if !settings.searching() => { settings.category = categories[(at + categories.len() - 1) % categories.len()]; settings.focused = 0; }
            Key::Right if !settings.searching() => { settings.category = categories[(at + 1) % categories.len()]; settings.focused = 0; }
            Key::Enter => if let Some(&index) = visible.get(settings.focused) { self.toggle_setting(index) },
            Key::Escape => if settings.searching() { settings.search.replace_text(""); settings.focused = 0; } else { self.settings = None },
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(path: &str, category: ConfigCategory, control: ConfigControl, kind: ConfigValueKind) -> ConfigDescriptor {
        serde_json::from_value(json!({
            "path": path, "label": path.rsplit('.').next().unwrap(), "description": "", "value_kind": kind,
            "default": null, "extensible": false, "category": category, "control": control,
        })).unwrap()
    }

    #[test]
    fn rows_sort_by_category_then_label_and_drop_wildcards() {
        let mut s = Settings::default();
        s.set_descriptors(vec![
            descriptor("editor.zeta", ConfigCategory::Editor, ConfigControl::Toggle, ConfigValueKind::Boolean),
            descriptor("general.alpha", ConfigCategory::General, ConfigControl::Toggle, ConfigValueKind::Boolean),
            descriptor("render.x", ConfigCategory::Renderer, ConfigControl::Text, ConfigValueKind::String),
            descriptor("mcp.*.url", ConfigCategory::Agent, ConfigControl::Text, ConfigValueKind::String),
        ]);
        let paths: Vec<&str> = s.rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["general.alpha", "editor.zeta", "render.x"]);
        assert_eq!(s.rows[2].category, Category::Developer, "renderer rows live under Developer");
        assert!(!s.categories().contains(&Category::Platform), "Platform hides without rows");
    }

    #[test]
    fn search_spans_categories() {
        let mut s = Settings::default();
        s.set_descriptors(vec![
            descriptor("general.copy", ConfigCategory::General, ConfigControl::Toggle, ConfigValueKind::Boolean),
            descriptor("editor.copy_on_select", ConfigCategory::Editor, ConfigControl::Toggle, ConfigValueKind::Boolean),
        ]);
        assert_eq!(s.visible(), vec![0]);
        s.search.replace_text("copy");
        assert_eq!(s.visible(), vec![0, 1]);
    }

    #[test]
    fn values_coerce_to_their_kind() {
        assert_eq!(parse_value(" 14 ", ConfigValueKind::Integer), Some(json!(14)));
        assert_eq!(parse_value("1.5", ConfigValueKind::Number), Some(json!(1.5)));
        assert_eq!(parse_value("nope", ConfigValueKind::Integer), None);
        assert_eq!(parse_value("[1,2]", ConfigValueKind::Array), Some(json!([1, 2])));
        let mut s = Settings::default();
        s.set_local("appearance.fonts.size", json!(16));
        assert_eq!(s.value("appearance.fonts.size"), Some(&json!(16)));
    }

    #[test]
    fn number_with_options_is_a_select() {
        let mut d = descriptor("ui.fps", ConfigCategory::Ui, ConfigControl::Number, ConfigValueKind::Integer);
        d.static_suggestions = vec!["60".into(), "120".into()];
        assert_eq!(Row::from_descriptor(d).control, Control::Select);
    }
}
