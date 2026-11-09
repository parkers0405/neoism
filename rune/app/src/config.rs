//! The user's appearance settings from `~/.config/neoism/config.json`, as GPUiX reads them.

use serde_json::Value;

pub struct Config {
    pub theme: String,
    pub font_size: f32,
    pub font_weight: f32,
    pub model: Option<String>,
    pub variant: Option<String>,
}

pub fn load() -> Config {
    let path = std::env::var_os("NEOISM_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::config_dir().map(|d| d.join("neoism")))
        .map(|d| d.join("config.json"));
    let value: Value = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&strip_comments(&s)).ok())
        .unwrap_or(Value::Null);
    let appearance = value.get("appearance").unwrap_or(&value);
    let theme = appearance["theme"].as_str().or(value["theme"].as_str()).unwrap_or("pastel_dark").to_owned();
    let font_size = appearance["fonts"]["size"].as_f64().or(value["fonts"]["size"].as_f64()).unwrap_or(15.0) as f32;
    let font_weight = appearance["fonts"]["weight"].as_f64().unwrap_or(400.0) as f32;
    let model = value["agent"]["model"].as_str().map(str::to_owned);
    let variant = value["agent"]["variant"].as_str().map(str::to_owned);
    Config { theme, font_size, font_weight, model, variant }
}

/// JSONC: drop `//` and `/* */` comments outside strings.
fn strip_comments(source: &str) -> String {
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
    let output = std::process::Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()).filter(|b| !b.is_empty())
}
