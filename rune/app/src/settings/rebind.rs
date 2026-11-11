//! Shortcut capture and dispatch share one catalogue, so saved bindings take effect immediately.
use super::keybinds::{KEYBINDS, KeybindDef};
use crate::{app::App, palette::commands::Action, tabs::TabKind};
use rune::{Key, KeyEvent, Tx};
use serde_json::{Value, json};

fn action(name: &str) -> Option<Action> {
    Some(match name {
        "createtab" => Action::NewWorkspace,
        "closesplitortab" => Action::CloseTab,
        "selectnexttab" | "selectnextbuffertab" => Action::NextTab,
        "selectprevtab" | "selectprevbuffertab" => Action::PreviousTab,
        "splitright" => Action::SplitRight,
        "splitdown" => Action::SplitDown,
        "selectnextsplit" => Action::NextSplit,
        "selectprevsplit" => Action::PreviousSplit,
        "increasefontsize" => Action::IncreaseFontSize,
        "decreasefontsize" => Action::DecreaseFontSize,
        "resetfontsize" => Action::ResetFontSize,
        "quit" => Action::Quit,
        "togglegitdiffpanel" => Action::ToggleGitPanel,
        _ => return None,
    })
}
pub fn supported(name: &str) -> bool {
    action(name).is_some()
        || matches!(
            name,
            "opencommandpalette"
                | "toggleneoismnotes"
                | "openconfigeditor"
                | "createworkspaceterminaltab"
                | "paste"
                | "copy"
                | "createwindow"
                | "togglefullscreen"
        )
}
fn default(def: &KeybindDef) -> (&str, &str) {
    if cfg!(target_os = "macos") {
        (def.mac_mods, def.mac_key)
    } else {
        (def.other_mods, def.other_key)
    }
}
fn binding<'a>(values: &'a Value, def: &'a KeybindDef) -> (&'a str, &'a str) {
    values
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|v| v["action"].as_str() == Some(def.action))
        })
        .map(|v| {
            (
                v["with"].as_str().unwrap_or(""),
                v["key"].as_str().unwrap_or(""),
            )
        })
        .unwrap_or_else(|| default(def))
}
fn chord(event: &KeyEvent) -> Option<(String, String)> {
    let key = match &event.key {
        Key::Command(c) => c.to_lowercase().to_string(),
        Key::Text(t) if t == " " => "space".into(),
        Key::Text(t) => t.to_lowercase(),
        Key::Tab => "tab".into(),
        Key::Enter => "enter".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::PageUp => "pageup".into(),
        Key::PageDown => "pagedown".into(),
        Key::Backspace => "backspace".into(),
        Key::Delete => "delete".into(),
        _ => return None,
    };
    let m = event.mods;
    let mods = [
        (m.ctrl, "control"),
        (m.alt, "alt"),
        (m.shift, "shift"),
        (m.logo, "super"),
    ]
    .into_iter()
    .filter_map(|(on, s)| on.then_some(s))
    .collect::<Vec<_>>()
    .join(" | ");
    Some((mods, key))
}
fn same(a: (&str, &str), b: (&str, &str)) -> bool {
    let mods = |s: &str| {
        let mut v: Vec<_> = s
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect();
        v.sort();
        v
    };
    !a.1.is_empty() && a.1.eq_ignore_ascii_case(b.1) && mods(a.0) == mods(b.0)
}
impl App {
    pub fn start_rebind(&mut self, index: usize) {
        if !KEYBINDS.get(index).is_some_and(|d| supported(d.action)) {
            return;
        }
        if let Some(s) = &mut self.settings {
            s.rebinding = Some(index);
            s.binding_error = None;
            s.search.focused = false;
        }
    }
    pub fn reset_binding(&mut self, index: usize) {
        let Some(def) = KEYBINDS.get(index) else {
            return;
        };
        let mut list = self.keybindings.as_array().cloned().unwrap_or_default();
        list.retain(|v| v["action"].as_str() != Some(def.action));
        self.write_setting("keybinds.keys", json!(list));
    }
    pub fn capture_binding(&mut self, event: &KeyEvent) -> bool {
        let Some(index) = self.settings.as_ref().and_then(|s| s.rebinding) else {
            return false;
        };
        if event.key == Key::Escape {
            if let Some(s) = &mut self.settings {
                s.rebinding = None;
                s.binding_error = None;
            }
            return true;
        }
        let Some((mods, key)) = chord(event) else {
            return true;
        };
        let def = &KEYBINDS[index];
        let conflict = KEYBINDS.iter().find(|other| {
            other.action != def.action
                && (supported(other.action) || (other.action.is_empty() && other.literal.is_empty()))
                && same(binding(&self.keybindings, other), (&mods, &key))
        });
        let error = if mods.is_empty() {
            Some("Use at least one modifier to preserve text and Vim input".to_owned())
        } else {
            conflict.map(|other| format!("Already used by {}", other.label))
        };
        if let Some(error) = error {
            if let Some(s) = &mut self.settings {
                s.binding_error = Some(error);
            }
            return true;
        }
        let mut list = self.keybindings.as_array().cloned().unwrap_or_default();
        list.retain(|v| v["action"].as_str() != Some(def.action));
        list.push(json!({"action":def.action, "with":mods, "key":key}));
        self.write_setting("keybinds.keys", json!(list));
        if let Some(s) = &mut self.settings {
            s.rebinding = None;
            s.binding_error = None;
        }
        true
    }
    pub fn bound_shortcut(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some((mods, key)) = chord(event) else {
            return false;
        };
        let chosen = KEYBINDS
            .iter()
            .find(|d| {
                supported(d.action)
                    && (!matches!(d.action, "selectnextbuffertab" | "selectprevbuffertab") || matches!(self.active_tab().kind, TabKind::Editor { .. }))
                    && same(binding(&self.keybindings, d), (&mods, &key))
            })
            .map(|d| d.action);
        if let Some(name) = chosen {
            if let Some(action) = action(name) {
                self.run_action(action, tx);
            } else {
                match name {
                    "opencommandpalette" => {
                        self.open_picker(crate::palette::Mode::Commands)
                    }
                    "toggleneoismnotes" => self.toggle_tree(crate::app::Sidebar::Notes),
                    "openconfigeditor" => self.open_config_editor(),
                    "createworkspaceterminaltab" => {
                        self.open_tab(TabKind::Terminal);
                    }
                    "togglefullscreen" => tx.toggle_full_screen(),
                    "createwindow" => {
                        match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).current_dir(&self.cwd)
                            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()) {
                            Ok(mut child) => { std::thread::spawn(move || { let _ = child.wait(); }); },
                            Err(error) => { self.modal = Some(crate::ui::modal::Modal { title: "Could not open window".into(), body: error.to_string(), buttons: vec![], selected: 0 }); }
                        }
                    }
                    "copy" => self.copy_terminal_selection(tx),
                    "paste" => {
                        tx.request_paste();
                    }
                    _ => {}
                }
            }
            return true;
        }
        // An overridden default must not fall through to the old hardcoded shortcut.
        KEYBINDS.iter().any(|d| {
            supported(d.action)
                && same(default(d), (&mods, &key))
                && !same(binding(&self.keybindings, d), default(d))
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifier_order_and_overrides() {
        assert!(same(("control | shift", "T"), ("shift|control", "t")));
        assert!(!same(("control", "t"), ("shift|control", "t")));
        let def = KEYBINDS
            .iter()
            .find(|d| d.action == "opencommandpalette")
            .unwrap();
        let values = json!([{"action":def.action,"with":"control","key":"p"}]);
        assert_eq!(binding(&values, def), ("control", "p"));
    }
}
