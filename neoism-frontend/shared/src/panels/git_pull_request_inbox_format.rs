//! Pure presentation helpers: never expose host enum spellings in inbox chrome.
use neoism_protocol::pull_request::{PullRequestFilter, PullRequestSort};
use unicode_segmentation::UnicodeSegmentation;
pub(super) const SORTS: [PullRequestSort; 3] = [
    PullRequestSort::Updated,
    PullRequestSort::Newest,
    PullRequestSort::Oldest,
];
pub(super) const FILTERS: [PullRequestFilter; 4] = [
    PullRequestFilter::Open,
    PullRequestFilter::Closed,
    PullRequestFilter::Merged,
    PullRequestFilter::All,
];
pub(super) fn sort_label(v: PullRequestSort) -> &'static str {
    match v {
        PullRequestSort::Updated => "Recently updated",
        PullRequestSort::Newest => "Newest first",
        PullRequestSort::Oldest => "Oldest first",
    }
}
pub(super) fn filter_label(v: PullRequestFilter) -> &'static str {
    match v {
        PullRequestFilter::Open => "Open",
        PullRequestFilter::Closed => "Closed",
        PullRequestFilter::Merged => "Merged",
        PullRequestFilter::All => "All pull requests",
    }
}
pub(super) fn state_label(state: &str, draft: bool) -> &'static str {
    if draft {
        return "Draft";
    }
    match state.to_ascii_lowercase().as_str() {
        "open" => "Open",
        "closed" => "Closed",
        "merged" => "Merged",
        _ => "State unavailable",
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Good,
    Bad,
    Pending,
    Muted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Status {
    pub glyph: &'static str,
    pub label: &'static str,
    pub tone: Tone,
}
pub(super) fn checks(value: &str) -> Status {
    let (glyph, label, tone) = match value.to_ascii_lowercase().as_str() {
        "success" | "passing" => ("✓", "Checks passed", Tone::Good),
        "failure" | "failed" | "error" | "timed_out" => ("×", "Checks failed", Tone::Bad),
        "pending" | "expected" | "queued" | "in_progress" => {
            ("◌", "Checks pending", Tone::Pending)
        }
        "cancelled" => ("−", "Checks cancelled", Tone::Muted),
        "neutral" | "skipped" => ("−", "Checks skipped", Tone::Muted),
        _ => ("·", "No checks reported", Tone::Muted),
    };
    Status { glyph, label, tone }
}
pub(super) fn review(value: &str) -> Status {
    let (glyph, label, tone) = match value.to_ascii_lowercase().as_str() {
        "approved" => ("✓", "Approved", Tone::Good),
        "changes_requested" => ("!", "Changes requested", Tone::Bad),
        "review_required" => ("○", "Review requested", Tone::Pending),
        _ => ("·", "Not reviewed", Tone::Muted),
    };
    Status { glyph, label, tone }
}
pub(super) fn now_seconds() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}
// Gregorian civil date to Unix days. Validate before applying the era transform.
fn date_parts(value: &str) -> Option<(i64, u32, u32, i64)> {
    let date = value.get(..10)?;
    let y = date.get(..4)?.parse::<i64>().ok()?;
    let m = date.get(5..7)?.parse::<u32>().ok()?;
    let d = date.get(8..10)?.parse::<u32>().ok()?;
    if date.get(4..5)? != "-"
        || date.get(7..8)? != "-"
        || !(1..=9999).contains(&y)
        || !(1..=12).contains(&m)
    {
        return None;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max = match m {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d == 0 || d > max {
        return None;
    }
    let year = y - i64::from(m <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(m) + if m > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + i64::from(d) - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some((y, m, d, days))
}
fn timestamp(value: &str) -> Option<i64> {
    let (_, _, _, days) = date_parts(value)?;
    if value.len() == 10 {
        return Some(days * 86400);
    }
    if value.get(10..11)? != "T" || value.get(13..14)? != ":" || value.get(16..17)? != ":"
    {
        return None;
    }
    let hour = value.get(11..13)?.parse::<i64>().ok()?;
    let minute = value.get(14..16)?.parse::<i64>().ok()?;
    let second = value.get(17..19)?.parse::<i64>().ok()?;
    if hour > 23 || minute > 59 || second > 59 || hour < 0 || minute < 0 || second < 0 {
        return None;
    }
    let zone = value.get(19..)?;
    let zone = if zone.starts_with('.') {
        let n = zone[1..].find(|c: char| !c.is_ascii_digit())? + 1;
        zone.get(n..)?
    } else {
        zone
    };
    let offset = if zone == "Z" {
        0
    } else {
        let sign = match zone.get(..1)? {
            "+" => 1,
            "-" => -1,
            _ => return None,
        };
        if zone.len() != 6 || zone.get(3..4)? != ":" {
            return None;
        }
        let h = zone.get(1..3)?.parse::<i64>().ok()?;
        let m = zone.get(4..6)?.parse::<i64>().ok()?;
        if h > 23 || m > 59 || h < 0 || m < 0 {
            return None;
        }
        sign * (h * 3600 + m * 60)
    };
    Some(days * 86400 + hour * 3600 + minute * 60 + second - offset)
}
pub(super) fn updated(value: &str, now: u64) -> String {
    let Some((year, month, day, _)) = date_parts(value) else {
        return "Update time unavailable".into();
    };
    if let Some(ts) = timestamp(value) {
        let age = (now.min(i64::MAX as u64) as i64).saturating_sub(ts);
        if age >= 0 {
            if age < 60 {
                return "just now".into();
            }
            if age < 3600 {
                return format!("{}m ago", age / 60);
            }
            if age < 86400 {
                return format!("{}h ago", age / 3600);
            }
            if age < 30 * 86400 {
                return format!("{}d ago", age / 86400);
            }
        }
    }
    let name = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
        "Dec",
    ][(month - 1) as usize];
    format!("{name} {day}, {year}")
}
/// One-line truncation at grapheme boundaries (including ZWJ emoji and combining marks).
pub(super) fn ellipsis(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> String {
    if measure(text) <= width {
        return text.into();
    }
    if measure("…") > width {
        return String::new();
    }
    let mut result = String::new();
    for g in text.graphemes(true) {
        let candidate = format!("{result}{g}…");
        if measure(&candidate) > width {
            break;
        }
        result.push_str(g);
    }
    result.push('…');
    result
}
/// Keep the caret visible, favoring the current query tail without changing font metrics.
pub(super) fn search_geometry(
    prefix: f32,
    total: f32,
    width: f32,
    old_offset: f32,
    caret_width: f32,
) -> (f32, f32) {
    let usable = (width - caret_width).max(0.0);
    let mut offset = old_offset.max(0.0).min((total - usable).max(0.0));
    if prefix < offset {
        offset = prefix.max(0.0);
    } else if prefix - offset > usable {
        offset = (prefix - usable).max(0.0);
    }
    (offset, (prefix - offset).clamp(0.0, usable))
}
