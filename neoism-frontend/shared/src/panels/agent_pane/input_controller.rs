//! Shared agent input editing policy.
//!
//! This module owns the pure text-buffer behavior used by the agent
//! composer: cursor movement, history traversal, paste normalization,
//! large-paste attachment decisions, and token insertion spacing. Hosts
//! still decide how to attach files/images and how to execute queued IO.

pub const LARGE_PASTE_CHARS: usize = 8000;
pub const LARGE_PASTE_BREAKS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInputBuffer {
    pub input: String,
    pub cursor_byte: usize,
    pub sent_history: Vec<String>,
    pub history_index: Option<usize>,
    pub history_draft: String,
}

impl AgentInputBuffer {
    pub fn new(
        input: String,
        cursor_byte: usize,
        sent_history: Vec<String>,
        history_index: Option<usize>,
        history_draft: String,
    ) -> Self {
        Self {
            cursor_byte: clamp_to_char_boundary(&input, cursor_byte),
            input,
            sent_history,
            history_index,
            history_draft,
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        let cursor = self.cursor_byte();
        self.input.insert_str(cursor, text);
        self.cursor_byte = cursor.saturating_add(text.len());
        self.history_index = None;
    }

    pub fn insert_token(&mut self, token: &str) {
        let cursor = self.cursor_byte();
        if cursor > 0
            && self.input[..cursor]
                .chars()
                .last()
                .is_some_and(|ch| !ch.is_whitespace())
        {
            self.insert_text(" ");
        }
        self.insert_text(token);
        if self
            .input
            .get(self.cursor_byte()..)
            .and_then(|rest| rest.chars().next())
            .is_none_or(|ch| !ch.is_whitespace())
        {
            self.input.insert(self.cursor_byte(), ' ');
            self.cursor_byte = self.cursor_byte().saturating_add(1);
        }
    }

    pub fn delete_char_before_cursor(&mut self) -> bool {
        if self.cursor_byte() == 0 {
            return false;
        }
        let prev = previous_char_boundary(&self.input, self.cursor_byte());
        self.input.replace_range(prev..self.cursor_byte(), "");
        self.cursor_byte = prev;
        true
    }

    /// Delete a whole composer token ending at the cursor in one
    /// backspace, like Claude Code's pills: a known attachment token
    /// (`[pasted 2 lines]`, file/skill chips — optionally followed by
    /// the single space `insert_token` added) or an `@mention` word.
    /// Returns the deleted token so the host can drop its attachment;
    /// `None` means nothing token-like precedes the cursor and the
    /// caller should fall back to a plain char delete.
    pub fn delete_token_before_cursor(&mut self, tokens: &[&str]) -> Option<String> {
        let cursor = self.cursor_byte();
        if cursor == 0 {
            return None;
        }
        let before = &self.input[..cursor];
        // The token may carry the single trailing space `insert_token` adds.
        let body = before.strip_suffix(' ').unwrap_or(before);
        let matched = tokens
            .iter()
            .filter(|token| !token.is_empty() && body.ends_with(**token))
            .max_by_key(|token| token.len());
        if let Some(token) = matched {
            let start = body.len() - token.len();
            let deleted = token.to_string();
            self.input.replace_range(start..cursor, "");
            self.cursor_byte = start;
            self.history_index = None;
            return Some(deleted);
        }
        // `@mention` words delete as a unit too — but only words that
        // START with `@`, so emails like `bob@host` still delete
        // char-by-char.
        if before.chars().last()?.is_whitespace() {
            return None;
        }
        let word_start = before
            .rfind(char::is_whitespace)
            .map(|index| index + before[index..].chars().next().map_or(1, char::len_utf8))
            .unwrap_or(0);
        let word = &before[word_start..];
        if word.len() > 1 && word.starts_with('@') {
            let deleted = word.to_string();
            self.input.replace_range(word_start..cursor, "");
            self.cursor_byte = word_start;
            self.history_index = None;
            return Some(deleted);
        }
        None
    }

    pub fn move_left(&mut self) {
        self.cursor_byte = previous_char_boundary(&self.input, self.cursor_byte());
    }

    pub fn move_right(&mut self) {
        self.cursor_byte = next_char_boundary(&self.input, self.cursor_byte());
    }

    pub fn move_home(&mut self) {
        let cursor = self.cursor_byte();
        self.cursor_byte = self.input[..cursor]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
    }

    pub fn move_end(&mut self) {
        let cursor = self.cursor_byte();
        self.cursor_byte = self.input[cursor..]
            .find('\n')
            .map(|relative| cursor + relative)
            .unwrap_or(self.input.len());
    }

    pub fn move_up_with_history(&mut self) {
        let (line, col) = cursor_line_col(&self.input, self.cursor_byte());
        if line > 0 {
            self.cursor_byte = byte_for_line_col(&self.input, line - 1, col);
        } else {
            self.history_previous();
        }
    }

    pub fn move_down_with_history(&mut self) {
        let (line, col) = cursor_line_col(&self.input, self.cursor_byte());
        let last_line = self.input.split('\n').count().saturating_sub(1);
        if line < last_line {
            self.cursor_byte = byte_for_line_col(&self.input, line + 1, col);
        } else {
            self.history_next();
        }
    }

    /// Move the cursor one VISUAL line up. `ranges` are the byte spans
    /// of the soft-wrapped rows the renderer laid out last frame (same
    /// wrap the caret is drawn with), so Up walks wrapped rows exactly
    /// as they appear on screen; history recall only fires when the
    /// cursor is already on the first visual row. Falls back to
    /// hard-newline movement when the ranges don't match the buffer
    /// (stale frame, never rendered).
    pub fn move_up_with_history_visual(&mut self, ranges: &[(usize, usize)]) {
        if !self.visual_ranges_valid(ranges) {
            return self.move_up_with_history();
        }
        match visual_range_index(ranges, self.cursor_byte()) {
            Some(0) => self.history_previous(),
            Some(ix) => {
                let col = visual_col(&self.input, ranges[ix], self.cursor_byte());
                self.cursor_byte = byte_for_visual_col(&self.input, ranges[ix - 1], col);
            }
            None => self.move_up_with_history(),
        }
    }

    /// Mirror of [`Self::move_up_with_history_visual`]: Down walks the
    /// soft-wrapped rows and only advances history from the last one.
    pub fn move_down_with_history_visual(&mut self, ranges: &[(usize, usize)]) {
        if !self.visual_ranges_valid(ranges) {
            return self.move_down_with_history();
        }
        match visual_range_index(ranges, self.cursor_byte()) {
            Some(ix) if ix + 1 < ranges.len() => {
                let col = visual_col(&self.input, ranges[ix], self.cursor_byte());
                self.cursor_byte = byte_for_visual_col(&self.input, ranges[ix + 1], col);
            }
            Some(_) => self.history_next(),
            None => self.move_down_with_history(),
        }
    }

    fn visual_ranges_valid(&self, ranges: &[(usize, usize)]) -> bool {
        !ranges.is_empty()
            && ranges.iter().all(|&(start, end)| {
                start <= end
                    && end <= self.input.len()
                    && self.input.is_char_boundary(start)
                    && self.input.is_char_boundary(end)
            })
    }

    pub fn history_previous(&mut self) {
        if self.sent_history.is_empty() {
            return;
        }
        let next = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_draft = self.input.clone();
                self.sent_history.len() - 1
            }
        };
        self.history_index = Some(next);
        self.input = self.sent_history[next].clone();
        self.cursor_byte = self.input.len();
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.sent_history.len() {
            let next = index + 1;
            self.history_index = Some(next);
            self.input = self.sent_history[next].clone();
        } else {
            self.history_index = None;
            self.input = std::mem::take(&mut self.history_draft);
        }
        self.cursor_byte = self.input.len();
    }

    pub fn cursor_byte(&self) -> usize {
        self.cursor_byte.min(self.input.len())
    }
}

pub fn normalize_paste(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}

pub fn paste_should_compact(text: &str) -> bool {
    if text.len() >= LARGE_PASTE_CHARS || text.contains('\n') {
        return true;
    }
    let mut breaks = 0usize;
    for ch in text.chars() {
        if ch != '\n' {
            continue;
        }
        breaks += 1;
        if breaks >= LARGE_PASTE_BREAKS {
            return true;
        }
    }
    false
}

pub fn paste_token(text: &str) -> String {
    let breaks = text.bytes().filter(|byte| *byte == b'\n').count();
    if breaks > 0 {
        let lines = breaks + 1;
        return format!(
            "[pasted {} {}]",
            format_count(lines as u64),
            if lines == 1 { "line" } else { "lines" }
        );
    }
    let chars = text.chars().count();
    format!(
        "[pasted {} {}]",
        format_count(chars as u64),
        if chars == 1 { "char" } else { "chars" }
    )
}

pub fn path_from_pasted_reference(raw: &str) -> Option<std::path::PathBuf> {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    if trimmed.is_empty() {
        return None;
    }
    let value = trimmed
        .strip_prefix("file://")
        .map(decode_percent_path)
        .unwrap_or_else(|| trimmed.to_string());
    Some(std::path::PathBuf::from(value))
}

pub fn decode_percent_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hi = hex_value(bytes[index + 1]);
            let lo = hex_value(bytes[index + 2]);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn mime_can_attach_from_paste(mime: &str) -> bool {
    mime.starts_with("image/") || mime == "application/pdf"
}

pub fn attachment_mime_can_inline(mime: &str) -> bool {
    mime.starts_with("image/") || mime == "application/pdf"
}

pub fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        _ => "png",
    }
}

pub fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" => "text/typescript",
        "rs" | "py" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php"
        | "swift" | "kt" | "kts" | "sh" | "bash" | "zsh" | "fish" | "sql" | "xml"
        | "txt" => "text/plain",
        _ => "text/plain",
    }
}

pub fn file_url(path: &std::path::Path) -> String {
    let absolute = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace(' ', "%20");
    format!("file://{absolute}")
}

pub fn previous_char_boundary(value: &str, byte: usize) -> usize {
    let byte = byte.min(value.len());
    value[..byte]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub fn next_char_boundary(value: &str, byte: usize) -> usize {
    let byte = byte.min(value.len());
    if byte >= value.len() {
        return value.len();
    }
    value[byte..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| byte + index)
        .unwrap_or(value.len())
}

pub fn cursor_line_col(value: &str, byte: usize) -> (usize, usize) {
    let byte = clamp_to_char_boundary(value, byte);
    let prefix = &value[..byte];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let col = prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count();
    (line, col)
}

/// Index of the visual row containing `cursor`. A cursor sitting on a
/// soft-wrap boundary (== end of row N == start of row N+1) belongs to
/// row N — the same resolution the renderer uses when it wraps the
/// prefix to place the caret.
fn visual_range_index(ranges: &[(usize, usize)], cursor: usize) -> Option<usize> {
    ranges
        .iter()
        .position(|&(start, end)| cursor >= start && cursor <= end)
}

fn visual_col(value: &str, range: (usize, usize), cursor: usize) -> usize {
    let cursor = cursor.clamp(range.0, range.1);
    value[range.0..cursor].chars().count()
}

fn byte_for_visual_col(value: &str, range: (usize, usize), col: usize) -> usize {
    let slice = &value[range.0..range.1];
    range.0
        + slice
            .char_indices()
            .nth(col)
            .map(|(index, _)| index)
            .unwrap_or(slice.len())
}

pub fn byte_for_line_col(value: &str, target_line: usize, target_col: usize) -> usize {
    let mut line_start = 0;
    let mut line = 0;
    for (index, ch) in value.char_indices() {
        if line == target_line {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    let slice = &value[line_start..];
    let line_end = slice.find('\n').unwrap_or(slice.len());
    let line_slice = &slice[..line_end];
    line_start
        + line_slice
            .char_indices()
            .nth(target_col)
            .map(|(index, _)| index)
            .unwrap_or(line_slice.len())
}

fn clamp_to_char_boundary(value: &str, byte: usize) -> usize {
    let mut byte = byte.min(value.len());
    while byte > 0 && !value.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn format_count(value: u64) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let first = s.len() % 3;
    for (ix, ch) in s.chars().enumerate() {
        if ix > 0 && (ix % 3) == first {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_insertion_adds_spacing_around_mentions() {
        let mut input = AgentInputBuffer::new(
            "hello".to_string(),
            5,
            Vec::new(),
            None,
            String::new(),
        );

        input.insert_token("@src/main.rs");

        assert_eq!(input.input, "hello @src/main.rs ");
        assert_eq!(input.cursor_byte, input.input.len());
    }

    #[test]
    fn cursor_vertical_motion_preserves_column_across_multibyte_text() {
        let mut input = AgentInputBuffer::new(
            "αβγ\nab\nxyz".to_string(),
            "αβγ".len(),
            Vec::new(),
            None,
            String::new(),
        );

        input.move_down_with_history();

        assert_eq!(input.cursor_byte, "αβγ\nab".len());
        input.move_down_with_history();
        assert_eq!(input.cursor_byte, "αβγ\nab\nxy".len());
    }

    #[test]
    fn visual_motion_walks_soft_wrapped_rows_before_history() {
        // "abcdef" soft-wrapped as ["abc", "def"] — no '\n' anywhere.
        let ranges = [(0, 3), (3, 6)];
        let mut input = AgentInputBuffer::new(
            "abcdef".to_string(),
            4,
            vec!["old".to_string()],
            None,
            String::new(),
        );

        input.move_up_with_history_visual(&ranges);
        assert_eq!(input.cursor_byte, 1, "row 1 col 1 -> row 0 col 1");
        assert_eq!(input.input, "abcdef", "no history recall mid-draft");

        input.move_down_with_history_visual(&ranges);
        assert_eq!(input.cursor_byte, 4, "row 0 col 1 -> row 1 col 1");

        input.move_up_with_history_visual(&ranges);
        input.move_up_with_history_visual(&ranges);
        assert_eq!(
            input.input, "old",
            "Up on the first visual row recalls history"
        );
    }

    #[test]
    fn visual_motion_boundary_cursor_belongs_to_the_earlier_row() {
        // Cursor at byte 3 == end of row 0 == start of row 1: the
        // renderer paints the caret at the end of row 0, so Down must
        // move into row 1 (not fall through to history).
        let ranges = [(0, 3), (3, 6)];
        let mut input = AgentInputBuffer::new(
            "abcdef".to_string(),
            3,
            vec!["old".to_string()],
            None,
            String::new(),
        );

        input.move_down_with_history_visual(&ranges);
        assert_eq!(input.input, "abcdef");
        assert_eq!(input.cursor_byte, 6, "row 0 end -> row 1 col 3 (clamped)");
    }

    #[test]
    fn visual_motion_falls_back_on_stale_ranges() {
        // Ranges that don't match the buffer (stale frame) must not
        // panic or misplace the cursor — hard-newline fallback runs.
        let ranges = [(0, 50)];
        let mut input = AgentInputBuffer::new(
            "ab\ncd".to_string(),
            4,
            Vec::new(),
            None,
            String::new(),
        );

        input.move_up_with_history_visual(&ranges);
        assert_eq!(input.cursor_byte, 1, "hard-newline movement used instead");
    }

    #[test]
    fn history_navigation_round_trips_through_draft() {
        let mut input = AgentInputBuffer::new(
            "draft".to_string(),
            5,
            vec!["first".to_string(), "second".to_string()],
            None,
            String::new(),
        );

        input.history_previous();
        assert_eq!(input.input, "second");
        input.history_previous();
        assert_eq!(input.input, "first");
        input.history_next();
        assert_eq!(input.input, "second");
        input.history_next();
        assert_eq!(input.input, "draft");
        assert_eq!(input.history_index, None);
    }

    #[test]
    fn paste_policy_normalizes_and_names_large_paste_attachment() {
        let normalized = normalize_paste("a\r\nb\rc");

        assert_eq!(normalized, "a\nb\nc");
        assert!(paste_should_compact(&normalized));
        assert_eq!(paste_token(&normalized), "[pasted 3 lines]");
    }

    #[test]
    fn attachment_policy_decodes_file_uri_and_classifies_mime() {
        let path = path_from_pasted_reference("file:///tmp/hello%20world.png").unwrap();

        assert_eq!(path.to_string_lossy(), "/tmp/hello world.png");
        assert_eq!(mime_for_path(&path), "image/png");
        assert!(mime_can_attach_from_paste(mime_for_path(&path)));
        assert!(attachment_mime_can_inline("application/pdf"));
        assert_eq!(extension_for_mime("image/jpeg"), "jpg");
    }

    #[test]
    fn backspace_deletes_attachment_token_atomically() {
        let input = "see [pasted 2 lines] ".to_string();
        let cursor = input.len();
        let mut buffer =
            AgentInputBuffer::new(input, cursor, Vec::new(), None, String::new());

        let deleted = buffer.delete_token_before_cursor(&["[pasted 2 lines]"]);

        assert_eq!(deleted.as_deref(), Some("[pasted 2 lines]"));
        assert_eq!(buffer.input, "see ");
        assert_eq!(buffer.cursor_byte, "see ".len());
    }

    #[test]
    fn backspace_deletes_file_mention_word_atomically() {
        let input = "open @src/main.rs".to_string();
        let cursor = input.len();
        let mut buffer =
            AgentInputBuffer::new(input, cursor, Vec::new(), None, String::new());

        let deleted = buffer.delete_token_before_cursor(&[]);

        assert_eq!(deleted.as_deref(), Some("@src/main.rs"));
        assert_eq!(buffer.input, "open ");
    }

    #[test]
    fn backspace_falls_through_for_plain_words_and_emails() {
        for text in ["plain word", "mail bob@host.io"] {
            let mut buffer = AgentInputBuffer::new(
                text.to_string(),
                text.len(),
                Vec::new(),
                None,
                String::new(),
            );
            assert_eq!(buffer.delete_token_before_cursor(&[]), None);
            assert_eq!(buffer.input, text);
        }
    }

    #[test]
    fn horizontal_motion_respects_utf8_boundaries() {
        let mut input =
            AgentInputBuffer::new("a🚀b".to_string(), 1, Vec::new(), None, String::new());

        input.move_right();
        assert_eq!(input.cursor_byte, "a🚀".len());
        input.move_left();
        assert_eq!(input.cursor_byte, 1);
        input.delete_char_before_cursor();
        assert_eq!(input.input, "🚀b");
        assert_eq!(input.cursor_byte, 0);
    }
}
