//! Canvas editor pointer selection policy, shared by native hosts.
use super::CodePane;

/// The desktop double-click contract: select an identifier run, otherwise the
/// single Unicode scalar under the pointer. Columns are UTF-8 byte offsets.
pub fn word_range_at(line: &str, col: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if line.is_empty() {
        return (0, 0);
    }
    let mut start = col.min(line.len().saturating_sub(1));
    while start > 0 && !line.is_char_boundary(start) {
        start -= 1;
    }
    let ch = line[start..].chars().next().unwrap_or(' ');
    if !is_word(ch) {
        return (start, start + ch.len_utf8());
    }
    let mut begin = start;
    while begin > 0 {
        let prev = line[..begin].chars().next_back().unwrap_or(' ');
        if !is_word(prev) {
            break;
        }
        begin -= prev.len_utf8();
    }
    let mut end = start;
    for c in line[start..].chars() {
        if !is_word(c) {
            break;
        }
        end += c.len_utf8();
    }
    (begin, end)
}

impl CodePane {
    pub fn begin_pointer_selection(
        &mut self,
        line: usize,
        col: usize,
        clicks: usize,
        extend: bool,
    ) {
        let line = line.min(self.buffer.lines.len().saturating_sub(1));
        match clicks {
            2 => {
                let (start, end) = word_range_at(&self.buffer.lines[line], col);
                self.set_pointer_position(line, start, false);
                self.set_pointer_position(line, end, true);
            }
            n if n >= 3 => {
                let end = self.buffer.lines[line].len();
                self.set_pointer_position(line, 0, false);
                self.set_pointer_position(line, end, true);
            }
            _ => self.set_pointer_position(line, col, extend),
        }
        self.mouse_selecting = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn double_click_keeps_identifiers_and_utf8_boundaries() {
        assert_eq!(word_range_at("foo_bar(x)", 4), (0, 7));
        assert_eq!(word_range_at("foo_bar(x)", 7), (7, 8));
        assert_eq!(word_range_at("éclair", 1), (0, 7));
        assert_eq!(word_range_at("👩🏽‍💻", 2), (0, 4));
        assert_eq!(word_range_at("", 4), (0, 0));
    }
}
