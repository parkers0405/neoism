//! Shared terminal hint policy.
//!
//! Desktop and web still own terminal grid access and hint side effects.
//! This module owns portable hint selection behavior: label generation and
//! filtering the labels visible after the keys typed so far.

use crate::editor::selection_model::post_process_hyperlink_uri;
use neoism_terminal_core::crosswords::grid::Dimensions;
use neoism_terminal_core::crosswords::pos::{Column, Line, Pos};
use neoism_terminal_core::crosswords::Crosswords;

/// Default hint-label alphabet — mirrors
/// `neoism_backend::config::hints::DEFAULT_HINTS_ALPHABET` so hosts
/// without the backend crate (web) label matches identically.
pub const DEFAULT_HINT_ALPHABET: &str = "jfkdls;ahgurieowpq";

/// Generates hint labels using the same least-significant-index first counter
/// as the desktop terminal hint mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HintLabelGenerator {
    alphabet: Vec<char>,
    indices: Vec<usize>,
}

impl HintLabelGenerator {
    pub fn new(alphabet: &str) -> Self {
        Self {
            alphabet: alphabet.chars().collect(),
            indices: vec![0],
        }
    }

    pub fn next_label(&mut self) -> Option<Vec<char>> {
        if self.alphabet.is_empty() {
            return None;
        }

        let label = self.current_label();
        self.increment();
        Some(label)
    }

    fn current_label(&self) -> Vec<char> {
        self.indices
            .iter()
            .rev()
            .map(|&i| self.alphabet[i])
            .collect()
    }

    fn increment(&mut self) {
        let mut carry = true;
        let mut pos = 0;

        while carry && pos < self.indices.len() {
            self.indices[pos] += 1;
            if self.indices[pos] >= self.alphabet.len() {
                self.indices[pos] = 0;
                pos += 1;
            } else {
                carry = false;
            }
        }

        if carry {
            self.indices.push(0);
        }
    }
}

pub fn generate_hint_labels(alphabet: &str, count: usize) -> Vec<Vec<char>> {
    let mut generator = HintLabelGenerator::new(alphabet);
    (0..count).filter_map(|_| generator.next_label()).collect()
}

pub fn visible_hint_labels(
    labels: &[Vec<char>],
    keys_pressed: &[char],
) -> Vec<(usize, Vec<char>)> {
    let keys_len = keys_pressed.len();
    labels
        .iter()
        .enumerate()
        .filter_map(|(index, label)| {
            if label.len() >= keys_len && label[..keys_len] == keys_pressed[..] {
                Some((index, label[keys_len..].to_vec()))
            } else {
                None
            }
        })
        .collect()
}

pub fn sort_dedup_hint_matches_by_start<T>(
    matches: &mut Vec<T>,
    start: impl Fn(&T) -> Pos + Copy,
) {
    matches.sort_by_key(|hint_match| {
        let pos = start(hint_match);
        (pos.row, pos.col)
    });
    matches.dedup_by_key(|hint_match| start(hint_match));
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileLinkToken {
    pub col_start: usize,
    pub col_end: usize,
    pub text: String,
}

/// Classify the terminal row token under `col` as a possible local file link.
///
/// This is intentionally filesystem-free: hosts still resolve the returned
/// text against their current working directory and decide how to open it.
pub fn terminal_file_link_token_at(row_text: &str, col: usize) -> Option<FileLinkToken> {
    let chars: Vec<char> = row_text.chars().collect();
    if col >= chars.len() || is_file_link_delimiter(chars[col]) {
        return None;
    }

    let mut start = col;
    while start > 0 && !is_file_link_delimiter(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && !is_file_link_delimiter(chars[end]) {
        end += 1;
    }

    let text: String = chars[start..end].iter().collect();
    let text = text
        .trim_end_matches(|c: char| matches!(c, ':' | '.' | ','))
        .to_string();
    if text.is_empty() {
        return None;
    }

    Some(FileLinkToken {
        col_start: start,
        col_end: end,
        text,
    })
}

#[inline]
fn is_file_link_delimiter(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '\'' | '"' | '(' | ')' | '[' | ']' | '<' | '>' | '|' | '*' | '?' | ';' | ','
        )
}

/// Split a grep/compiler-style `path:LINE[:COL]` suffix off a link
/// token. Returns the path portion and the parsed line number (1-based,
/// as printed). `src/main.rs:42:7` → (`src/main.rs`, Some(42));
/// `src/main.rs` → (`src/main.rs`, None). A single trailing numeric
/// group is treated as the line.
pub fn split_file_link_line_suffix(text: &str) -> (&str, Option<u32>) {
    let strip_numeric_suffix = |value: &str| -> Option<usize> {
        let (head, tail) = value.rsplit_once(':')?;
        if head.is_empty() || tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        Some(head.len())
    };

    let Some(first_cut) = strip_numeric_suffix(text) else {
        return (text, None);
    };
    // Two numeric groups → path:line:col; one group → path:line.
    if let Some(second_cut) = strip_numeric_suffix(&text[..first_cut]) {
        let line = text[second_cut + 1..first_cut].parse::<u32>().ok();
        (&text[..second_cut], line)
    } else {
        let line = text[first_cut + 1..].parse::<u32>().ok();
        (&text[..first_cut], line)
    }
}

/// A web (HTTP/markdown) link found in a soft-wrapped terminal row,
/// expressed in columns of the physical row under the pointer. Shared
/// lift of desktop `terminal::file_link::detect_web_in_wrapped_row`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebRowLink {
    /// Column range within the pointed physical row.
    pub col_start: usize,
    pub col_end: usize,
    pub url: String,
}

/// Detect a markdown/bare web link in a joined soft-wrapped logical
/// line, returning only the part occupying the physical row currently
/// under the pointer. `row_text` is the joined logical line, `col` the
/// pointed char offset within it, `physical_row_start`/`physical_row_len`
/// the char range that physical row occupies inside `row_text`.
pub fn detect_web_link_in_wrapped_row(
    row_text: &str,
    col: usize,
    physical_row_start: usize,
    physical_row_len: usize,
) -> Option<WebRowLink> {
    let span = crate::widgets::markdown::web_link_at(row_text, col)?;
    let logical_start = row_text[..span.raw_start].chars().count();
    let logical_end =
        logical_start + row_text[span.raw_start..span.raw_end].chars().count();
    let physical_row_end = physical_row_start + physical_row_len;
    let segment_start = logical_start.max(physical_row_start);
    let segment_end = logical_end.min(physical_row_end);
    if segment_start >= segment_end {
        return None;
    }
    Some(WebRowLink {
        col_start: segment_start - physical_row_start,
        col_end: segment_end - physical_row_start,
        url: span.target,
    })
}

// ---------------------------------------------------------------------------
// Host-seeded link existence cache (web).
//
// On wasm `std::fs` never answers, so the desktop file-link existence
// check (`FileLink` only fires when the token resolves on disk) is
// replaced by daemon-seeded directory listings: the host drains
// requested parent directories via [`drain_link_dir_requests`], lists
// them through the daemon, and seeds the result back through
// [`seed_link_dir_listing`]. Until a listing lands the probe answers
// `Unknown` and the hover/click stays inert — the same fail-closed
// behavior desktop gets from a failed `stat`.
// ---------------------------------------------------------------------------

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

thread_local! {
    static LINK_DIR_CACHE: RefCell<HashMap<PathBuf, Vec<(String, bool)>>> =
        RefCell::new(HashMap::new());
    static LINK_DIR_REQUESTS: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    /// Dirs already handed to the host once — never re-request so an
    /// out-of-workspace dir the host cannot list doesn't loop forever.
    static LINK_DIR_REQUESTED: RefCell<HashSet<PathBuf>> = RefCell::new(HashSet::new());
}

/// Store a host-resolved directory listing (`(name, is_dir)` pairs)
/// for terminal link existence checks.
pub fn seed_link_dir_listing(dir: PathBuf, entries: Vec<(String, bool)>) {
    let dir = normalize_link_path(&dir);
    LINK_DIR_CACHE.with(|cache| {
        cache.borrow_mut().insert(dir, entries);
    });
}

/// Drain parent directories a link probe wanted but had no listing
/// for. The host lists each through the daemon and seeds it back.
pub fn drain_link_dir_requests() -> Vec<PathBuf> {
    LINK_DIR_REQUESTS.with(|requests| std::mem::take(&mut *requests.borrow_mut()))
}

/// Answer of [`link_path_existence`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkPathExistence {
    Exists {
        is_dir: bool,
    },
    Missing,
    /// The parent directory has no cached listing yet; a request was
    /// queued (once per dir) for the host to satisfy.
    Unknown,
}

/// Lexically normalize `.` / `..` segments so cache keys line up with
/// the absolute dir strings the host seeds.
pub fn normalize_link_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Check an absolute candidate path against the host-seeded listings.
/// Queues a listing request for the parent dir when unknown.
pub fn link_path_existence(path: &Path) -> LinkPathExistence {
    let normalized = normalize_link_path(path);
    let Some(parent) = normalized.parent().map(Path::to_path_buf) else {
        return LinkPathExistence::Unknown;
    };
    let Some(name) = normalized
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    else {
        return LinkPathExistence::Unknown;
    };
    let cached = LINK_DIR_CACHE.with(|cache| cache.borrow().get(&parent).cloned());
    match cached {
        Some(entries) => entries
            .iter()
            .find(|(entry_name, _)| *entry_name == name)
            .map(|(_, is_dir)| LinkPathExistence::Exists { is_dir: *is_dir })
            .unwrap_or(LinkPathExistence::Missing),
        None => {
            let first_request = LINK_DIR_REQUESTED
                .with(|requested| requested.borrow_mut().insert(parent.clone()));
            if first_request {
                LINK_DIR_REQUESTS.with(|requests| requests.borrow_mut().push(parent));
            }
            LinkPathExistence::Unknown
        }
    }
}

// ---------------------------------------------------------------------------
// Regex-free hint scan for hosts without an oniguruma engine (web).
// ---------------------------------------------------------------------------

/// URI schemes recognized as URL-shaped hint text (mirrors the scheme
/// branch of the desktop `DEFAULT_URL_REGEX`).
pub const HINT_URL_SCHEMES: &[&str] = &[
    "https://",
    "http://",
    "mailto:",
    "ftp://",
    "file:",
    "ssh://",
    "ssh:",
    "git://",
    "tel:",
    "magnet:",
    "ipfs://",
    "ipns://",
    "gemini://",
    "gopher://",
    "news:",
];

/// Whether hint/link text should be treated as a URL for opening.
pub fn hint_text_is_url(text: &str) -> bool {
    HINT_URL_SCHEMES
        .iter()
        .any(|scheme| text.starts_with(scheme))
}

/// Approximate the desktop hint regex (`DEFAULT_URL_REGEX`) without a
/// regex engine: byte spans of web links (markdown `[label](url)` and
/// bare URLs) plus path-shaped tokens (`/abs`, `./rel`, `../rel`,
/// `~/x`, `$VAR/x`, `word/name.ext`). Used as the `regex_finder` the
/// shared [`crate::hint_state::HintState`] takes on hosts where onig
/// doesn't compile (wasm).
pub fn web_hint_line_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();

    // Web links first — the markdown scanner already handles balanced
    // parens and trailing punctuation. For `[label](url)` spans, label
    // the inner URL (what the desktop regex would match / open).
    for span in crate::widgets::markdown::web_link_spans(line) {
        let raw = &line[span.raw_start..span.raw_end];
        let (start, end) = match raw.rfind(span.target.as_str()) {
            Some(offset) => (
                span.raw_start + offset,
                span.raw_start + offset + span.target.len(),
            ),
            None => (span.raw_start, span.raw_end),
        };
        spans.push((start, end));
    }

    // Path-shaped tokens. Tokens split on the shared delimiter set so
    // hint spans line up with the hover/click tokenizer.
    let mut token_start: Option<usize> = None;
    let push_token = |spans: &mut Vec<(usize, usize)>, start: usize, end: usize| {
        let token = &line[start..end];
        let trimmed = token.trim_end_matches(|c: char| matches!(c, ':' | '.' | ','));
        if trimmed.is_empty() {
            return;
        }
        let end = start + trimmed.len();
        if hint_text_is_url(trimmed) || trimmed.contains("://") {
            return; // already covered by the web-link scan
        }
        if spans.iter().any(|(s, e)| start < *e && end > *s) {
            return; // overlaps a web-link span
        }
        let (path_part, _) = split_file_link_line_suffix(trimmed);
        let explicit_prefix = path_part.starts_with('/')
            || path_part.starts_with("./")
            || path_part.starts_with("../")
            || path_part.starts_with("~/")
            || (path_part.starts_with('$') && path_part.contains('/'));
        let bare_relative_with_ext = path_part.contains('/')
            && path_part
                .rsplit('/')
                .next()
                .is_some_and(|segment| segment.contains('.') && !segment.ends_with('.'));
        if explicit_prefix || bare_relative_with_ext {
            spans.push((start, end));
        }
    };
    for (idx, ch) in line.char_indices() {
        if is_file_link_delimiter(ch) {
            if let Some(start) = token_start.take() {
                push_token(&mut spans, start, idx);
            }
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }
    if let Some(start) = token_start {
        push_token(&mut spans, start, line.len());
    }

    spans.sort_by_key(|(start, _)| *start);
    spans
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HintTextMatch {
    pub text: String,
    pub start: Pos,
    pub end: Pos,
}

pub fn visible_hyperlink_hint_matches(
    terminal: &Crosswords,
    post_process: bool,
) -> Vec<HintTextMatch> {
    let grid = &terminal.grid;
    let display_offset = grid.display_offset();
    let visible_lines = grid.screen_lines();
    let mut matches = Vec::new();

    for line_idx in 0..visible_lines {
        let line = Line(line_idx as i32 - display_offset as i32);
        if line < Line(0) || line.0 >= grid.total_lines() as i32 {
            continue;
        }

        let mut col = 0usize;
        let cols = grid.columns();
        while col < cols {
            let id = match terminal.cell_hyperlink_id(line, Column(col)) {
                Some(id) => id,
                None => {
                    col += 1;
                    continue;
                }
            };

            let start_col = col;
            let mut end_col = col;
            while end_col < cols
                && terminal.cell_hyperlink_id(line, Column(end_col)) == Some(id)
            {
                end_col += 1;
            }

            if let Some(hyperlink) = terminal.cell_hyperlink(line, Column(start_col)) {
                let text = if post_process {
                    post_process_hyperlink_uri(hyperlink.uri())
                } else {
                    hyperlink.uri().to_string()
                };
                matches.push(HintTextMatch {
                    text,
                    start: Pos::new(line, Column(start_col)),
                    end: Pos::new(line, Column(end_col - 1)),
                });
            }

            col = end_col;
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_terminal_core::ansi::CursorShape;
    use neoism_terminal_core::crosswords::pos::{Column, Line};
    use neoism_terminal_core::crosswords::CrosswordsSize;
    use neoism_terminal_core::handler::{Processor, StdSyncHandler};

    fn pos(row: i32, col: usize) -> Pos {
        Pos::new(Line(row), Column(col))
    }

    fn terminal_with_osc8(bytes: &[u8]) -> Crosswords {
        let mut terminal = Crosswords::new(
            CrosswordsSize::new(40, 5),
            CursorShape::Block,
            neoism_terminal_core::TerminalId::new(0),
            10_000,
        );
        let mut processor = Processor::<StdSyncHandler>::new();
        processor.advance(&mut terminal, bytes);
        terminal
    }

    #[test]
    fn label_generator_matches_desktop_sequence() {
        let mut gen = HintLabelGenerator::new("abc");

        assert_eq!(gen.next_label(), Some(vec!['a']));
        assert_eq!(gen.next_label(), Some(vec!['b']));
        assert_eq!(gen.next_label(), Some(vec!['c']));
        assert_eq!(gen.next_label(), Some(vec!['a', 'a']));
        assert_eq!(gen.next_label(), Some(vec!['a', 'b']));
        assert_eq!(gen.next_label(), Some(vec!['a', 'c']));
        assert_eq!(gen.next_label(), Some(vec!['b', 'a']));
    }

    #[test]
    fn generate_hint_labels_limits_to_requested_count() {
        assert_eq!(
            generate_hint_labels("ab", 5),
            vec![
                vec!['a'],
                vec!['b'],
                vec!['a', 'a'],
                vec!['a', 'b'],
                vec!['b', 'a'],
            ]
        );
        assert!(generate_hint_labels("ab", 0).is_empty());
        assert!(generate_hint_labels("", 4).is_empty());
    }

    #[test]
    fn visible_hint_labels_filters_prefix_and_strips_entered_keys() {
        let labels = vec![
            vec!['a'],
            vec!['b'],
            vec!['a', 'b'],
            vec!['a', 'c'],
            vec!['b', 'a'],
        ];

        assert_eq!(
            visible_hint_labels(&labels, &[]),
            vec![
                (0, vec!['a']),
                (1, vec!['b']),
                (2, vec!['a', 'b']),
                (3, vec!['a', 'c']),
                (4, vec!['b', 'a']),
            ]
        );
        assert_eq!(
            visible_hint_labels(&labels, &['a']),
            vec![(0, vec![]), (2, vec!['b']), (3, vec!['c'])]
        );
        assert_eq!(visible_hint_labels(&labels, &['z']), Vec::new());
    }

    #[test]
    fn sort_dedup_hint_matches_orders_by_start_and_keeps_first_duplicate() {
        #[derive(Debug, Eq, PartialEq)]
        struct Match {
            id: &'static str,
            start: Pos,
        }

        let mut matches = vec![
            Match {
                id: "line1-col8",
                start: Pos::new(Line(1), Column(8)),
            },
            Match {
                id: "line0-col7",
                start: Pos::new(Line(0), Column(7)),
            },
            Match {
                id: "line0-col7-duplicate",
                start: Pos::new(Line(0), Column(7)),
            },
            Match {
                id: "line0-col3",
                start: Pos::new(Line(0), Column(3)),
            },
        ];

        sort_dedup_hint_matches_by_start(&mut matches, |hint_match| hint_match.start);

        assert_eq!(
            matches
                .into_iter()
                .map(|hint_match| hint_match.id)
                .collect::<Vec<_>>(),
            vec!["line0-col3", "line0-col7", "line1-col8"]
        );
    }

    #[test]
    fn terminal_file_link_token_at_extracts_token_under_column() {
        assert_eq!(
            terminal_file_link_token_at("open src/main.rs now", 8),
            Some(FileLinkToken {
                col_start: 5,
                col_end: 16,
                text: "src/main.rs".to_string(),
            })
        );
    }

    #[test]
    fn terminal_file_link_token_at_trims_trailing_punctuation_for_resolution() {
        assert_eq!(
            terminal_file_link_token_at("error: src/main.rs: done", 12),
            Some(FileLinkToken {
                col_start: 7,
                col_end: 19,
                text: "src/main.rs".to_string(),
            })
        );
        assert_eq!(
            terminal_file_link_token_at("see ./notes.md.", 6).map(|token| (
                token.col_start,
                token.col_end,
                token.text
            )),
            Some((4, 15, "./notes.md".to_string()))
        );
    }

    #[test]
    fn terminal_file_link_token_at_rejects_delimiters_and_out_of_bounds() {
        assert_eq!(terminal_file_link_token_at("src/main.rs", 99), None);
        assert_eq!(terminal_file_link_token_at("src/main.rs other", 11), None);
        assert_eq!(terminal_file_link_token_at("(src/main.rs)", 0), None);
    }

    #[test]
    fn split_file_link_line_suffix_parses_line_and_col() {
        assert_eq!(
            split_file_link_line_suffix("src/main.rs:42"),
            ("src/main.rs", Some(42))
        );
        assert_eq!(
            split_file_link_line_suffix("src/main.rs:42:7"),
            ("src/main.rs", Some(42))
        );
        assert_eq!(
            split_file_link_line_suffix("src/main.rs"),
            ("src/main.rs", None)
        );
        assert_eq!(split_file_link_line_suffix("v1.2"), ("v1.2", None));
        assert_eq!(split_file_link_line_suffix(":42"), (":42", None));
    }

    #[test]
    fn detect_web_link_in_wrapped_row_clips_to_physical_row() {
        let logical = "[Search](https://example.com/jobs)";
        let middle = detect_web_link_in_wrapped_row(logical, 15, 12, 12).unwrap();
        assert_eq!(middle.col_start, 0);
        assert_eq!(middle.col_end, 12);
        assert_eq!(middle.url, "https://example.com/jobs");
    }

    #[test]
    fn link_path_existence_uses_seeded_listing_and_requests_once() {
        seed_link_dir_listing(
            PathBuf::from("/ws/src"),
            vec![("main.rs".to_string(), false), ("sub".to_string(), true)],
        );
        assert_eq!(
            link_path_existence(Path::new("/ws/src/main.rs")),
            LinkPathExistence::Exists { is_dir: false }
        );
        assert_eq!(
            link_path_existence(Path::new("/ws/./src/sub")),
            LinkPathExistence::Exists { is_dir: true }
        );
        assert_eq!(
            link_path_existence(Path::new("/ws/src/nope.rs")),
            LinkPathExistence::Missing
        );
        // Unknown dir queues exactly one request.
        assert_eq!(
            link_path_existence(Path::new("/elsewhere/x.rs")),
            LinkPathExistence::Unknown
        );
        assert_eq!(
            link_path_existence(Path::new("/elsewhere/y.rs")),
            LinkPathExistence::Unknown
        );
        let requests = drain_link_dir_requests();
        assert_eq!(requests, vec![PathBuf::from("/elsewhere")]);
        assert!(drain_link_dir_requests().is_empty());
    }

    #[test]
    fn web_hint_line_spans_finds_urls_and_paths() {
        let line = "see https://neoism.dev/docs and src/main.rs:42 or ./run.sh now";
        let spans = web_hint_line_spans(line);
        let texts: Vec<&str> = spans.iter().map(|(s, e)| &line[*s..*e]).collect();
        assert_eq!(
            texts,
            vec!["https://neoism.dev/docs", "src/main.rs:42", "./run.sh"]
        );
        // Plain words and mid-word slashes don't match.
        assert!(web_hint_line_spans("plain words only").is_empty());
    }

    #[test]
    fn visible_hyperlink_hint_matches_walks_osc8_spans() {
        let terminal = terminal_with_osc8(
            b"go \x1b]8;;https://example.com/path]\x07click\x1b]8;;\x07.",
        );

        assert_eq!(
            visible_hyperlink_hint_matches(&terminal, true),
            vec![HintTextMatch {
                text: "https://example.com/path".to_string(),
                start: pos(0, 3),
                end: pos(0, 7),
            }]
        );
        assert_eq!(
            visible_hyperlink_hint_matches(&terminal, false)[0].text,
            "https://example.com/path]"
        );
    }
}
