//! Immutable commit metadata used above the shared, read-only diff cards.
use crate::{layout::Rect, primitives::{IdeTheme, draw_text_with_occlusion}};
use neoism_protocol::git::GitCommitDetail;
use sugarloaf::{text::DrawOpts, Sugarloaf};

pub struct CommitMetadata {
    pub detail: GitCommitDetail,
    copy_rect: Rect,
    message_rect: Rect,
    message_scroll: f32,
    message_max_scroll: f32,
    copy_pending: bool,
}
impl CommitMetadata {
    pub fn new(detail: GitCommitDetail) -> Self {
        Self { detail, copy_rect: Rect::new(0.0, 0.0, 0.0, 0.0), message_rect: Rect::new(0.0, 0.0, 0.0, 0.0), message_scroll: 0.0, message_max_scroll: 0.0, copy_pending: false }
    }
    pub fn pointer(&self, x: f32, y: f32) -> bool { self.copy_rect.contains(x, y) }
    pub fn click(&mut self, x: f32, y: f32) -> bool {
        if !self.pointer(x, y) { return false; }
        self.copy_pending = true;
        true
    }
    pub fn take_copy(&mut self) -> Option<String> {
        std::mem::take(&mut self.copy_pending).then(|| self.detail.sha.clone())
    }
    /// Long commit messages have their own bounded viewport; nothing is dropped.
    pub fn scroll_at(&mut self, pointer: Option<(f32, f32)>, delta: f32) -> bool {
        if !pointer.is_some_and(|(x, y)| self.message_rect.contains(x, y)) { return false; }
        self.message_scroll = (self.message_scroll + delta).clamp(0.0, self.message_max_scroll);
        true
    }
    pub fn render(&mut self, painter: &mut Sugarloaf, rect: Rect, scale: f32, theme: &IdeTheme, occlusions: &[[f32; 4]]) -> Rect {
        let line_h = 20.0 * scale;
        let x = rect.x + 16.0 * scale;
        let width = (rect.w - 32.0 * scale).max(1.0);
        let opts = DrawOpts { font_size: 12.0 * scale, color: theme.u8(theme.fg), ..DrawOpts::default() };
        // Measured, Unicode-safe wrapping, including unbroken subjects and SHAs.
        let mut lines = Vec::new();
        for source in self.detail.message.lines() {
            let mut line = String::new();
            for ch in source.chars() {
                let previous = line.len();
                line.push(ch);
                if previous > 0 && painter.text_mut().measure(&line, &opts) > width {
                    line.truncate(previous);
                    lines.push(std::mem::take(&mut line));
                    line.push(ch);
                }
            }
            lines.push(line);
        }
        let height = ((lines.len().max(1) + 3) as f32 * line_h + 24.0 * scale).min(rect.h * 0.55);
        painter.quad(None, rect.x, rect.y, rect.w, height, theme.f32(theme.bg), [0.0; 4], 0.0, 20);
        let opts = DrawOpts { clip_rect: Some([rect.x, rect.y, rect.w, height]), ..opts };
        let mut y = rect.y + 12.0 * scale;
        for text in [format!("{} <{}>", self.detail.author, self.detail.email), format_date(self.detail.timestamp, self.detail.offset_minutes)] {
            let _ = draw_text_with_occlusion(painter, x, y, &text, &opts, occlusions);
            y += line_h;
        }
        let label = format!("{}  Copy SHA", self.detail.sha);
        let copy_width = draw_text_with_occlusion(painter, x, y, &label, &DrawOpts { color: theme.u8(theme.accent), ..opts }, occlusions);
        self.copy_rect = Rect::new(x, y, copy_width.min(width).max(0.0), line_h);
        y += line_h;
        self.message_rect = Rect::new(x, y, width, (height - (y - rect.y) - 8.0 * scale).max(0.0));
        self.message_max_scroll = (lines.len() as f32 * line_h - self.message_rect.h).max(0.0);
        self.message_scroll = self.message_scroll.min(self.message_max_scroll);
        let opts = DrawOpts { clip_rect: Some([x, y, width, self.message_rect.h]), ..opts };
        for (index, text) in lines.iter().enumerate() {
            let line_y = y + index as f32 * line_h - self.message_scroll;
            if line_y + line_h >= y && line_y < y + self.message_rect.h {
                let _ = draw_text_with_occlusion(painter, x, line_y, text, &opts, occlusions);
            }
        }
        Rect::new(rect.x, rect.y + height, rect.w, (rect.h - height).max(0.0))
    }
}

/// Git author time, including its recorded UTC offset (no host-local timezone).
pub fn format_date(timestamp: i64, offset: i32) -> String {
    let seconds = timestamp + i64::from(offset) * 60;
    let days = seconds.div_euclid(86400);
    let clock = seconds.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} {}{:02}:{:02}", clock / 3600, clock / 60 % 60, clock % 60, if offset < 0 { '-' } else { '+' }, offset.abs() / 60, offset.abs() % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_time_is_offset_aware_and_handles_before_epoch() {
        assert_eq!(format_date(0, 0), "1970-01-01 00:00:00 +00:00");
        assert_eq!(format_date(0, 330), "1970-01-01 05:30:00 +05:30");
        assert_eq!(format_date(-1, 0), "1969-12-31 23:59:59 +00:00");
        assert_eq!(format_date(0, -60), "1969-12-31 23:00:00 -01:00");
    }
    #[test]
    fn historical_snapshot_cannot_focus_live_editor() {
        let detail = GitCommitDetail { sha: "abcdef0123456789".into(), author: "Ada".into(), email: "a@example.test".into(), timestamp: 0, offset_minutes: 0, message: "subject\n\nbody".into(), files: Vec::new(), diffs: Vec::new() };
        let mut view = crate::panels::git_diff_view::GitDiffView::from_commit("/repo".into(), detail);
        view.set_edit_focus(true);
        assert!(view.editing_path().is_none());
        assert!(view.take_edit_request().is_none());
        assert!(view.take_actions().is_empty());
        assert_eq!(view.commit_id.as_deref(), Some("abcdef0123456789"));
    }
}
