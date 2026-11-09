//! The Home block terminal (`reference/02`, `03`, motion `home-terminal-scroll`). libghostty-vt
//! (`ghostty.rs`) owns every byte of terminal state; this file only keeps command metadata, as
//! `gpuix/src/terminal/blocks.ts` did: the shell integration's OSC 133 marks (A prompt, B input,
//! C start, D;status end) and OSC 7 directories open and close blocks, and each block's rows are
//! captured out of Ghostty from a tracked mark, so reflow and pruning never misattribute output.

use crate::ghostty::{self, Colors, Position, Row, Vt};
use std::time::Instant;

pub const COLS: u16 = 132;
pub const ROWS: u16 = 40;
const MAX_BLOCKS: usize = 200;
const MAX_ROWS: usize = 20_000;

#[derive(Clone, Debug)]
pub struct Block {
    pub id: u64,
    pub command: String,
    pub cwd: String,
    pub started: Instant,
    /// `None` while the command runs.
    pub elapsed_ms: Option<u128>,
    pub exit: Option<i32>,
    pub lines: Vec<Row>,
    /// Output that arrived outside any command (background jobs, `motd`, ...).
    pub background: bool,
}

impl Block {
    pub fn failed(&self) -> bool { self.exit.is_some_and(|code| code != 0) }
}

struct Live {
    id: u64,
    command: String,
    cwd: String,
    started: Instant,
    start: Position,
    mark: u64,
    background: bool,
    clear_command: bool,
    saw_clear: bool,
    lines: Vec<Row>,
}

#[derive(Debug, PartialEq)]
enum ShellEvent {
    Prompt,
    Input,
    Start,
    End(Option<i32>),
    Directory(String),
    Clear,
}

/// Observes shell metadata without consuming bytes; Ghostty still parses every one
/// (port of `gpuix/src/terminal/shell-events.ts`).
#[derive(Default)]
struct ShellEvents {
    state: ScanState,
    payload: Vec<u8>,
    overflow: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum ScanState {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Str,
    StrEscape,
}

impl ShellEvents {
    fn reset(&mut self) {
        self.state = ScanState::Text;
        self.payload.clear();
        self.overflow = false;
    }

    /// Calls `emit(end, event)` with the byte index just past each event's sequence.
    fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(usize, ShellEvent)) {
        use ScanState::*;
        for (i, &byte) in bytes.iter().enumerate() {
            match self.state {
                Text => if byte == 27 { self.state = Escape },
                Escape => {
                    if byte == b'c' { emit(i + 1, ShellEvent::Clear); }
                    self.state = match byte { b'[' => Csi, b']' => Osc, b'P' | b'X' | b'^' | b'_' => Str, 27 => Escape, _ => Text };
                    self.payload.clear();
                    self.overflow = false;
                }
                Csi => {
                    if byte == 24 || byte == 26 { self.reset(); }
                    else if byte == 27 { self.reset(); self.state = Escape; }
                    else if (64..=126).contains(&byte) {
                        // Only ED 2/3 (erase screen / scrollback) matters to blocks.
                        if !self.overflow && byte == b'J' && (self.payload == b"2" || self.payload == b"3") { emit(i + 1, ShellEvent::Clear); }
                        self.reset();
                    } else if self.payload.len() < 32 { self.payload.push(byte); } else { self.overflow = true; }
                }
                Str | StrEscape => {
                    self.state = if self.state == StrEscape && byte == b'\\' { Text } else if byte == 27 { StrEscape } else { Str };
                }
                Osc | OscEscape => {
                    if byte == 24 || byte == 26 { self.reset(); continue; }
                    if byte == 7 || (self.state == OscEscape && byte == b'\\') {
                        if !self.overflow { if let Some(event) = osc_event(&self.payload) { emit(i + 1, event); } }
                        self.reset();
                        continue;
                    }
                    if self.state == OscEscape {
                        self.reset();
                        self.state = if byte == 27 { Escape } else { Text };
                        continue;
                    }
                    if byte == 27 { self.state = OscEscape; continue; }
                    if self.payload.len() < 8192 { self.payload.push(byte); } else { self.overflow = true; }
                }
            }
        }
    }
}

fn osc_event(payload: &[u8]) -> Option<ShellEvent> {
    let value = std::str::from_utf8(payload).ok()?;
    if let Some(rest) = value.strip_prefix("133;").or_else(|| value.strip_prefix("633;")) {
        let mut parts = rest.split(';');
        let kind = parts.next()?;
        let status = parts.next().and_then(|s| s.parse().ok());
        return match kind {
            "A" => Some(ShellEvent::Prompt),
            "B" => Some(ShellEvent::Input),
            "C" => Some(ShellEvent::Start),
            "D" => Some(ShellEvent::End(status)),
            _ => None,
        };
    }
    let url = value.strip_prefix("7;file://")?;
    let path = &url[url.find('/')?..];
    Some(ShellEvent::Directory(percent_decode(path)))
}

pub struct Terminal {
    pub session: Option<String>,
    pub cwd: String,
    pub blocks: Vec<Block>,
    /// The running command or background output, captured after each feed.
    pub live: Option<Block>,
    pub error: Option<String>,
    /// The shell printed B and is waiting for a command.
    pub ready: bool,
    pub recent: Vec<String>,
    vt: Option<Vt>,
    colors: Colors,
    events: ShellEvents,
    running: Option<Live>,
    background: Option<Live>,
    input: Option<(Position, u64)>,
    submitted: String,
    pending: bool,
    prompting: bool,
    next_id: u64,
    offset: Option<u64>,
}

impl Terminal {
    pub fn new(cwd: String, colors: Colors) -> Terminal {
        let (vt, error) = match Vt::new(COLS, ROWS, &colors) {
            Ok(vt) => (Some(vt), None),
            Err(error) => (None, Some(error)),
        };
        Terminal {
            session: None, cwd, blocks: Vec::new(), live: None, error, ready: false, recent: Vec::new(), vt, colors,
            events: ShellEvents::default(), running: None, background: None, input: None, submitted: String::new(),
            pending: false, prompting: false, next_id: 0, offset: None,
        }
    }

    /// A fresh shell on a new PTY: forget the old engine state but keep finished blocks.
    pub fn attach(&mut self, session_id: String) {
        self.reset_engine();
        self.session = Some(session_id);
    }

    fn reset_engine(&mut self) {
        self.running = None;
        self.background = None;
        self.input = None;
        self.live = None;
        self.submitted.clear();
        self.pending = false;
        self.ready = false;
        self.prompting = false;
        self.offset = None;
        self.events.reset();
        if self.error.is_none() {
            match Vt::new(COLS, ROWS, &self.colors) {
                Ok(vt) => self.vt = Some(vt),
                Err(error) => self.error = Some(error),
            }
        }
    }

    /// Stage `command` from the composer. False while a command is still running or the shell
    /// has not reached its input prompt; the caller then sends the line as raw input.
    pub fn prepare(&mut self, command: &str) -> bool {
        if !self.ready || self.pending || self.running.is_some() || command.trim().is_empty() { return false; }
        self.finish_background();
        self.submitted = command.to_owned();
        self.pending = true;
        self.ready = false;
        true
    }

    /// Feed PTY output. Returns Ghostty's replies, which belong back on the PTY.
    pub fn feed(&mut self, bytes: &[u8], offset: Option<u64>) -> Vec<u8> {
        let mut data = bytes;
        if let (Some(offset), Some(seen)) = (offset, self.offset) {
            // Replayed output after a reattach: skip what Ghostty already has.
            if offset < seen { data = &bytes[((seen - offset) as usize).min(bytes.len())..]; }
            if offset > seen { self.reset_engine(); }
        }
        if let Some(offset) = offset { self.offset = Some(self.offset.unwrap_or(0).max(offset + bytes.len() as u64)); }
        if self.vt.is_none() || data.is_empty() { return Vec::new(); }
        if self.running.is_none() && self.background.is_none() && !self.prompting && !self.pending {
            self.background = self.begin("");
        }
        let mut events = Vec::new();
        self.events.feed(data, |end, event| events.push((end, event)));
        let mut replies = Vec::new();
        let mut start = 0;
        for (end, event) in events {
            let was_alt = self.alt();
            if let Some(vt) = &mut self.vt { replies.extend(vt.write(&data[start..end])); }
            start = end;
            self.event(event, was_alt);
        }
        if start < data.len() {
            if let Some(vt) = &mut self.vt { replies.extend(vt.write(&data[start..])); }
        }
        self.refresh_live();
        replies
    }

    fn begin(&mut self, command: &str) -> Option<Live> {
        let vt = self.vt.as_mut()?;
        let start = vt.position();
        let mark = vt.mark().ok()?;
        self.next_id += 1;
        Some(Live {
            id: self.next_id, command: command.to_owned(), cwd: self.cwd.clone(), started: Instant::now(), start, mark,
            background: command.is_empty(), clear_command: false, saw_clear: false, lines: Vec::new(),
        })
    }

    /// Re-read a live block's rows from Ghostty (never from a viewport copy).
    fn capture(vt: &Vt, block: &mut Live) {
        let end = vt.position();
        if end.alt { return; }
        let Ok(capture) = vt.capture(block.start.row, end.row + 1, Some(block.mark)) else { return };
        if capture.truncated && capture.rows.is_empty() { return; }
        let mut rows = capture.rows;
        // B usually follows visible prompt ink on the same row; never archive that prefix.
        if block.background && block.start.col > 0 && capture.start == block.start.row && !capture.truncated {
            if let Some(first) = rows.first_mut() { *first = after_column(first, block.start.col as usize); }
        }
        block.lines = rows;
    }

    fn refresh_live(&mut self) {
        let Some(vt) = &self.vt else { return };
        let active = if self.running.is_some() { &mut self.running } else { &mut self.background };
        self.live = active.as_mut().and_then(|block| {
            Self::capture(vt, block);
            let visible = !block.background || block.lines.iter().any(|r| ghostty::row_visible(r));
            visible.then(|| Block {
                id: block.id, command: block.command.clone(), cwd: block.cwd.clone(), started: block.started,
                elapsed_ms: None, exit: None, lines: block.lines.clone(), background: block.background,
            })
        });
    }

    fn archive(&mut self, mut block: Live, exit: Option<i32>) {
        if let Some(vt) = &mut self.vt {
            Self::capture(vt, &mut block);
            vt.unmark(block.mark);
        }
        let mut lines = block.lines;
        while lines.last().is_some_and(|r| !ghostty::row_visible(r)) { lines.pop(); }
        if !block.background || !lines.is_empty() {
            self.blocks.push(Block {
                id: block.id, command: block.command, cwd: block.cwd, started: block.started,
                elapsed_ms: Some(block.started.elapsed().as_millis()), exit, lines, background: block.background,
            });
        }
        let mut rows: usize = self.blocks.iter().map(|b| b.lines.len()).sum();
        while self.blocks.len() > MAX_BLOCKS || rows > MAX_ROWS {
            rows -= self.blocks.remove(0).lines.len();
        }
    }

    fn finish_background(&mut self) {
        if let Some(block) = self.background.take() { self.archive(block, None); }
    }

    fn finish(&mut self, exit: Option<i32>) {
        if let Some(block) = self.running.take() { self.archive(block, exit); }
    }

    fn alt(&self) -> bool { self.vt.as_ref().is_some_and(|vt| vt.position().alt) }

    fn event(&mut self, event: ShellEvent, was_alt: bool) {
        match event {
            ShellEvent::Clear => {
                if was_alt || self.alt() { return; }
                if self.running.as_ref().is_some_and(|r| r.clear_command) {
                    // `clear` only wipes history once it succeeds (at D).
                    if let (Some(vt), Some(running)) = (self.vt.as_mut(), self.running.as_mut()) {
                        running.saw_clear = true;
                        vt.unmark(running.mark);
                        running.start = vt.position();
                        if let Ok(mark) = vt.mark() { running.mark = mark; }
                        running.lines.clear();
                    }
                } else if self.running.is_none() && self.ready && !self.pending {
                    // Readline's Ctrl-L repaint.
                    self.clear();
                }
            }
            ShellEvent::Directory(path) => self.cwd = path,
            ShellEvent::Start => {
                if self.running.is_some() { return; }
                self.finish_background();
                self.prompting = false;
                let mut command = std::mem::take(&mut self.submitted);
                if command.is_empty() {
                    // Typed straight into the PTY: read the command back out of Ghostty.
                    if let (Some(vt), Some((input, mark))) = (&self.vt, self.input) {
                        let start = vt.position();
                        if let Ok(capture) = vt.capture(input.row, start.row + u64::from(start.col > 0), Some(mark)) {
                            let mut rows = capture.rows;
                            if let Some(first) = rows.first_mut() { *first = after_column(first, input.col as usize); }
                            command = rows.iter().map(|r| ghostty::row_text(r)).collect::<Vec<_>>().join("\n").trim().to_owned();
                        }
                    }
                }
                let label = if command.is_empty() { "Shell command".to_owned() } else { command.clone() };
                self.running = self.begin(&label);
                if let Some(running) = &mut self.running {
                    running.background = false;
                    running.clear_command = is_clear_command(&command);
                }
                if !command.is_empty() {
                    self.recent.push(command);
                    if self.recent.len() > 1000 { self.recent.remove(0); }
                }
                self.ready = false;
                self.pending = false;
            }
            ShellEvent::End(status) => {
                if status == Some(0) && self.running.as_ref().is_some_and(|r| r.clear_command && r.saw_clear) {
                    if let (Some(vt), Some(running)) = (self.vt.as_mut(), self.running.take()) { vt.unmark(running.mark); }
                    self.clear();
                }
                self.finish(status);
                self.pending = false;
                self.submitted.clear();
                if !self.alt() { self.background = self.begin(""); }
            }
            ShellEvent::Prompt => {
                self.finish(None);
                self.finish_background();
                self.prompting = true;
            }
            ShellEvent::Input => {
                // A ready prompt also ends commands from integrations that omit D.
                self.finish(None);
                self.finish_background();
                self.prompting = false;
                if let Some(vt) = &mut self.vt {
                    if let Some((_, mark)) = self.input.take() { vt.unmark(mark); }
                    let position = vt.position();
                    if let Ok(mark) = vt.mark() { self.input = Some((position, mark)); }
                }
                self.ready = true;
                self.pending = false;
                self.submitted.clear();
                self.background = self.begin("");
            }
        }
    }

    /// Drop every block and capture (the `clear` command, Ctrl-L at a prompt).
    pub fn clear(&mut self) {
        if let Some(vt) = &mut self.vt {
            for live in [self.running.take(), self.background.take()].into_iter().flatten() { vt.unmark(live.mark); }
        }
        self.blocks.clear();
        self.live = None;
        // A prompt redraw without fresh OSC marks is not background output.
        self.prompting = true;
    }
}

fn is_clear_command(command: &str) -> bool {
    let command = command.trim();
    let command = command.strip_prefix("command ").map(str::trim).unwrap_or(command);
    let mut words = command.split_whitespace();
    let name = words.next().unwrap_or("");
    let base = name.rsplit('/').next().unwrap_or(name);
    let rest: Vec<&str> = words.collect();
    (base == "clear" && (rest.is_empty() || rest == ["-x"])) || (base == "reset" && rest.is_empty())
}

/// The part of a row from cell `column` on.
fn after_column(row: &Row, column: usize) -> Row {
    row.iter().filter_map(|run| {
        let skip = column.saturating_sub(run.col);
        if skip >= run.width { return None; }
        let chars: Vec<char> = run.text.chars().collect();
        let drop = (skip * chars.len()).div_ceil(run.width.max(1));
        Some(ghostty::Run { text: chars[drop.min(chars.len())..].iter().collect(), col: run.col.max(column) - column, width: run.width - skip, ..run.clone() })
    }).collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_osc_133_and_7() {
        let mut events = ShellEvents::default();
        let mut seen = Vec::new();
        events.feed(b"\x1b]133;A\x07$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07out\x1b]133;D;2\x07\x1b]7;file://host/tmp/a%20b\x07\x1b[2J", |_, e| seen.push(e));
        assert_eq!(seen, vec![ShellEvent::Prompt, ShellEvent::Input, ShellEvent::Start, ShellEvent::End(Some(2)),
            ShellEvent::Directory("/tmp/a b".into()), ShellEvent::Clear]);
    }

    #[test]
    fn clear_commands() {
        assert!(is_clear_command("clear"));
        assert!(is_clear_command("/usr/bin/clear -x"));
        assert!(!is_clear_command("clear foo"));
        assert!(!is_clear_command("echo clear"));
    }

    #[test]
    fn blocks_from_shell_marks() {
        let mut palette = [0x101010; 16];
        palette[1] = 0xff0000;
        let mut t = Terminal::new("/tmp".into(), Colors { palette, fg: 0xffffff, bg: 0 });
        assert!(t.error.is_none(), "{:?}", t.error);
        t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07", None);
        assert!(t.ready);
        assert!(t.prepare("printf hi"));
        t.feed(b"printf hi\r\n\x1b]133;C\x07hi\r\n\x1b[31mthere\x1b[0m\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07", None);
        assert_eq!(t.blocks.len(), 1, "{:?}", t.blocks);
        let block = &t.blocks[0];
        assert_eq!(block.command, "printf hi");
        assert_eq!(block.exit, Some(0));
        let text: Vec<String> = block.lines.iter().map(|r| ghostty::row_text(r)).collect();
        assert_eq!(text, vec!["hi", "there"]);
        assert_eq!(block.lines[1][0].fg, Some(0xff0000));
        // Typed directly into the PTY: the command is read back out of Ghostty.
        t.feed(b"false\r\n\x1b]133;C\x07\x1b]133;D;1\x07\x1b]133;A\x07$ \x1b]133;B\x07", None);
        assert_eq!(t.blocks.last().map(|b| (b.command.as_str(), b.failed())), Some(("false", true)));
    }
}
