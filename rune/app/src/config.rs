//! The user's appearance settings from `~/.config/neoism/config.json`, as GPUiX reads them.

/// `appearance.fonts.size` when the config has none.
pub const DEFAULT_FONT_SIZE: f32 = 15.0;

use serde_json::Value;

pub struct Config {
    /// `presence.display-name`.
    pub display_name: Option<String>,
    pub gold_avatar: bool,
    pub theme: String,
    pub font_size: f32,
    pub font_weight: f32,
    pub font_families: (String, String),
    pub wordmark_fire: bool,
    pub focused_view: bool,
    pub turn_at_top: bool,
    pub liquid_glass_cursor: bool,
    pub liquid_glass_selection: bool,
    pub markdown_spellcheck: bool,
    pub model: Option<String>,
    pub variant: Option<String>,
}

/// The shared desktop config directory, or the app's private Application Support on iOS.
pub fn dir() -> std::path::PathBuf {
    #[cfg(not(target_os = "ios"))]
    { neoism_agent_service_api::server_registry::registry_directory() }
    #[cfg(target_os = "ios")]
    {
        use std::{ffi::{CStr, c_char, c_void}, os::unix::ffi::OsStrExt, path::Path};
        #[link(name = "Foundation", kind = "framework")]
        unsafe extern "C" { fn NSHomeDirectory() -> *mut c_void; }
        #[link(name = "objc")]
        unsafe extern "C" {
            fn sel_registerName(name: *const c_char) -> *const c_void;
            fn objc_msgSend();
        }
        // Foundation returns this application's sandbox, not the system user's home.
        unsafe {
            let send: unsafe extern "C" fn(*mut c_void, *const c_void) -> *const c_char = std::mem::transmute(objc_msgSend as *const ());
            let home = send(NSHomeDirectory(), sel_registerName(c"fileSystemRepresentation".as_ptr()));
            assert!(!home.is_null(), "iOS app sandbox path is unavailable");
            Path::new(std::ffi::OsStr::from_bytes(CStr::from_ptr(home).to_bytes())).join("Library/Application Support/Neoism")
        }
    }
}

pub fn read() -> Value {
    std::fs::read_to_string(dir().join("config.json")).ok()
        .and_then(|s| serde_json::from_str(&strip_comments(&s)).ok())
        .unwrap_or(Value::Null)
}

/// `ui.window.background-image.path`, read fresh (the Backgrounds picker shows the live choice).
pub fn background_image() -> Option<String> {
    read()["ui"]["window"]["background-image"]["path"].as_str().map(str::to_owned)
}

pub fn load() -> Config {
    let path = Some(dir().join("config.json"));
    let value: Value = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&strip_comments(&s)).ok())
        .unwrap_or(Value::Null);
    let appearance = value.get("appearance").unwrap_or(&value);
    let theme = theme_id(&value).to_owned();
    let font_size = appearance["fonts"]["size"].as_f64().or(value["fonts"]["size"].as_f64()).unwrap_or(DEFAULT_FONT_SIZE as f64) as f32;
    let font_weight = appearance["fonts"]["weight"].as_f64().unwrap_or(400.0) as f32;
    let model = value["agent"]["model"].as_str().map(str::to_owned);
    let variant = value["agent"]["variant"].as_str().map(str::to_owned);
    let display_name = value["presence"]["display-name"].as_str().map(str::to_owned);
    let gold_avatar = value["presence"]["avatar-palette"].as_str() == Some("obsidian-gold");
    Config { display_name, gold_avatar, theme, font_size, font_weight, font_families: crate::ui::fonts::configured(&value), wordmark_fire: wordmark_fire(&value), focused_view: focused_view(&value), turn_at_top: turn_at_top(&value), liquid_glass_cursor: liquid_glass_cursor(&value), liquid_glass_selection: liquid_glass_selection(&value), markdown_spellcheck: markdown_spellcheck(&value), model, variant }
}

/// A sent message slides to the top of the view only when asked to (`agent/turn.rs`).
pub fn turn_at_top(value: &Value) -> bool {
    value["appearance"]["agent-turn-at-top"].as_bool().unwrap_or(false)
}

/// Turns fold to a work row and the reply unless this is turned off (`agent/focus.rs`).
pub fn focused_view(value: &Value) -> bool {
    value["appearance"]["agent-focused-view"].as_bool().unwrap_or(true)
}

/// Flames default on; an explicit grouped false disables them.
pub fn wordmark_fire(value: &Value) -> bool {
    value["appearance"]["wordmark-fire"].as_bool().unwrap_or(true)
}

/// Liquid glass is opt-in, including when an older daemon omits the setting.
pub fn liquid_glass_cursor(value: &Value) -> bool {
    value["appearance"]["effects"]["liquid-glass-cursor"].as_bool().unwrap_or(false)
}

pub fn liquid_glass_selection(value: &Value) -> bool {
    value["appearance"]["effects"]["liquid-glass-selection"].as_bool().unwrap_or(false)
}

pub fn markdown_spellcheck(value: &Value) -> bool {
    value["editor"]["markdown"]["spellcheck"].as_bool().unwrap_or(false)
}

/// Canonicalize the legacy default in memory; never rewrite the user's config on load.
fn theme_id(value: &Value) -> &str {
    let name = value["appearance"]["theme"].as_str().or(value["theme"].as_str()).unwrap_or("neoism_dark");
    if name == "pastel_dark" { "neoism_dark" } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_spelling_only_accepts_explicit_true() {
        for value in [Value::Null, serde_json::json!({}), serde_json::json!({"editor":{"markdown":{}}}),
            serde_json::json!({"editor":{"markdown":{"spellcheck":"true"}}}), serde_json::json!({"editor":{"markdown":{"spellcheck":false}}})] {
            assert!(!markdown_spellcheck(&value));
        }
        assert!(markdown_spellcheck(&serde_json::json!({"editor":{"markdown":{"spellcheck":true}}})));
    }

    #[test]
    fn selection_is_independent_and_defaults_off() {
        assert!(!liquid_glass_selection(&Value::Null));
        assert!(!liquid_glass_selection(&serde_json::json!({"appearance":{"effects":{"liquid-glass-cursor":true}}})));
        assert!(!liquid_glass_selection(&serde_json::json!({"appearance":{"effects":{"liquid-glass-selection":"true"}}})));
        assert!(liquid_glass_selection(&serde_json::json!({"appearance":{"effects":{"liquid-glass-selection":true}}})));
    }

    #[test]
    fn liquid_glass_cursor_defaults_off_and_reads_persisted_booleans() {
        for value in [Value::Null, serde_json::json!({}), serde_json::json!({"appearance": {"effects": {}}}), serde_json::json!({"appearance": {"effects": {"liquid-glass-cursor": "true"}}})] {
            assert!(!liquid_glass_cursor(&value));
        }
        for enabled in [false, true] {
            assert_eq!(liquid_glass_cursor(&serde_json::json!({"appearance": {"effects": {"liquid-glass-cursor": enabled}}})), enabled);
        }
    }

    #[test]
    fn wordmark_fire_defaults_on_and_reads_persisted_booleans() {
        for value in [Value::Null, serde_json::json!({}), serde_json::json!({"appearance": {}}), serde_json::json!({"appearance": {"wordmark-fire": "true"}})] {
            assert!(wordmark_fire(&value));
        }
        for enabled in [false, true] {
            assert_eq!(wordmark_fire(&serde_json::json!({"appearance": {"wordmark-fire": enabled}})), enabled);
        }
    }

    #[test]
    fn theme_defaults_and_persisted_ids() {
        assert_eq!(theme_id(&Value::Null), "neoism_dark");
        for name in ["pastel_dark", "neoism_dark", "neoism_light", "pastelDark", "one_light"] {
            let expected = if name == "pastel_dark" { "neoism_dark" } else { name };
            assert_eq!(theme_id(&serde_json::json!({"appearance": {"theme": name}})), expected);
            assert_eq!(theme_id(&serde_json::json!({"theme": name})), expected);
        }
    }
}

/// JSONC: drop `//` and `/* */` comments outside strings.
pub fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' { if let Some(n) = chars.next() { out.push(n); } } else if c == '"' { in_string = false; }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => { in_string = true; out.push(c); }
            ('/', Some('/')) => { for n in chars.by_ref() { if n == '\n' { out.push('\n'); break; } } }
            ('/', Some('*')) => { chars.next(); let mut prev = ' '; for n in chars.by_ref() { if prev == '*' && n == '/' { break; } prev = n; } }
            _ => out.push(c),
        }
    }
    out
}

pub fn git_branch() -> Option<String> {
    // Unit tests build windows by the dozen: no `git` children (a child forked mid-test holds
    // the registry tests' file locks until it execs).
    if cfg!(test) { return None; }
    // A Mac without the developer tools answers `git` with an install prompt: only ask git
    // where there is a repository to ask about.
    let mut dir = std::env::current_dir().ok()?;
    while !dir.join(".git").exists() { if !dir.pop() { return None; } }
    let output = crate::cli::quiet("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()).filter(|b| !b.is_empty())
}
