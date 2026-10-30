//! Read-only hosted review state. Drafts are local UI state, never source buffers.
use crate::{
    layout::Rect,
    primitives::{draw_text_with_occlusion, IdeTheme},
};
use neoism_protocol::pull_request::*;
use neoism_paint::text::DrawOpts;
use crate::primitives::paint::Paint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Conversation,
    Checks,
}
#[derive(Clone, Debug)]
pub enum Intent {
    Discard,
    Cancel,
    Tab(Tab),
    Field(usize),
    Draft,
    Browser(String),
    Refresh,
    Checkout(bool),
    Review,
    Event(PullRequestReviewEvent),
    Submit,
    Queue,
    Reply(String),
    Resolve(String, bool),
}

pub struct PullRequestView {
    pub detail: Option<PullRequestDetail>,
    /// Inbox supplies a persistent header; keep immutable code and review sheets.
    pub embedded: bool,
    pub branch: String,
    pub head: String,
    pub tab: Tab,
    pub fields: [String; 4], // title, body, base, review/reply text
    pub initial_base: String,
    pub draft: bool,
    pub comments: Vec<PullRequestInlineComment>,
    pub anchor: Option<PullRequestInlineComment>,
    pub stale: bool,
    pub pending: bool,
    pending_reply: bool,
    pub message: Option<String>,
    pub sheet: bool,
    pub event: PullRequestReviewEvent,
    pub reply_thread: Option<String>,
    pub focus: Option<usize>,
    cursor: usize,
    pub hits: Vec<(Rect, Intent)>,
    pub scroll: f32,
    pub max_scroll: f32,
}
/// Wrap even unbroken text using the actual host font metric. Preserve blank lines.
fn measured_lines(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    for c in text.chars() {
        if c == '\n' {
            rows.push(std::mem::take(&mut row));
            continue;
        }
        let mut next = row.clone();
        next.push(c);
        if !row.is_empty() && measure(&next) > width.max(1.0) {
            rows.push(std::mem::take(&mut row));
        }
        row.push(c);
    }
    rows.push(row);
    rows
}
#[cfg(test)]
mod pull_request_form_tests {
    use super::*;
    #[test]
    fn unbroken_unicode_text_wraps_and_blank_lines_survive() {
        assert_eq!(
            measured_lines("αβγδε\n\nx", 2.0, |s| s.chars().count() as f32),
            vec!["αβ", "γδ", "ε", "", "x"]
        );
    }
    #[test]
    fn zero_width_is_bounded() {
        assert_eq!(
            measured_lines("abc", 0.0, |s| s.len() as f32),
            vec!["a", "b", "c"]
        );
    }
}
#[cfg(test)]
#[path = "git_pull_request_visual_tests.rs"]
mod visual_tests;

// The same visible fragments constrain controls, their decoration, and their hit targets.
fn visible_regions(rect: Rect, clip: Rect, occlusions: &[[f32; 4]]) -> Vec<Rect> {
    let x = rect.x.max(clip.x);
    let y = rect.y.max(clip.y);
    let right = (rect.x + rect.w).min(clip.x + clip.w);
    let bottom = (rect.y + rect.h).min(clip.y + clip.h);
    if right <= x || bottom <= y {
        return Vec::new();
    }
    let mut visible = vec![Rect::new(x, y, right - x, bottom - y)];
    for cut in occlusions {
        let mut next = Vec::new();
        for r in visible {
            let l = r.x.max(cut[0]);
            let t = r.y.max(cut[1]);
            let rr = (r.x + r.w).min(cut[0] + cut[2]);
            let b = (r.y + r.h).min(cut[1] + cut[3]);
            if rr <= l || b <= t {
                next.push(r);
                continue;
            }
            for part in [
                Rect::new(r.x, r.y, r.w, t - r.y),
                Rect::new(r.x, b, r.w, r.y + r.h - b),
                Rect::new(r.x, t, l - r.x, b - t),
                Rect::new(rr, t, r.x + r.w - rr, b - t),
            ] {
                if part.w > 0.0 && part.h > 0.0 {
                    next.push(part);
                }
            }
        }
        visible = next;
    }
    visible
}
fn control_quad(
    sl: &mut dyn Paint,
    rect: Rect,
    clip: Rect,
    color: [f32; 4],
    occlusions: &[[f32; 4]],
) {
    for r in visible_regions(rect, clip, occlusions) {
        sl.quad(None, r.x, r.y, r.w, r.h, color, [0.0; 4], 0.0, 21);
    }
}
impl PullRequestView {
    pub fn create(branch: String, head: String, base: String) -> Self {
        Self {
            detail: None,
            embedded: false,
            branch,
            head,
            tab: Tab::Changes,
            initial_base: base.clone(),
            fields: [String::new(), String::new(), base, String::new()],
            draft: false,
            comments: Vec::new(),
            anchor: None,
            stale: false,
            pending: false,
            pending_reply: false,
            message: None,
            sheet: false,
            event: PullRequestReviewEvent::Comment,
            reply_thread: None,
            focus: Some(0),
            cursor: 0,
            hits: Vec::new(),
            scroll: 0.0,
            max_scroll: 0.0,
        }
    }
    pub(crate) fn coverage_warning(&self) -> Option<&'static str> {
        self.detail.as_ref().filter(|d| d.truncated).map(|_| {
            "Partial review · host limits reached. Open Browser for all changes."
        })
    }
    pub fn number(&self) -> Option<u64> {
        self.detail.as_ref().map(|d| d.summary.number)
    }
    pub fn has_drafts(&self) -> bool {
        (self.detail.is_none() && (self.draft || self.fields[2] != self.initial_base))
            || !self.comments.is_empty()
            || self.anchor.is_some()
            || self
                .fields
                .iter()
                .enumerate()
                .any(|(i, s)| (self.detail.is_none() && i < 2 || i == 3) && !s.is_empty())
    }
    pub fn focus_field(&mut self, i: usize) {
        self.focus = Some(i);
        self.cursor = self.fields[i].len();
    }
    pub fn insert_text(&mut self, text: &str) -> bool {
        let Some(i) = self.focus else {
            return false;
        };
        if self.pending {
            return true;
        }
        let text = if i == 0 || i == 2 {
            text.replace(['\n', '\r'], " ")
        } else {
            text.replace('\r', "")
        };
        self.cursor = self.cursor.min(self.fields[i].len());
        self.fields[i].insert_str(self.cursor, &text);
        self.cursor += text.len();
        true
    }
    pub fn key(&mut self, key: &str) -> bool {
        if key == "Escape" {
            self.focus = None;
            self.sheet = false;
            return true;
        }
        let Some(i) = self.focus else { return false };
        if self.pending {
            return true;
        }
        if key == "Tab" {
            self.focus_field(if self.detail.is_none() {
                (i + 1) % 3
            } else {
                3
            });
            return true;
        }
        let s = &mut self.fields[i];
        self.cursor = self.cursor.min(s.len());
        match key {
            "ArrowLeft" | "Left" => {
                self.cursor = s[..self.cursor]
                    .char_indices()
                    .last()
                    .map(|(n, _)| n)
                    .unwrap_or(0);
            }
            "ArrowRight" | "Right" => {
                self.cursor += s[self.cursor..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(0);
            }
            "Home" => {
                self.cursor = s[..self.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
            }
            "End" => {
                self.cursor +=
                    s[self.cursor..].find('\n').unwrap_or(s.len() - self.cursor)
            }
            "Backspace" => {
                if self.cursor > 0 {
                    let p = s[..self.cursor].char_indices().last().unwrap().0;
                    s.drain(p..self.cursor);
                    self.cursor = p;
                }
            }
            "Delete" => {
                if let Some(c) = s[self.cursor..].chars().next() {
                    s.drain(self.cursor..self.cursor + c.len_utf8());
                }
            }
            "Enter" if i == 1 || i == 3 => {
                s.insert(self.cursor, '\n');
                self.cursor += 1;
            }
            _ => return true,
        }
        true
    }
    pub fn request(&mut self) -> Option<PullRequestRequest> {
        if self.pending || self.stale {
            return None;
        }
        let request = if let Some(number) = self.number() {
            if let Some(id) = &self.reply_thread {
                if self.fields[3].trim().is_empty() {
                    return None;
                }
                PullRequestRequest::ReplyThread {
                    number,
                    thread_id: id.clone(),
                    body: self.fields[3].clone(),
                }
            } else {
                if self.anchor.is_some() {
                    return None;
                }
                PullRequestRequest::SubmitReview {
                    number,
                    expected_head: self.head.clone(),
                    body: self.fields[3].clone(),
                    event: self.event,
                    comments: self.comments.clone(),
                }
            }
        } else {
            if self.fields[0].trim().is_empty() || self.fields[2].trim().is_empty() {
                return None;
            }
            PullRequestRequest::Create {
                title: self.fields[0].clone(),
                body: self.fields[1].clone(),
                base: self.fields[2].clone(),
                draft: self.draft,
                expected_branch: self.branch.clone(),
                expected_head: self.head.clone(),
            }
        };
        self.pending_reply = matches!(&request, PullRequestRequest::ReplyThread { .. });
        self.pending = true;
        self.message = Some("Submitting…".into());
        Some(request)
    }
    pub fn queue(&mut self) {
        if self.pending {
            return;
        }
        if let Some(mut anchor) = self.anchor.take() {
            if self.fields[3].trim().is_empty() {
                self.anchor = Some(anchor);
                return;
            }
            anchor.body = std::mem::take(&mut self.fields[3]);
            self.comments.push(anchor);
            self.sheet = false;
            self.focus = None;
        }
    }
    pub fn apply(&mut self, reply: &PullRequestReply) {
        match reply {
            PullRequestReply::Detail { detail }
                if self.number() == Some(detail.summary.number) =>
            {
                if self.head != detail.summary.head_sha {
                    self.stale = self.has_drafts();
                    self.head = detail.summary.head_sha.clone();
                }
                self.detail = Some(detail.clone());
                self.pending = false;
            }
            PullRequestReply::Created { pull_request } if self.detail.is_none() => {
                self.pending = false;
                self.fields[0].clear();
                self.fields[1].clear();
                self.focus = None;
                self.detail = Some(PullRequestDetail {
                    summary: pull_request.clone(),
                    body: String::new(),
                    files: Vec::new(),
                    diffs: Vec::new(),
                    conversation: Vec::new(),
                    threads: Vec::new(),
                    checks: Vec::new(),
                    truncated: false,
                });
                self.message = Some(format!("Created #{}", pull_request.number));
            }
            PullRequestReply::ReviewSubmitted { number }
                if self.number() == Some(*number) =>
            {
                self.pending = false;
                self.comments.clear();
                self.fields[3].clear();
                self.sheet = false;
                self.message = Some("Review submitted".into());
            }
            PullRequestReply::ThreadUpdated { number }
                if self.number() == Some(*number) =>
            {
                self.pending = false;
                if std::mem::take(&mut self.pending_reply) {
                    self.reply_thread = None;
                    self.fields[3].clear();
                    self.sheet = false;
                }
                self.message = Some("Thread updated".into());
            }
            PullRequestReply::CheckedOut {
                number,
                path,
                worktree,
            } if self.number() == Some(*number) => {
                self.pending = false;
                self.message = Some(format!(
                    "{} · {}",
                    if *worktree {
                        "Worktree ready"
                    } else {
                        "Checked out"
                    },
                    path
                ));
            }
            PullRequestReply::Error { kind, message } => {
                self.pending_reply = false;
                self.pending = false;
                self.stale |= *kind == PullRequestErrorKind::Stale;
                self.message = Some(message.clone());
            }
            _ => {}
        }
    }
    /// Returns the remaining Changes viewport; other tabs and sheets own all input.
    pub fn render(
        &mut self,
        sl: &mut dyn Paint,
        rect: Rect,
        scale: f32,
        theme: &IdeTheme,
        occlusions: &[[f32; 4]],
    ) -> Option<Rect> {
        self.hits.clear();
        let opts = DrawOpts {
            font_size: 12.0 * scale,
            font_id: crate::primitives::prose_font::ui_prose_font_id(sl.font_library()),
            bold: false,
            color: theme.u8(theme.fg),
            clip_rect: Some([rect.x, rect.y, rect.w, rect.h]),
            ..DrawOpts::default()
        };
        sl.quad(
            None,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            theme.f32(theme.bg),
            [0.0; 4],
            0.0,
            20,
        );
        let mut y = rect.y + 12.0 * scale;
        let x = rect.x + 12.0 * scale;
        let right = (rect.x + rect.w - 12.0 * scale).max(x);
        let dim_opts = DrawOpts {
            font_size: 11.0 * scale,
            color: theme.u8(theme.dim),
            ..opts
        };
        if !self.embedded || self.sheet {
            let title = self
                .detail
                .as_ref()
                .map_or("New pull request", |d| d.summary.title.as_str());
            let title_opts = DrawOpts {
                font_size: 14.0 * scale,
                ..opts
            };
            for line in measured_lines(title, right - x, |s| {
                sl.text_mut().measure(s, &title_opts)
            }) {
                draw_text_with_occlusion(sl, x, y, &line, &title_opts, occlusions);
                y += 20.0 * scale;
            }
            y += 4.0 * scale;
            let sub = if let Some(d) = &self.detail {
                format!(
                    "#{} · {}{}\n{} ← {}",
                    d.summary.number,
                    d.summary.state,
                    if d.summary.draft { " · draft" } else { "" },
                    d.summary.base_branch,
                    d.summary.head_branch
                )
            } else {
                format!("{} ← {}", self.fields[2], self.branch)
            };
            let dim_opts = DrawOpts {
                font_size: 11.0 * scale,
                color: theme.u8(theme.dim),
                ..opts
            };
            for line in
                measured_lines(&sub, right - x, |s| sl.text_mut().measure(s, &dim_opts))
            {
                draw_text_with_occlusion(sl, x, y, &line, &dim_opts, occlusions);
                y += 17.0 * scale;
            }
            y += 8.0 * scale;
            // Tabs describe existing detail only; operational actions live on their own row.
            let mut buttons = Vec::new();
            if let Some(d) = &self.detail {
                buttons.extend([
                    ("Changes".to_string(), Intent::Tab(Tab::Changes)),
                    ("Conversation".into(), Intent::Tab(Tab::Conversation)),
                    ("Checks".into(), Intent::Tab(Tab::Checks)),
                ]);
                buttons.extend([
                    ("Browser ↗".into(), Intent::Browser(d.summary.url.clone())),
                    ("Refresh".into(), Intent::Refresh),
                    ("Check out".into(), Intent::Checkout(false)),
                    ("New worktree".into(), Intent::Checkout(true)),
                    (
                        if self.comments.is_empty() {
                            "Review…".into()
                        } else {
                            format!("Review · {}", self.comments.len())
                        },
                        Intent::Review,
                    ),
                ]);
            }
            buttons.push(("Discard drafts".into(), Intent::Discard));
            if self.sheet {
                buttons.push(("Cancel".into(), Intent::Cancel));
            }
            let mut bx = x;
            let mut was_tab = false;
            for (label, intent) in buttons {
                let is_tab = matches!(&intent, Intent::Tab(_));
                if was_tab && !is_tab {
                    bx = x;
                    y += 28.0 * scale;
                }
                was_tab = is_tab;
                let action_opts = if is_tab { opts } else { dim_opts };
                let w = sl.text_mut().measure(&label, &action_opts) + 16.0 * scale;
                if bx + w > right && bx > x {
                    bx = x;
                    y += 28.0 * scale;
                }
                if y >= rect.y + rect.h {
                    continue;
                }
                let selected = matches!(&intent,Intent::Tab(t) if *t==self.tab);
                let enabled = (!self.pending
                    || matches!(
                        &intent,
                        Intent::Tab(_) | Intent::Browser(_) | Intent::Cancel
                    ))
                    && !(matches!(&intent, Intent::Checkout(_)) && self.has_drafts());
                let r = Rect::new(
                    bx,
                    y,
                    w.min((right - bx).max(0.0)),
                    (26.0 * scale).min(rect.y + rect.h - y),
                );
                if selected && r.h >= 24.0 * scale {
                    control_quad(
                        sl,
                        Rect::new(
                            bx,
                            y + 23.0 * scale,
                            (w - 16.0 * scale).min(r.w).max(0.0),
                            scale,
                        ),
                        rect,
                        theme.f32(theme.accent),
                        occlusions,
                    );
                }
                draw_text_with_occlusion(
                    sl,
                    bx,
                    y + 4.0 * scale,
                    &label,
                    &DrawOpts {
                        clip_rect: Some([r.x, r.y, r.w, r.h]),
                        color: theme.u8(if !enabled {
                            theme.muted
                        } else if selected {
                            theme.fg
                        } else {
                            theme.dim
                        }),
                        ..action_opts
                    },
                    occlusions,
                );
                for visible in visible_regions(r, rect, occlusions) {
                    self.hits.push((visible, intent.clone()));
                }
                bx += w;
            }
            y += 32.0 * scale;
            if let Some(msg) = &self.message {
                for line in
                    measured_lines(msg, right - x, |s| sl.text_mut().measure(s, &opts))
                {
                    draw_text_with_occlusion(sl, x, y, &line, &opts, occlusions);
                    y += 20.0 * scale;
                }
                y += 4.0 * scale;
            }
            if self.stale {
                let warning_opts = DrawOpts {
                    color: theme.u8(theme.yellow),
                    ..opts
                };
                for line in measured_lines(
                    "Head changed. Drafts preserved; reopen a fresh review to submit.",
                    right - x,
                    |s| sl.text_mut().measure(s, &warning_opts),
                ) {
                    draw_text_with_occlusion(sl, x, y, &line, &warning_opts, occlusions);
                    y += 20.0 * scale;
                }
                y += 4.0 * scale;
            }
            if let Some(warning) = self.coverage_warning() {
                for line in measured_lines(warning, (right - x).max(1.0), |s| {
                    sl.text_mut().measure(s, &opts)
                }) {
                    draw_text_with_occlusion(
                        sl,
                        x,
                        y,
                        &line,
                        &DrawOpts {
                            color: theme.u8(theme.yellow),
                            ..opts
                        },
                        occlusions,
                    );
                    y += 20.0 * scale;
                }
                y += 4.0 * scale;
            }
        } // inbox owns the surrounding header
        if self.detail.is_some() && self.tab == Tab::Changes && !self.sheet {
            return Some(Rect::new(rect.x, y, rect.w, (rect.y + rect.h - y).max(0.0)));
        }
        let mut rows: Vec<(String, Option<Intent>)> = Vec::new();
        if self.detail.is_none() {
            for (i, label) in [(0, "Title"), (1, "Description"), (2, "Base branch")] {
                rows.push((label.into(), None));
                rows.push((self.fields[i].clone(), Some(Intent::Field(i))));
            }
            rows.push((
                format!("Draft · {}", if self.draft { "on" } else { "off" }),
                Some(Intent::Draft),
            ));
            rows.push((
                if self.pending {
                    "Creating…"
                } else {
                    "Create pull request"
                }
                .into(),
                Some(Intent::Submit),
            ));
        } else if self.sheet {
            rows.push((
                if let Some(a) = &self.anchor {
                    format!("{} · {:?}:{}", a.path, a.side, a.line)
                } else {
                    "Review".into()
                },
                None,
            ));
            rows.push(("Comment".into(), None));
            rows.push((self.fields[3].clone(), Some(Intent::Field(3))));
            if self.anchor.is_some() {
                rows.push(("Queue comment".into(), Some(Intent::Queue)));
            } else if self.reply_thread.is_some() {
                rows.push(("Send reply".into(), Some(Intent::Submit)));
            } else {
                for (label, e) in [
                    ("Comment", PullRequestReviewEvent::Comment),
                    ("Approve", PullRequestReviewEvent::Approve),
                    ("Request changes", PullRequestReviewEvent::RequestChanges),
                ] {
                    rows.push((
                        format!("{} {}", if self.event == e { "•" } else { " " }, label),
                        Some(Intent::Event(e)),
                    ));
                }
                rows.push((
                    if self.pending {
                        "Submitting…"
                    } else {
                        "Submit review"
                    }
                    .into(),
                    Some(Intent::Submit),
                ));
            }
            for c in &self.comments {
                rows.push((format!("Queued · {}:{} — {}", c.path, c.line, c.body), None));
            }
        } else if let Some(d) = &self.detail {
            match self.tab {
                Tab::Conversation => {
                    if !self.embedded {
                        rows.push((d.body.clone(), None));
                    } else if d.conversation.is_empty() && d.threads.is_empty() {
                        rows.push(("No conversation yet".into(), None));
                    }
                    for c in &d.conversation {
                        rows.push((
                            format!("{} · {}\n{}", c.author, c.created_at, c.body),
                            Some(Intent::Browser(c.url.clone())),
                        ));
                    }
                    for t in &d.threads {
                        rows.push((
                            format!(
                                "{}:{:?}{}{}",
                                t.path,
                                t.line,
                                if t.outdated { " · outdated" } else { "" },
                                if t.resolved { " · resolved" } else { "" }
                            ),
                            None,
                        ));
                        for c in &t.comments {
                            rows.push((format!("{}: {}", c.author, c.body), None));
                        }
                        rows.push(("Reply".into(), Some(Intent::Reply(t.id.clone()))));
                        rows.push((
                            if t.resolved { "Reopen" } else { "Resolve" }.into(),
                            Some(Intent::Resolve(t.id.clone(), !t.resolved)),
                        ));
                    }
                }
                Tab::Checks => {
                    for c in &d.checks {
                        rows.push((
                            format!("{} · {} · {} ↗", c.name, c.status, c.conclusion),
                            Some(Intent::Browser(c.url.clone())),
                        ));
                    }
                    if d.checks.is_empty() {
                        rows.push(("No checks reported".into(), None));
                    }
                }
                _ => {}
            }
            if d.truncated {
                rows.push((
                    "Results bounded by host · open in browser for full detail".into(),
                    None,
                ));
            }
        }
        let content_top = y;
        let content_clip = Rect::new(
            x,
            content_top,
            (right - x).max(0.0),
            (rect.y + rect.h - content_top).max(0.0),
        );
        y -= self.scroll;
        let mut rows = rows.into_iter().peekable();
        while let Some((text, intent)) = rows.next() {
            let start = y;
            let field = match &intent {
                Some(Intent::Field(i)) => Some(*i),
                _ => None,
            };
            let label = intent.is_none()
                && matches!(rows.peek(), Some((_, Some(Intent::Field(_)))));
            let primary = matches!(&intent, Some(Intent::Submit | Intent::Queue));
            let enabled = !self.pending && !self.stale;
            let mut row_opts = if label { dim_opts } else { opts };
            row_opts.clip_rect = Some([
                content_clip.x,
                content_clip.y,
                content_clip.w,
                content_clip.h,
            ]);
            if primary {
                row_opts.color = theme.u8(if enabled { theme.bg } else { theme.muted });
            } else if intent.is_some() && field.is_none() {
                row_opts.color =
                    theme.u8(if self.pending { theme.muted } else { theme.dim });
            }
            let inset = if field.is_some() || primary {
                8.0 * scale
            } else {
                0.0
            };
            let width = (right - x - 2.0 * inset).max(0.0);
            let lines =
                measured_lines(&text, width, |s| sl.text_mut().measure(s, &row_opts));
            let line_h = if label { 17.0 } else { 19.0 } * scale;
            if field.is_some() || primary {
                y += 5.0 * scale;
            }
            let text_y = y;
            let text_h = lines.len() as f32 * line_h;
            let control_w = if primary {
                (sl.text_mut().measure(&text, &row_opts) + 2.0 * inset).min(right - x)
            } else {
                right - x
            }
            .max(0.0);
            let control_h = text_h
                + if field.is_some() || primary {
                    10.0 * scale
                } else {
                    0.0
                };
            let control = Rect::new(x, start, control_w, control_h);
            if primary {
                control_quad(
                    sl,
                    control,
                    content_clip,
                    theme.f32(if enabled { theme.accent } else { theme.surface }),
                    occlusions,
                );
            }
            // Byte offsets stay in the original field: the caret is geometry, not a glyph.
            let mut offset = 0;
            let mut caret = None;
            for line in &lines {
                if let Some(i) = field {
                    let end = offset + line.len();
                    if self.focus == Some(i)
                        && self.cursor >= offset
                        && self.cursor <= end
                    {
                        let prefix = &line[..self.cursor - offset];
                        caret = Some(Rect::new(
                            x + inset + sl.text_mut().measure(prefix, &row_opts),
                            y,
                            scale.max(1.0),
                            14.0 * scale,
                        ));
                    }
                    offset = end;
                    if self.fields[i].as_bytes().get(offset) == Some(&b'\n') {
                        offset += 1;
                    }
                }
                draw_text_with_occlusion(sl, x + inset, y, line, &row_opts, occlusions);
                y += line_h;
            }
            if let Some(i) = field {
                control_quad(
                    sl,
                    Rect::new(
                        x,
                        text_y + text_h + 4.0 * scale,
                        (right - x).max(0.0),
                        scale,
                    ),
                    content_clip,
                    theme.f32(if self.focus == Some(i) {
                        theme.accent
                    } else {
                        theme.border
                    }),
                    occlusions,
                );
                if let Some(caret) = caret {
                    // Keep a caret at the right edge visible without spilling into the margin.
                    let caret = Rect::new(
                        caret.x.min((right - scale).max(x)),
                        caret.y,
                        caret.w,
                        caret.h,
                    );
                    control_quad(
                        sl,
                        caret,
                        content_clip,
                        theme.f32(theme.accent),
                        occlusions,
                    );
                }
            }
            y = start + control_h + if label { 2.0 } else { 10.0 } * scale;
            if let Some(i) = intent {
                for visible in visible_regions(control, content_clip, occlusions) {
                    self.hits.push((visible, i.clone()));
                }
            }
        }
        self.max_scroll = (y + self.scroll - (rect.y + rect.h)).max(0.0);
        None
    }
}
