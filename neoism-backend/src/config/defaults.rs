use crate::config::Shell;
use neoism_terminal_core::ansi::CursorShape;

#[inline]
pub fn default_bool_true() -> bool {
    true
}

#[inline]
pub fn default_line_height() -> f32 {
    1.0
}

#[inline]
pub fn default_cursor_interval() -> u64 {
    // Half-period in ms. ~530ms matches classic terminal cadence (and
    // the chrome carets' 500/530ms) — the old 800ms read as sluggish.
    530
}

#[inline]
pub fn default_scrollback_history_limit() -> usize {
    10_000
}

#[inline]
pub fn default_title_placeholder() -> Option<String> {
    Some(String::from("▲"))
}

#[inline]
pub fn default_title_content() -> String {
    #[cfg(unix)]
    return String::from("{{ TITLE || RELATIVE_PATH }}");

    #[cfg(not(unix))]
    return String::from("{{ TITLE || PROGRAM }}");
}

#[inline]
pub fn default_margin() -> crate::config::layout::Margin {
    crate::config::layout::Margin::all(2.0)
}

#[inline]
pub fn default_shell() -> crate::config::Shell {
    #[cfg(not(target_os = "windows"))]
    {
        // IDE fork: prefer zsh as the project default. Probe the
        // common system paths first, then fall back to walking $PATH
        // (covers Nix, Homebrew, and other non-FHS layouts). Bash is
        // the safety net so first-run never fails to spawn a shell.
        let program =
            find_shell_in_path("zsh").unwrap_or_else(|| String::from("/bin/bash"));
        crate::config::Shell {
            program,
            args: vec![String::from("--login")],
        }
    }

    #[cfg(target_os = "windows")]
    {
        crate::config::Shell {
            program: String::from("powershell"),
            args: vec![],
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn find_shell_in_path(name: &str) -> Option<String> {
    use std::path::Path;
    for fixed in ["/bin", "/usr/bin", "/usr/local/bin"] {
        let candidate = Path::new(fixed).join(name);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

#[inline]
pub fn default_use_fork() -> bool {
    #[cfg(target_os = "macos")]
    {
        false
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[inline]
pub fn default_working_dir() -> Option<String> {
    None
}

#[inline]
pub fn default_opacity() -> f32 {
    1.0
}

#[inline]
pub fn default_option_as_alt() -> String {
    #[cfg(target_os = "macos")]
    {
        String::from("both")
    }
    #[cfg(not(target_os = "macos"))]
    {
        String::from("none")
    }
}

#[inline]
pub fn default_log_level() -> String {
    String::from("OFF")
}

#[inline]
pub fn default_cursor() -> CursorShape {
    CursorShape::default()
}

#[inline]
pub fn default_theme() -> String {
    String::from("")
}

#[inline]
pub fn default_neoism_theme() -> String {
    String::from("pastel_dark")
}

#[inline]
pub fn default_editor() -> Shell {
    #[cfg(not(target_os = "windows"))]
    {
        Shell {
            program: String::from("vi"),
            args: vec![],
        }
    }

    #[cfg(target_os = "windows")]
    {
        Shell {
            program: String::from("notepad"),
            args: vec![],
        }
    }
}

#[inline]
pub fn default_window_width() -> i32 {
    800
}

#[inline]
pub fn default_window_height() -> i32 {
    490
}

#[inline]
pub fn default_disable_ctlseqs_alt() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[inline]
pub fn default_ime_cursor_positioning() -> bool {
    true
}

pub fn default_config_file_content() -> String {
    // Written to a fresh `config.json`. Comments are legal — the loader
    // accepts JSONC (`//`, `/* */`, trailing commas).
    String::from(
        "// See the full configuration reference: https://neoism.com/docs/config\n{\n}\n",
    )
}
