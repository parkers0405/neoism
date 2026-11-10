//! The Keybinds section's catalogue (`settings_page/state.rs`): rebindable rows backed by config
//! actions, and reference rows documenting fixed / vim-engine keys. Data copied verbatim.

#![allow(dead_code)]

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyGroup {
    Global,
    Terminal,
    Editor,
    Agent,
}

impl KeyGroup {
    pub fn title(self) -> &'static str {
        // Short + uppercase: these render in the bundled Press Start 2P
        // pixel face (same as the notes / agent section headers).
        match self {
            KeyGroup::Global => "GLOBAL",
            KeyGroup::Terminal => "TERMINAL",
            KeyGroup::Editor => "EDITOR",
            KeyGroup::Agent => "AGENT",
        }
    }
}

/// A shortcut shown in the Keybinds section. Three flavors:
/// - **rebindable** (`action` set): a config action; the live binding is
///   the `bindings.keys` override, else the platform default below.
///   macOS (⌘) and Linux/Windows (Ctrl/Alt) defaults differ.
/// - **global reference** (`action` empty, `literal` empty): a hardcoded
///   app shortcut (Alt+S, Alt+E…) shown platform-aware but not rebindable.
/// - **literal reference** (`literal` set): a vim-engine key like `dd`,
///   shown verbatim, not rebindable.
#[derive(Clone, Copy)]
pub struct KeybindDef {
    pub action: &'static str,
    pub label: &'static str,
    pub group: KeyGroup,
    pub mac_mods: &'static str,
    pub mac_key: &'static str,
    pub other_mods: &'static str,
    pub other_key: &'static str,
    pub literal: &'static str,
}

/// Rebindable config action.
const fn kb(
    action: &'static str,
    label: &'static str,
    group: KeyGroup,
    mm: &'static str,
    mk: &'static str,
    om: &'static str,
    ok: &'static str,
) -> KeybindDef {
    KeybindDef {
        action,
        label,
        group,
        mac_mods: mm,
        mac_key: mk,
        other_mods: om,
        other_key: ok,
        literal: "",
    }
}

/// Global reference shortcut (hardcoded, platform-aware, not rebindable).
const fn kbg(
    label: &'static str,
    group: KeyGroup,
    mm: &'static str,
    mk: &'static str,
    om: &'static str,
    ok: &'static str,
) -> KeybindDef {
    KeybindDef {
        action: "",
        label,
        group,
        mac_mods: mm,
        mac_key: mk,
        other_mods: om,
        other_key: ok,
        literal: "",
    }
}

/// Literal reference shortcut (vim-engine key, shown verbatim).
const fn kbl(label: &'static str, group: KeyGroup, literal: &'static str) -> KeybindDef {
    KeybindDef {
        action: "",
        label,
        group,
        mac_mods: "",
        mac_key: "",
        other_mods: "",
        other_key: "",
        literal,
    }
}

/// The Keybinds section, grouped by surface. Rebindable rows use real
/// config actions (from `bindings/platform/*` + `bindings/defaults.rs`);
/// reference rows document hardcoded / vim-engine keys you can't rebind.
pub const KEYBINDS: &[KeybindDef] = &[
    // ── Global ──
    kb(
        "opencommandpalette",
        "Command palette",
        KeyGroup::Global,
        "super",
        "p",
        "alt",
        "p",
    ),
    kbg("Search files", KeyGroup::Global, "alt", "s", "alt", "s"),
    kbg("Open agent", KeyGroup::Global, "alt", "a", "alt", "a"),
    kbg("Toggle file tree", KeyGroup::Global, "alt", "e", "alt", "e"),
    kb(
        "toggleneoismnotes",
        "Toggle notes panel",
        KeyGroup::Global,
        "alt",
        "n",
        "alt",
        "n",
    ),
    kb(
        "togglegitdiffpanel",
        "Toggle Git panel",
        KeyGroup::Global,
        "alt",
        "g",
        "alt",
        "g",
    ),
    kb(
        "createtab",
        "New workspace",
        KeyGroup::Global,
        "super",
        "t",
        "control | shift",
        "w",
    ),
    kb(
        "createworkspaceterminaltab",
        "New terminal tab",
        KeyGroup::Global,
        "",
        "",
        "control | shift",
        "t",
    ),
    kb(
        "createwindow",
        "New window",
        KeyGroup::Global,
        "super",
        "n",
        "control | shift",
        "n",
    ),
    kb(
        "closesplitortab",
        "Close tab / split",
        KeyGroup::Global,
        "super",
        "w",
        "",
        "",
    ),
    kb(
        "selectnexttab",
        "Next tab",
        KeyGroup::Global,
        "control",
        "tab",
        "control",
        "tab",
    ),
    kb(
        "selectprevtab",
        "Previous tab",
        KeyGroup::Global,
        "control | shift",
        "tab",
        "control | shift",
        "tab",
    ),
    kb(
        "selectnextbuffertab",
        "Next editor tab",
        KeyGroup::Global,
        "",
        "",
        "control | shift",
        "]",
    ),
    kb(
        "selectprevbuffertab",
        "Previous editor tab",
        KeyGroup::Global,
        "",
        "",
        "control | shift",
        "[",
    ),
    kb(
        "splitright",
        "Split right",
        KeyGroup::Global,
        "super",
        "d",
        "control | shift",
        "r",
    ),
    kb(
        "splitdown",
        "Split down",
        KeyGroup::Global,
        "super | shift",
        "d",
        "control | shift",
        "d",
    ),
    kb(
        "selectnextsplit",
        "Next split",
        KeyGroup::Global,
        "super",
        "]",
        "control | shift",
        "]",
    ),
    kb(
        "selectprevsplit",
        "Previous split",
        KeyGroup::Global,
        "super",
        "[",
        "control | shift",
        "[",
    ),
    kb(
        "increasefontsize",
        "Increase font size",
        KeyGroup::Global,
        "super",
        "=",
        "control",
        "=",
    ),
    kb(
        "decreasefontsize",
        "Decrease font size",
        KeyGroup::Global,
        "super",
        "-",
        "control",
        "-",
    ),
    kb(
        "resetfontsize",
        "Reset font size",
        KeyGroup::Global,
        "super",
        "0",
        "control",
        "0",
    ),
    kb(
        "openconfigeditor",
        "Open config file",
        KeyGroup::Global,
        "alt",
        ",",
        "alt",
        ",",
    ),
    kb(
        "togglefullscreen",
        "Toggle fullscreen",
        KeyGroup::Global,
        "control | super",
        "f",
        "",
        "",
    ),
    kb("quit", "Quit", KeyGroup::Global, "super", "q", "", ""),
    // ── Terminal ──
    kb(
        "copy",
        "Copy",
        KeyGroup::Terminal,
        "super",
        "c",
        "control | shift",
        "c",
    ),
    kb(
        "paste",
        "Paste",
        KeyGroup::Terminal,
        "super",
        "v",
        "control | shift",
        "v",
    ),
    kb(
        "searchforward",
        "Find in terminal",
        KeyGroup::Terminal,
        "super",
        "f",
        "control | shift",
        "f",
    ),
    kb(
        "togglevimode",
        "Toggle terminal vi-mode",
        KeyGroup::Terminal,
        "alt | shift",
        "space",
        "alt | shift",
        "space",
    ),
    kbl("Move (vi motions)", KeyGroup::Terminal, "h j k l"),
    kbl("Yank / paste", KeyGroup::Terminal, "y / p"),
    kbl("Visual / line select", KeyGroup::Terminal, "v / V"),
    kbl("Top / bottom", KeyGroup::Terminal, "gg / G"),
    kbl("Search in scrollback", KeyGroup::Terminal, "/"),
    // ── Editor (Vim) ── (reference — driven by the editor's vim engine)
    kbl("Insert before / at line start", KeyGroup::Editor, "i / I"),
    kbl("Append after / at line end", KeyGroup::Editor, "a / A"),
    kbl("Open line below / above", KeyGroup::Editor, "o / O"),
    kbl("Back to Normal mode", KeyGroup::Editor, "Esc"),
    kbl("Paste after / before", KeyGroup::Editor, "p / P"),
    kbl("Yank line", KeyGroup::Editor, "yy"),
    kbl("Delete line", KeyGroup::Editor, "dd"),
    kbl("Delete char", KeyGroup::Editor, "x"),
    kbl("Change word", KeyGroup::Editor, "cw"),
    kbl("Undo / redo", KeyGroup::Editor, "u / Ctrl+r"),
    kbl("Visual / visual line", KeyGroup::Editor, "v / V"),
    kbl("Top / bottom of file", KeyGroup::Editor, "gg / G"),
    kbl("Word forward / back", KeyGroup::Editor, "w / b"),
    kbl("Line start / end", KeyGroup::Editor, "0 / $"),
    kbl("Search", KeyGroup::Editor, "/"),
    // ── Agent ── (reference — composer input)
    kbl("Send message", KeyGroup::Agent, "Enter"),
    kbl("New line in message", KeyGroup::Agent, "Shift+Enter"),
    kbl("Stop / cancel run", KeyGroup::Agent, "Esc"),
];

/// Render a config-style `(mods, key)` combo for humans, honoring the
/// platform's modifier glyphs (⌘/⌥/⌃/⇧ on macOS, Ctrl/Alt/Shift else).
pub fn format_combo(mods: &str, key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let mut parts: Vec<String> = Vec::new();
    for m in mods.split('|').map(|s| s.trim().to_lowercase()) {
        if m.is_empty() || m == "none" {
            continue;
        }
        let glyph = match m.as_str() {
            "super" | "command" => {
                if mac {
                    "⌘"
                } else {
                    "Ctrl"
                }
            }
            "control" => {
                if mac {
                    "⌃"
                } else {
                    "Ctrl"
                }
            }
            "alt" | "option" => {
                if mac {
                    "⌥"
                } else {
                    "Alt"
                }
            }
            "shift" => {
                if mac {
                    "⇧"
                } else {
                    "Shift"
                }
            }
            _ => continue,
        };
        parts.push(glyph.to_string());
    }
    parts.push(format_key_label(key));
    parts.join(if mac { " " } else { "+" })
}

fn format_key_label(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "space" => "Space".to_string(),
        "return" | "enter" => "Enter".to_string(),
        "esc" => "Esc".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        other if other.chars().count() == 1 => other.to_uppercase(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}
