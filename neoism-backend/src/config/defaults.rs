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
        // Prefer PowerShell 7 (pwsh) when it is on PATH; classic
        // Windows PowerShell ships with the OS and is the safety net.
        let program = find_shell_in_path("pwsh.exe")
            .unwrap_or_else(|| String::from("powershell.exe"));
        crate::config::Shell {
            program,
            args: vec![String::from("-NoLogo")],
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn find_shell_in_path(name: &str) -> Option<String> {
    use std::path::PathBuf;
    // System locations first, then the common user-profile package-manager
    // bin dirs (Nix, Homebrew, ~/.local). A neoism launched from a display
    // manager / app launcher frequently inherits a MINIMAL $PATH that omits
    // these, so a zsh installed via `nix profile` or Homebrew would go unseen
    // and we'd silently fall back to bash — the "I installed zsh but the
    // terminal is still bash" trap. Probe them explicitly so the zsh default
    // actually wins wherever zsh is present.
    let mut probe: Vec<PathBuf> = vec![
        PathBuf::from("/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin"),
        PathBuf::from("/nix/var/nix/profiles/default/bin"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        probe.push(home.join(".nix-profile/bin"));
        probe.push(home.join(".local/state/nix/profile/bin"));
        probe.push(home.join(".local/bin"));
    }
    for dir in &probe {
        let candidate = dir.join(name);
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

#[cfg(target_os = "windows")]
fn find_shell_in_path(name: &str) -> Option<String> {
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
    // Written to a fresh `config.json`. The loader accepts JSONC, so the
    // `//` comments and trailing commas below are legal. Every key is
    // commented out — uncomment and edit what you want; anything you omit
    // uses its default. The examples show the common knobs; the full
    // reference lives in the in-app docs; Alt+, opens this file with
    // built-in completion for keys and host-specific values.
    String::from(
        r##"// Neoism configuration — JSONC (// comments and trailing commas are fine).
// One file, shared by the terminal and the agent. Settings are grouped by
// domain: uncomment a block and edit the keys you want; anything omitted
// uses its default. Press Alt+, to reopen this file. Completion lists every
// supported key plus values available on this host (fonts, agents, LSPs, etc.).
// Full reference: Alt+N → Default → Welcome → Getting Started → Configure Neoism.
{
    // ── [appearance] — theming + typography ───────────────────────
    // "appearance": {
    //     "theme": "tokyo_night",    // IDE theme: pastel_dark | nvchad_one | tokyo_night | catppuccin_mocha
    //     "palette": "lucario",      // terminal color file in themes/<name>.json
    //     "line-height": 1.2,
    //     "mashup-pack": "synth",    // active Mash Up Pack under packs/<id>
    //     "fonts": { "family": "CascadiaCode", "size": 14.0, "weight": 400 },
    //     "effects": { "trail-cursor": true },
    // },

    // ── [editor] — the built-in code + markdown editor ────────────
    // "editor": {
    //     "vim-mode": true,
    //     "format-on-save": true,    // run the LSP formatter before every save
    //     "minimap": true,
    // },

    // ── [terminal] — the terminal emulator ────────────────────────
    // "terminal": {
    //     "shell": { "program": "/bin/fish", "args": ["--login"] },
    //     "cursor": { "shape": "beam", "blinking": true },
    //     "scroll": { "multiplier": 3.0 },
    //     "scrollback-history-limit": 10000,
    //     "copy-on-select": false,
    //     "hide-mouse-cursor-when-typing": false,
    // },

    // ── [ui] — window chrome, tabs, status line ───────────────────
    // "ui": {
    //     "window": { "opacity": 0.95, "blur": true },
    //     "navigation": { "hide-if-single": true, "use-split": true },
    //     "status-fps": true,        // FPS pill on the status bar
    //     "confirm-before-quit": false,
    // },

    // ── [presence] — how collaborators see you in multiplayer ─────
    // "presence": {
    //     "display-name": "parker",  // the name collaborators see
    //     "cursor-color": "#44C9F0", // your caret colour (collaborators see it too)
    //     "cursor-style": "rainbow", // "solid" (default) or "rainbow"
    // },

    // ── [keybinds] — override the built-in shortcuts ──────────────
    // "keybinds": { "keys": [ { "key": "t", "with": "super", "action": "CreateTab" } ] },

    // ── [agent] — the coding agent (its own block, same file) ─────
    // "agent": {
    //     "model": "anthropic/claude-opus-5",
    //     "smallModel": "anthropic/claude-haiku-4-5",
    //     "variant": "high",       // low | medium | high | xhigh | max
    //     "textVerbosity": "low",  // low | medium | high
    //     "permission": { "edit": "ask", "bash": "ask" },
    //     "mcp": { "my-server": { "type": "local", "command": ["my-mcp"] } },
    // },

    // ── [developer] ───────────────────────────────────────────────
    // "developer": { "log-level": "info", "enable-log-file": true },
}
"##,
    )
}
