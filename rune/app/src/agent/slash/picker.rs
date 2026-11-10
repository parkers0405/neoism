//! The inline picker docked above the composer: kinds, rows, query filtering and keyboard
//! selection, with the original rules (`state/picker.rs`): every query word must appear in a
//! row's text; a matching section header keeps its whole section; `/` rows rank by how the
//! command matches; headers are never selectable.

use rune::TextEdit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The `/` menu; its query is the composer text itself.
    Slash,
    Usage,
    Model,
    Agent,
    Thinking,
    Mcp,
    McpActions,
    Connect,
    ConnectAccount,
    ConnectAccountActions,
    ConnectAuth,
    /// A single secret field (API key or pasted OAuth token); the query row is the input.
    ConnectSecret,
    ConnectLabel,
    ConnectConfirm,
    Session,
    Skill,
    Subagent,
    /// `/cd`: the session directory, home, and the daemon's fuzzy directory hits.
    Directory,
}

impl Kind {
    /// Pickers whose query row is an input rather than a filter.
    pub fn is_entry(self) -> bool { matches!(self, Kind::ConnectSecret | Kind::ConnectLabel) }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub title: String,
    pub description: String,
    pub footer: String,
    pub value: String,
    /// The group a row belongs to (a provider name for models); searched but not shown.
    pub section: String,
    pub header: bool,
    /// The active choice: an accent check on the right.
    pub current: bool,
    pub icon: Option<Icon>,
    pub status: Option<Status>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A provider mark from the logo atlas (`ui/ProviderLogo.tsx`).
    Provider(usize),
    Glyph(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Good,
    Warning,
    Error,
    Muted,
}

impl Row {
    pub fn new(title: impl Into<String>, description: impl Into<String>, footer: impl Into<String>, value: impl Into<String>) -> Row {
        Row { title: title.into(), description: description.into(), footer: footer.into(), value: value.into(), ..Row::default() }
    }

    pub fn header(title: impl Into<String>) -> Row {
        let title = title.into();
        Row { section: title.clone(), title, header: true, ..Row::default() }
    }

    pub fn section(mut self, section: impl Into<String>) -> Row { self.section = section.into(); self }
    pub fn current(mut self, current: bool) -> Row { self.current = current; self }
    pub fn icon(mut self, icon: Option<Icon>) -> Row { self.icon = icon; self }
    pub fn status(mut self, status: Option<Status>) -> Row { self.status = status; self }

    fn matches(&self, words: &[&str]) -> bool {
        let haystack = format!("{} {} {} {} {}", self.title, self.description, self.footer, self.value, self.section).to_lowercase();
        words.iter().all(|w| haystack.contains(w))
    }
}

/// A sign-in or long-running step shown in place of the header's search row.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub body: String,
    pub status: String,
    pub busy: bool,
    /// Keep the query/secret field under the status (paste-a-token flows).
    pub show_input: bool,
    pub url: Option<String>,
}

pub struct Picker {
    pub kind: Kind,
    pub title: String,
    pub placeholder: String,
    pub query: TextEdit,
    rows: Vec<Row>,
    filtered: Vec<Row>,
    pub selected: usize,
    /// First visible row of the scrolled window.
    pub top: usize,
    pub loading: bool,
    pub activity: Option<Activity>,
    /// Model picker: the provider whose models are listed (`None` = providers screen).
    pub provider: Option<String>,
    /// Model picker: the provider last opened, so Back returns to it.
    pub last_provider: Option<String>,
    /// Rows that fit above the composer (`row_limit_for_space`), as the view last measured.
    pub limit: std::cell::Cell<usize>,
}

pub const MAX_ROWS: usize = 8;

impl Picker {
    pub fn new(kind: Kind, title: impl Into<String>, rows: Vec<Row>) -> Picker {
        let mut query = TextEdit::new("");
        query.focused = kind != Kind::Slash;
        let mut picker = Picker {
            kind, title: title.into(), placeholder: "Search".into(), query, rows: Vec::new(), filtered: Vec::new(),
            selected: 0, top: 0, loading: false, activity: None, provider: None, last_provider: None,
            limit: std::cell::Cell::new(MAX_ROWS),
        };
        picker.replace(rows);
        picker
    }

    pub fn loading(kind: Kind, title: impl Into<String>) -> Picker {
        let mut picker = Picker::new(kind, title, Vec::new());
        picker.loading = true;
        picker
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Picker { self.placeholder = placeholder.into(); self }

    /// Select the row whose value is `value` (when present), else the first selectable.
    pub fn select_value(mut self, value: &str) -> Picker {
        if let Some(i) = self.filtered.iter().position(|r| !r.header && r.value == value) { self.selected = i; }
        self.reveal();
        self
    }

    pub fn rows(&self) -> &[Row] { &self.filtered }

    pub fn selected_row(&self) -> Option<&Row> { self.filtered.get(self.selected).filter(|r| !r.header) }

    /// New rows (a catalog arrived); the selection follows its value when it survives.
    pub fn replace(&mut self, rows: Vec<Row>) {
        let previous = self.selected_row().map(|r| r.value.clone());
        self.rows = rows;
        self.loading = false;
        self.refilter();
        if let Some(i) = previous.and_then(|v| self.filtered.iter().position(|r| !r.header && r.value == v)) { self.selected = i; }
        self.selected = selectable_near(&self.filtered, self.selected).unwrap_or(0);
        self.reveal();
    }

    /// Filter by `query` (for the `/` menu, the composer's text).
    pub fn set_query(&mut self, query: &str) {
        if self.kind.is_entry() { return; }
        let previous = (self.kind != Kind::Slash).then(|| self.selected_row().map(|r| r.value.clone())).flatten();
        if self.kind != Kind::Slash && self.query.text != query { self.query.replace_text(query); }
        self.refilter_with(query);
        self.selected = previous.and_then(|v| self.filtered.iter().position(|r| !r.header && r.value == v))
            .or_else(|| selectable_near(&self.filtered, 0)).unwrap_or(0);
        self.top = 0;
    }

    fn refilter(&mut self) {
        let query = if self.kind == Kind::Slash { String::new() } else { self.query.text.clone() };
        self.refilter_with(&query);
    }

    fn refilter_with(&mut self, query: &str) {
        let needle = query.trim().to_lowercase();
        // Directory rows are the daemon's own fuzzy hits for the query; never filter them again.
        if needle.is_empty() || self.kind.is_entry() || self.kind == Kind::Directory {
            self.filtered = self.rows.clone();
            return;
        }
        let words: Vec<&str> = needle.split_whitespace().collect();
        let mut out = Vec::new();
        let mut header: Option<&Row> = None;
        let (mut header_matches, mut emitted) = (false, false);
        for row in &self.rows {
            if row.header {
                header_matches = row.matches(&words);
                header = Some(row);
                emitted = false;
                continue;
            }
            if !(row.matches(&words) || header_matches) { continue; }
            if let (Some(h), false) = (header, emitted) {
                out.push(h.clone());
                emitted = true;
            }
            out.push(row.clone());
        }
        if self.kind == Kind::Slash { out.sort_by_key(|row| slash_rank(row, &needle)); }
        self.filtered = out;
    }

    /// Step over headers by `delta` selectable rows; stops at the ends (no wrap).
    pub fn step(&mut self, delta: isize) {
        let mut index = self.selected.min(self.filtered.len().saturating_sub(1));
        for _ in 0..delta.unsigned_abs().max(1) {
            let next = if delta > 0 {
                (index + 1..self.filtered.len()).find(|&i| !self.filtered[i].header)
            } else {
                (0..index).rev().find(|&i| !self.filtered[i].header)
            };
            match next { Some(i) => index = i, None => break }
        }
        self.selected = index;
        self.reveal();
    }

    fn window(&self) -> usize { self.limit.get().clamp(1, MAX_ROWS) }

    /// Keep the selection inside the visible window, showing a header just above it.
    fn reveal(&mut self) {
        let window = self.window();
        if self.selected < self.top { self.top = self.selected.saturating_sub(usize::from(self.selected > 0 && self.filtered[self.selected - 1].header)); }
        if self.selected >= self.top + window { self.top = self.selected + 1 - window; }
        self.top = self.top.min(self.filtered.len().saturating_sub(window));
    }

    /// Scroll the window by whole rows (wheel), keeping the selection visible.
    pub fn scroll(&mut self, rows: isize) {
        let window = self.window();
        let max = self.filtered.len().saturating_sub(window) as isize;
        self.top = (self.top as isize + rows).clamp(0, max) as usize;
        if self.selected < self.top || self.selected >= self.top + window {
            self.selected = selectable_near(&self.filtered, if rows > 0 { self.top } else { self.top + window - 1 }).unwrap_or(self.selected);
        }
    }
}

fn selectable_near(rows: &[Row], index: usize) -> Option<usize> {
    if rows.is_empty() { return None; }
    let index = index.min(rows.len() - 1);
    (0..rows.len()).find_map(|offset| {
        [index.checked_add(offset).filter(|&i| i < rows.len()), index.checked_sub(offset)]
            .into_iter().flatten().find(|&i| !rows[i].header)
    })
}

/// `/` rows: exact command, then prefix (shorter first), substring, then description,
/// footer and section matches.
fn slash_rank(row: &Row, query: &str) -> (u8, usize) {
    let query = query.trim().trim_start_matches('/');
    let command = row.value.trim().trim_start_matches('/').to_lowercase();
    if command == query { return (0, 0); }
    if command.starts_with(query) { return (1, command.len() - query.len()); }
    if let Some(i) = command.find(query) { return (2, i); }
    for (rank, text) in [(3, &row.description), (4, &row.footer), (5, &row.section)] {
        if let Some(i) = text.to_lowercase().find(query) { return (rank, i); }
    }
    (6, usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slash() -> Picker {
        Picker::new(Kind::Slash, "Commands", super::super::plan::COMMANDS.iter().map(|c| Row::new(c.title, c.description, c.footer, c.title)).collect())
    }

    #[test]
    fn slash_ranks_prefix_before_description() {
        let mut p = slash();
        p.set_query("/mo");
        assert_eq!(p.rows()[0].value, "/model");
        p.set_query("/s");
        assert_eq!(p.rows()[0].value, "/skills", "shortest prefix match first");
        p.set_query("/zzz");
        assert!(p.rows().is_empty());
    }

    #[test]
    fn headers_carry_their_section_and_are_skipped() {
        let rows = vec![
            Row::header("OpenAI"), Row::new("GPT-5", "", "", "openai/gpt-5"), Row::new("o3", "", "", "openai/o3"),
            Row::header("Anthropic"), Row::new("Opus", "", "", "anthropic/opus"),
        ];
        let mut p = Picker::new(Kind::Model, "Model", rows);
        assert_eq!(p.selected, 1, "first selectable");
        p.step(2);
        assert_eq!(p.selected_row().unwrap().value, "anthropic/opus", "steps over the header");
        p.step(5);
        assert_eq!(p.selected_row().unwrap().value, "anthropic/opus", "no wrap");
        p.set_query("openai");
        assert_eq!(p.rows().len(), 3, "header match keeps its section");
        p.set_query("opus");
        assert_eq!(p.rows().iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Anthropic", "Opus"]);
    }

    #[test]
    fn selection_survives_new_rows() {
        let mut p = Picker::new(Kind::Agent, "Agents", vec![Row::new("build", "", "", "build"), Row::new("plan", "", "", "plan")]).select_value("plan");
        p.replace(vec![Row::new("ask", "", "", "ask"), Row::new("build", "", "", "build"), Row::new("plan", "", "", "plan")]);
        assert_eq!(p.selected_row().unwrap().value, "plan");
    }

    #[test]
    fn window_follows_selection() {
        let rows = (0..20).map(|i| Row::new(format!("r{i}"), "", "", format!("{i}"))).collect();
        let mut p = Picker::new(Kind::Session, "Sessions", rows);
        p.step(10);
        assert_eq!((p.selected, p.top), (10, 3));
        p.step(-10);
        assert_eq!((p.selected, p.top), (0, 0));
    }
}
