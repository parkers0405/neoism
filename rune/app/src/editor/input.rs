//! Keys into a document: normal and visual keys go through the vim resolver and applier, insert
//! mode edits text, and `:` / `/` / `?` open the one-line command prompt at the editor's foot.

use super::document::{Document, Mode};
use super::vim::{VimKeyFeed, VimSearch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// What a key asks of the host beyond the document itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Handled,
    /// Not an editor key (window shortcuts may take it).
    Unhandled,
    /// An ex command was entered (`w`, `q`, `wq`, `42`, …), without the colon.
    Command(String),
    /// A yank or delete wrote the unnamed (or `+` / `*`) register: copy it to the clipboard.
    Yanked(String),
}

/// The command line under the editor: `:` for ex commands, `/` and `?` for search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prompt {
    pub kind: char,
    pub text: String,
}

impl Document {
    /// Feed one key. `clipboard` backs `p` when no register holds text (the host clipboard).
    pub fn key(&mut self, key: Key, prompt: &mut Option<Prompt>, clipboard: Option<&str>) -> Outcome {
        if let Some(open) = prompt.take() {
            let (keep, outcome) = self.prompt_key(key, open, clipboard);
            *prompt = keep;
            return outcome;
        }
        match self.mode {
            Mode::Insert => self.insert_key(key),
            Mode::Normal | Mode::Visual => self.normal_key(key, prompt, clipboard),
        }
    }

    /// Prompt editing: returns the prompt if it stays open, and the outcome.
    fn prompt_key(&mut self, key: Key, mut open: Prompt, clipboard: Option<&str>) -> (Option<Prompt>, Outcome) {
        match key {
            Key::Esc => (None, Outcome::Handled),
            Key::Backspace if open.text.is_empty() => (None, Outcome::Handled),
            Key::Backspace => { open.text.pop(); (Some(open), Outcome::Handled) }
            Key::Char(c) => { open.text.push(c); (Some(open), Outcome::Handled) }
            Key::Enter if open.kind == ':' => (None, Outcome::Command(open.text.trim().to_owned())),
            Key::Enter => {
                if !open.text.is_empty() {
                    self.vim.search = Some(VimSearch { pattern: open.text, forward: open.kind == '/', whole_word: false });
                    self.apply_vim_action(&super::vim::VimAction::Search { reverse: false, count: 1 }, clipboard);
                }
                (None, Outcome::Handled)
            }
            _ => (Some(open), Outcome::Handled),
        }
    }

    fn insert_key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Esc => self.leave_insert(),
            Key::Enter => self.type_text("\n"),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete_forward(),
            Key::Tab => self.insert_indent(),
            Key::Char(c) => self.type_text(c.encode_utf8(&mut [0; 4])),
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
                // Moving in insert mode ends the typing group, as vim's undo does.
                self.end_insert_group();
                let motion = match key { Key::Left => 'h', Key::Right => 'l', Key::Up => 'k', Key::Down => 'j', Key::Home => '0', _ => '$' };
                let feed = self.vim.feed(motion, false);
                if let VimKeyFeed::Action(action) = feed { self.apply_vim_action(&action, None); }
                if key == Key::End || key == Key::Right {
                    // Insert mode may sit past the last char.
                    if key == Key::End { self.cursor_col = self.lines[self.cursor_line].len(); }
                }
            }
            Key::Ctrl(_) => return Outcome::Unhandled,
        }
        Outcome::Handled
    }

    fn normal_key(&mut self, key: Key, prompt: &mut Option<Prompt>, clipboard: Option<&str>) -> Outcome {
        let visual = self.mode == Mode::Visual;
        let feed = match key {
            Key::Esc => {
                self.vim.clear_pending();
                if visual { self.mode = Mode::Normal; self.visual_anchor = None; self.vim.visual_linewise = false; self.vim.visual_block = false; }
                return Outcome::Handled;
            }
            Key::Char(':' | '/' | '?') if self.vim.pending.is_empty() => {
                let Key::Char(kind) = key else { unreachable!() };
                *prompt = Some(Prompt { kind, text: String::new() });
                return Outcome::Handled;
            }
            Key::Char(c) => self.vim.feed(c, visual),
            Key::Ctrl('d') | Key::Ctrl('u') => {
                let Key::Ctrl(c) = key else { unreachable!() };
                return self.half_page(c == 'd');
            }
            Key::Ctrl(c) => self.vim.feed_ctrl(c, visual),
            Key::Left => self.vim.feed('h', visual),
            Key::Right => self.vim.feed('l', visual),
            Key::Up => self.vim.feed('k', visual),
            Key::Down => self.vim.feed('j', visual),
            Key::Home => self.vim.feed('0', visual),
            Key::End => self.vim.feed('$', visual),
            Key::Enter => self.vim.feed('+', visual),
            Key::Backspace => self.vim.feed('h', visual),
            Key::Delete => self.vim.feed('x', visual),
            Key::Tab => return Outcome::Unhandled,
        };
        match feed {
            VimKeyFeed::Action(action) => {
                let applied = self.apply_vim_action(&action, clipboard);
                if let Some(keys) = applied.replay_keys {
                    // `@q`: replay the recorded keys through the same path.
                    self.vim.replaying_macro = true;
                    let mut last = Outcome::Handled;
                    for c in keys.chars() {
                        let outcome = self.key(Key::Char(c), prompt, clipboard);
                        if matches!(outcome, Outcome::Yanked(_)) { last = outcome; }
                    }
                    self.vim.replaying_macro = false;
                    return last;
                }
                // Unnamed, `+` and `*` writes go to the system clipboard; named registers don't.
                match applied.register {
                    Some(text) if applied.sync_clipboard => Outcome::Yanked(text),
                    _ => Outcome::Handled,
                }
            }
            VimKeyFeed::Pending | VimKeyFeed::Cancelled => Outcome::Handled,
            VimKeyFeed::Unhandled => Outcome::Unhandled,
        }
    }

    /// `Ctrl+d` / `Ctrl+u`: half a screen (the host sets `page`).
    fn half_page(&mut self, down: bool) -> Outcome {
        let rows = (self.page_rows / 2).max(1);
        let last = self.lines.len().saturating_sub(1);
        self.cursor_line = if down { (self.cursor_line + rows).min(last) } else { self.cursor_line.saturating_sub(rows) };
        self.cursor_col = super::vim::vim_first_non_blank(&self.lines[self.cursor_line]);
        self.clamp_cursor();
        self.follow_cursor = true;
        Outcome::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a vim key string; `<esc>` and `<cr>` are named keys.
    fn run(text: &str, keys: &str) -> Document {
        let mut doc = Document::from_text(text);
        let mut prompt = None;
        let mut rest = keys;
        while !rest.is_empty() {
            let (key, len) = if let Some(r) = rest.strip_prefix("<esc>") { (Key::Esc, rest.len() - r.len()) }
                else if let Some(r) = rest.strip_prefix("<cr>") { (Key::Enter, rest.len() - r.len()) }
                else if let Some(r) = rest.strip_prefix("<c-r>") { (Key::Ctrl('r'), rest.len() - r.len()) }
                else { let c = rest.chars().next().unwrap(); (Key::Char(c), c.len_utf8()) };
            doc.key(key, &mut prompt, None);
            rest = &rest[len..];
        }
        doc
    }

    #[test]
    fn delete_change_yank_paste() {
        assert_eq!(run("one two three", "dw").lines, ["two three"]);
        assert_eq!(run("one two three", "wcwTWO<esc>").lines, ["one TWO three"]);
        assert_eq!(run("a\nb\nc", "ddp").lines, ["b", "a", "c"]);
        assert_eq!(run("a\nb", "yyP").lines, ["a", "a", "b"]);
        assert_eq!(run("foo(bar, baz)", "fbciwX<esc>").lines, ["foo(X, baz)"]);
        assert_eq!(run("foo(bar, baz)", "fbdi(").lines, ["foo()"]);
        assert_eq!(run("abc", "xx").lines, ["c"]);
        assert_eq!(run("abc", "$x").lines, ["ab"]);
    }

    #[test]
    fn insert_kinds_and_undo() {
        assert_eq!(run("x", "ihi <esc>").lines, ["hi x"]);
        assert_eq!(run("x", "ay<esc>").lines, ["xy"]);
        assert_eq!(run("  x", "Iy<esc>").lines, ["  yx"]);
        assert_eq!(run("x", "onew<esc>").lines, ["x", "new"]);
        assert_eq!(run("x", "Onew<esc>").lines, ["new", "x"]);
        assert_eq!(run("one two", "dwu").lines, ["one two"]);
        assert_eq!(run("one two", "dwu<c-r>").lines, ["two"]);
        // Like the original code editor, `.` repeats operators, not typed inserts.
        assert_eq!(run("a", "ix<esc>.").lines, ["xa"]);
        assert_eq!(run("a b c", "dw.").lines, ["c"]);
    }

    #[test]
    fn visual_and_counts() {
        assert_eq!(run("one two three", "vwd").lines, ["wo three"]);
        assert_eq!(run("a\nb\nc", "Vjd").lines, ["c"]);
        assert_eq!(run("a b c d", "2dw").lines, ["c d"]);
        assert_eq!(run("a\nb\nc\nd", "3G").cursor_line, 2);
        assert_eq!(run("x\ny", ">>").lines[0], "    x");
    }

    #[test]
    fn prompt_commands_and_search() {
        let mut doc = Document::from_text("alpha\nbeta\ngamma");
        let mut prompt = None;
        doc.key(Key::Char(':'), &mut prompt, None);
        doc.key(Key::Char('w'), &mut prompt, None);
        assert_eq!(doc.key(Key::Enter, &mut prompt, None), Outcome::Command("w".into()));
        assert!(prompt.is_none());
        for k in [Key::Char('/'), Key::Char('g'), Key::Char('a'), Key::Enter] { doc.key(k, &mut prompt, None); }
        assert_eq!(doc.cursor_line, 2);
    }
}
