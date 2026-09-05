//! Terminal input model the chrome panels render against.
//!
//! `InputBuffer` is the surface `command_composer` and `completion_menu`
//! consume — they only read the shape of the user's pending command,
//! the cursor, completion flash, and the shell kind. Both native (via
//! `frontends/neoism::terminal::blocks::TerminalInputBuffer`) and web
//! (via a wire-message snapshot) implement this trait.
//!
//! The flash and shell-kind types are POD lifted out of
//! `frontends/neoism::terminal::blocks` so the shared panels don't have
//! to depend on native-only state machinery.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalShellKind {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
    Unknown,
}

impl TerminalShellKind {
    pub fn label(self) -> &'static str {
        match self {
            TerminalShellKind::Bash => "bash",
            TerminalShellKind::Zsh => "zsh",
            TerminalShellKind::Fish => "fish",
            TerminalShellKind::PowerShell => "powershell",
            TerminalShellKind::Cmd => "cmd",
            TerminalShellKind::Unknown => "sh",
        }
    }

    /// Detect the shell kind from the program path the host launched
    /// (usually `config.shell.program`). Matches by file_name so
    /// `/usr/bin/zsh`, `zsh`, and the login-shell `-zsh` form all
    /// produce `Zsh`.
    pub fn detect(program: &str) -> Self {
        let name = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .trim_start_matches('-')
            .to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        match name {
            "bash" => TerminalShellKind::Bash,
            "zsh" => TerminalShellKind::Zsh,
            "fish" => TerminalShellKind::Fish,
            "powershell" | "pwsh" => TerminalShellKind::PowerShell,
            "cmd" => TerminalShellKind::Cmd,
            _ => TerminalShellKind::Unknown,
        }
    }

    /// Sanitise + frame the command bytes the host should write into
    /// the PTY when the user submits. Fish wants ` \x08\n` to drop the
    /// inline autosuggestion before executing; every other shell just
    /// needs the newline.
    pub fn command_payload(self, command: &str, _bracketed_paste: bool) -> Vec<u8> {
        let sanitized = crate::terminal_blocks::shell::sanitize_input_text(command);
        let mut bytes = Vec::with_capacity(sanitized.len() + 4);
        bytes.extend_from_slice(sanitized.as_bytes());
        bytes.extend_from_slice(match self {
            TerminalShellKind::Fish => b" \x08\n",
            TerminalShellKind::PowerShell | TerminalShellKind::Cmd => b"\r",
            _ => b"\n",
        });
        bytes
    }

    /// Build a shell-safe command that changes only the target terminal's
    /// process directory. The path is always passed as one literal argument;
    /// controls are rejected rather than normalized so no second command can
    /// be smuggled through the palette/OSC path.
    pub fn change_directory_payload(self, path: &str) -> Result<Vec<u8>, &'static str> {
        if path.is_empty()
            || path
                .chars()
                .any(|ch| ch == '\0' || ch == '\r' || ch == '\n' || ch.is_control())
        {
            return Err("directory contains a control character");
        }
        let command = match self {
            TerminalShellKind::PowerShell => {
                format!("Set-Location -LiteralPath '{}'", path.replace('\'', "''"))
            }
            TerminalShellKind::Cmd => {
                if path.contains('"') {
                    return Err("directory contains an unsupported quote");
                }
                format!("cd /d \"{path}\"")
            }
            TerminalShellKind::Bash
            | TerminalShellKind::Zsh
            | TerminalShellKind::Fish
            | TerminalShellKind::Unknown => {
                format!("cd -- '{}'", path.replace('\'', "'\\''"))
            }
        };
        Ok(self.command_payload(&command, false))
    }
}

/// Live animation parameters the composer needs each frame. Computed
/// upstream from a `CompletionFlash` + elapsed time. `None` once the
/// flash has expired.
#[derive(Debug, Clone, Copy)]
pub enum CompletionFlashState {
    /// `intensity` ramps 1.0 → 0.0 over the success window.
    Success {
        range: (usize, usize),
        intensity: f32,
    },
    /// `shake_offset_logical` is the horizontal offset (in logical
    /// pixels) to apply to the editable run; `intensity` ramps 1.0 →
    /// 0.0 so red tint and shake fade together.
    NoMatch {
        shake_offset_logical: f32,
        intensity: f32,
    },
}

/// Read-only view onto the terminal's pending input the composer
/// renders. Native impl forwards to `TerminalInputBuffer`; web impl
/// reads a per-frame snapshot pushed from the daemon.
pub trait InputBuffer {
    fn text(&self) -> &str;
    fn cursor_byte(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn completion_items(&self) -> &[String];
    fn completion_detail(&self) -> Option<&str> {
        None
    }
    fn flash_state(&self) -> Option<CompletionFlashState>;
    fn control_notice(&self) -> Option<&'static str> {
        None
    }
    fn prompt_burst_elapsed_ms(&self) -> Option<f32>;
    fn suggestion_after_cursor(&self) -> Option<&str>;
    fn is_prompt_animating(&self) -> bool;
    fn shell_kind(&self) -> TerminalShellKind {
        TerminalShellKind::Unknown
    }
}

/// Minimal host-fed input snapshot for frontends that do not yet run
/// the native `TerminalInputBuffer` state machine. It is intentionally
/// read-only from the composer point of view: the host owns mutation
/// and pushes a fresh string/cursor after translating platform input.
#[derive(Debug, Clone)]
pub struct SimpleInputBuffer {
    text: String,
    cursor_byte: usize,
    completion_items: Vec<String>,
    shell_kind: TerminalShellKind,
}

impl Default for SimpleInputBuffer {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor_byte: 0,
            completion_items: Vec::new(),
            shell_kind: TerminalShellKind::Bash,
        }
    }
}

impl SimpleInputBuffer {
    pub fn set_text(&mut self, text: String) {
        self.cursor_byte = text.len();
        self.text = text;
        self.completion_items.clear();
    }

    pub fn set_snapshot(
        &mut self,
        text: String,
        cursor_byte: usize,
        completion_items: Vec<String>,
    ) {
        self.cursor_byte = cursor_byte.min(text.len());
        self.text = text;
        self.completion_items = completion_items;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor_byte = 0;
        self.completion_items.clear();
    }

    /// Insert `s` at the cursor and advance past it. Used by frontends
    /// that own a small self-driven text field (e.g. the git panel's
    /// commit-message box) rather than a host-fed snapshot.
    pub fn insert_str(&mut self, s: &str) {
        let at = self.cursor_byte.min(self.text.len());
        self.text.insert_str(at, s);
        self.cursor_byte = at + s.len();
        self.completion_items.clear();
    }

    /// Delete the character immediately before the cursor.
    pub fn backspace(&mut self) {
        let at = self.cursor_byte.min(self.text.len());
        if at == 0 {
            return;
        }
        let prev = self.text[..at]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.replace_range(prev..at, "");
        self.cursor_byte = prev;
        self.completion_items.clear();
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_shell_kind(&mut self, shell_kind: TerminalShellKind) {
        self.shell_kind = shell_kind;
    }
}

impl InputBuffer for SimpleInputBuffer {
    fn text(&self) -> &str {
        &self.text
    }

    fn cursor_byte(&self) -> usize {
        self.cursor_byte.min(self.text.len())
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn completion_items(&self) -> &[String] {
        &self.completion_items
    }

    fn flash_state(&self) -> Option<CompletionFlashState> {
        None
    }

    fn prompt_burst_elapsed_ms(&self) -> Option<f32> {
        None
    }

    fn suggestion_after_cursor(&self) -> Option<&str> {
        None
    }

    fn is_prompt_animating(&self) -> bool {
        false
    }

    fn shell_kind(&self) -> TerminalShellKind {
        self.shell_kind
    }
}
