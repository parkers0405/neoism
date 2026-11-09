//! Command blocks for the Home terminal (`reference/02`, `03`, motion `home-terminal-scroll`). libghostty-vt
//! (`crate::ghostty`) owns every byte of terminal state; this file only keeps command metadata, as
//! `gpuix/src/terminal/blocks.ts` did: the shell integration's OSC 133 marks (A prompt, B input,
//! C start, D;status end) and OSC 7 directories open and close blocks, and each block's rows are
//! captured out of Ghostty from a tracked mark, so reflow and pruning never misattribute output.

use super::shell_events::{ShellEvent, ShellEvents};
use crate::ghostty::{self, Colors, KeyInput, MouseInput, Position, Row, Screen, Vt};
use std::time::Instant;

/// The grid before the pane has been measured.
pub const COLS: u16 = 120;
pub const ROWS: u16 = 32;
const MAX_BLOCKS: usize = 200;
const MAX_ROWS: usize = 20_000;
/// A live block re-reads only this many rows above where its cursor was at the last read.
/// Appending output is the common case; progress bars move up a few lines at most.
const LIVE_REREAD_ROWS: usize = 64;

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
    /// A running block's cursor: `(row within lines, column)`.
    pub cursor: Option<(usize, u16)>,
}

impl Block {
    pub fn failed(&self) -> bool { self.exit.is_some_and(|code| code != 0) }
    pub fn running(&self) -> bool { self.elapsed_ms.is_none() }
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
    /// Row (within `lines`) the cursor was on at the last read.
    cursor_row: usize,
    cursor: Option<(usize, u16)>,
    /// The next read must take every row (after a resize reflowed them).
    stale: bool,
}

/// One shell: Ghostty plus the block model built from its shell-integration marks.
pub struct Shell {
    pub session: Option<String>,
    pub cwd: String,
    pub blocks: Vec<Block>,
    /// The running command or background output, as of the last `sync`.
    pub live: Option<Block>,
    pub error: Option<String>,
    /// The shell printed B and is waiting for a command.
    pub ready: bool,
    pub recent: Vec<String>,
    vt: Option<Vt>,
    colors: Colors,
    size: (u16, u16),
    cell: (u32, u32),
    events: ShellEvents,
    running: Option<Live>,
    background: Option<Live>,
    input: Option<(Position, u64)>,
    submitted: String,
    pending: bool,
    prompting: bool,
    next_id: u64,
    offset: Option<u64>,
    /// Output arrived since the last `sync`.
    dirty: bool,
}

impl Shell {
    pub fn new(cwd: String, colors: Colors) -> Shell {
        let (vt, error) = match Vt::new(COLS, ROWS, &colors) {
            Ok(vt) => (Some(vt), None),
            Err(error) => (None, Some(error)),
        };
        Shell {
            session: None, cwd, blocks: Vec::new(), live: None, error, ready: false, recent: Vec::new(), vt, colors,
            size: (COLS, ROWS), cell: (9, 20), events: ShellEvents::default(), running: None, background: None, input: None,
            submitted: String::new(), pending: false, prompting: false, next_id: 0, offset: None, dirty: false,
        }
    }

    /// Re-theme Ghostty's palette; already captured rows keep the colours they were drawn with.
    pub fn set_colors(&mut self, colors: &Colors) {
        self.colors = colors.clone();
        if let Some(vt) = &mut self.vt { vt.set_colors(colors); }
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
            match Vt::new(self.size.0, self.size.1, &self.colors) {
                Ok(mut vt) => {
                    vt.resize(self.size.0, self.size.1, self.cell);
                    self.vt = Some(vt);
                }
                Err(error) => self.error = Some(error),
            }
        }
    }

    /// The grid size the PTY should have.
    pub fn size(&self) -> (u16, u16) { self.size }

    /// Resize Ghostty; returns whether the grid changed (and the PTY needs a `Resize`).
    pub fn resize(&mut self, cols: u16, rows: u16, cell: (u32, u32)) -> bool {
        self.cell = cell;
        let Some(vt) = &mut self.vt else { self.size = (cols, rows); return false };
        if !vt.resize(cols, rows, cell) { return false; }
        self.size = vt.size();
        // Reflow moves rows: the next read of a live block takes all of them.
        for live in [&mut self.running, &mut self.background].into_iter().flatten() { live.stale = true; }
        self.dirty = true;
        true
    }

    /// A command was submitted and its output has not started yet.
    pub fn pending(&self) -> bool { self.pending }
    /// A command is running (between C and D).
    pub fn running(&self) -> bool { self.running.is_some() }
    pub fn busy(&self) -> bool { self.pending || self.running.is_some() }
    /// A full-screen program owns the alternate screen.
    pub fn alt(&self) -> bool { self.vt.as_ref().is_some_and(|vt| vt.position().alt) }
    /// The program asked for application cursor keys (readline REPLs, editors).
    pub fn app_cursor(&self) -> bool { self.vt.as_ref().is_some_and(|vt| vt.mode(1)) }
    /// Alternate scroll mode: wheel steps become arrow keys on the alternate screen.
    pub fn alternate_scroll(&self) -> bool { self.vt.as_ref().is_some_and(|vt| vt.mode(1007)) }

    /// The visible grid, for full-screen programs and direct input.
    pub fn screen(&mut self) -> Option<Screen> { self.vt.as_mut()?.viewport().ok() }

    pub fn encode_key(&mut self, input: &KeyInput) -> Vec<u8> {
        self.vt.as_mut().map(|vt| vt.encode_key(input)).unwrap_or_default()
    }

    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        self.vt.as_ref().map_or_else(|| text.as_bytes().to_vec(), |vt| vt.encode_paste(text))
    }

    pub fn encode_mouse(&mut self, input: &MouseInput) -> Option<Vec<u8>> { self.vt.as_mut()?.encode_mouse(input) }

    /// The bytes that run `command` from the composer (pasted, then Enter, as GPUiX sent it),
    /// or `None` when the shell is not at its prompt.
    pub fn submission(&mut self, command: &str) -> Option<Vec<u8>> {
        if !self.prepare(command) { return None; }
        let mut bytes = self.encode_paste(command);
        bytes.extend(self.encode_key(&KeyInput { key: "enter", text: "", shift: false, ctrl: false, alt: false, logo: false }));
        Some(bytes)
    }

    /// Stage `command` from the composer. False while a command is still running or the shell
    /// has not reached its input prompt.
    fn prepare(&mut self, command: &str) -> bool {
        if !self.ready || self.pending || self.running.is_some() || command.trim().is_empty() { return false; }
        self.finish_background();
        self.submitted = command.to_owned();
        self.pending = true;
        self.ready = false;
        true
    }

    /// Re-read the live block once per frame, not per PTY packet. Returns whether it changed.
    pub fn sync(&mut self) -> bool {
        if !std::mem::take(&mut self.dirty) { return false; }
        self.refresh_live();
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
        self.dirty = true;
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
            cursor_row: 0, cursor: None, stale: false,
        })
    }

    /// Bring a live block's rows up to date from Ghostty's grid (never from a viewport copy).
    /// Rows above `LIVE_REREAD_ROWS` before the last cursor row are kept from the previous read.
    fn capture(vt: &Vt, block: &mut Live) {
        let end = vt.position();
        if end.alt { return; }
        // Where the block starts now (scrollback pruning and reflow move it). A mark Ghostty
        // can no longer resolve (pruned, reset) keeps the rows already read.
        let Some(first) = vt.mark_row(block.mark) else { return };
        let keep = if block.stale { 0 } else { block.cursor_row.saturating_sub(LIVE_REREAD_ROWS).min(block.lines.len()) };
        block.stale = false;
        let Ok(mut rows) = vt.capture(first + keep as u64, end.row + 1) else { return };
        // B usually follows visible prompt ink on the same row; never archive that prefix.
        if keep == 0 && block.background && block.start.col > 0 && first == block.start.row {
            if let Some(row) = rows.first_mut() { *row = after_column(row, block.start.col as usize); }
        }
        block.lines.truncate(keep);
        block.lines.extend(rows);
        block.cursor_row = (end.row.saturating_sub(first)) as usize;
        block.cursor = Some((block.cursor_row, end.col));
    }

    fn refresh_live(&mut self) {
        let Some(vt) = &self.vt else { return };
        let active = if self.running.is_some() { &mut self.running } else { &mut self.background };
        self.live = active.as_mut().and_then(|block| {
            Self::capture(vt, block);
            let visible = !block.background || block.lines.iter().any(|r| ghostty::row_visible(r));
            visible.then(|| Block {
                id: block.id, command: block.command.clone(), cwd: block.cwd.clone(), started: block.started,
                elapsed_ms: None, exit: None, lines: block.lines.clone(), background: block.background, cursor: block.cursor,
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
                elapsed_ms: Some(block.started.elapsed().as_millis()), exit, lines, background: block.background, cursor: None,
            });
        }
        self.live = None;
        self.dirty = true;
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
                        running.cursor_row = 0;
                        running.stale = true;
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
                        let first = vt.mark_row(mark);
                        if let Some(Ok(mut rows)) = first.map(|first| vt.capture(first, start.row + u64::from(start.col > 0))) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut t = Shell::new("/tmp".into(), Colors { palette, fg: 0xffffff, bg: 0 });
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

    fn shell() -> Shell {
        let mut palette = [0x101010; 16];
        palette[4] = 0x0000ff;
        Shell::new("/tmp".into(), Colors { palette, fg: 0xffffff, bg: 0 })
    }

    fn key(name: &str) -> KeyInput<'_> {
        KeyInput { key: name, text: "", shift: false, ctrl: false, alt: false, logo: false }
    }

    #[test]
    fn full_screen_programs_read_through_the_viewport() {
        let mut t = shell();
        t.resize(20, 4, (9, 20));
        // vim-like: alternate screen, app cursor keys, a blue status row, cursor at (2, 1).
        t.feed(b"\x1b[?1049h\x1b[?1h\x1b[H\x1b[44mstatus\x1b[0m\r\nhello\x1b[2;3H", None);
        assert!(t.alt() && t.app_cursor());
        let screen = t.screen().unwrap();
        assert_eq!(screen.rows.len(), 4);
        assert_eq!(ghostty::row_text(&screen.rows[1]), "hello");
        assert_eq!(screen.rows[0][0].bg, Some(0x0000ff));
        let cursor = screen.cursor.unwrap();
        assert_eq!((cursor.col, cursor.row), (2, 1));
        // Application cursor mode turns Up into SS3 A; Ctrl+C is ETX either way.
        assert_eq!(t.encode_key(&key("up")), b"\x1bOA");
        assert_eq!(t.encode_key(&KeyInput { ctrl: true, ..key("c") }), b"\x03");
        t.feed(b"\x1b[?1h\x1b[?1049l\x1b[?1l", None);
        assert!(!t.alt());
        assert_eq!(t.encode_key(&key("up")), b"\x1b[A");
    }

    #[test]
    fn paste_is_bracketed_when_asked() {
        let mut t = shell();
        assert_eq!(t.encode_paste("ls"), b"ls");
        t.feed(b"\x1b[?2004h", None);
        assert_eq!(t.encode_paste("ls"), b"\x1b[200~ls\x1b[201~");
    }

    #[test]
    fn live_blocks_keep_their_tail_current_across_many_rows() {
        let mut t = shell();
        t.resize(40, 10, (9, 20));
        t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07", None);
        let bytes = t.submission("seq 300").unwrap();
        assert!(bytes.ends_with(b"\r"));
        let mut output = b"seq 300\r\n\x1b]133;C\x07".to_vec();
        for chunk in (1..=300).collect::<Vec<_>>().chunks(50) {
            for n in chunk { output.extend(format!("{n}\r\n").bytes()); }
            t.feed(&output, None);
            output.clear();
            t.sync();
        }
        let live = t.live.clone().unwrap();
        let text: Vec<String> = live.lines.iter().map(|r| ghostty::row_text(r)).collect();
        assert_eq!(text.first().map(String::as_str), Some("1"));
        assert_eq!(text.iter().rev().find(|l| !l.is_empty()).map(String::as_str), Some("300"));
        assert_eq!(text.iter().filter(|l| !l.is_empty()).count(), 300);
        t.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07", None);
        assert_eq!(t.blocks.last().unwrap().lines.len(), 300);
    }
}
