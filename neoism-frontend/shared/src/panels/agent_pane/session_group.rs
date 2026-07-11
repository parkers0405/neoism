//! Date-grouping + pinning helpers shared by the `/sessions` picker and the
//! agent side-panel session list.
//!
//! Both the desktop (`neoism-frontend/desktop`) and wasm
//! (`neoism-frontend/wasm`) session-option builders funnel through these so the
//! two paths group sessions identically: a "Pinned" section first, then one
//! section per calendar day ("Tue May 19 2026"), newest day first.
//!
//! The date formatting is a self-contained unix-ms → civil-date conversion
//! (Howard Hinnant's `civil_from_days`) so it needs no `chrono`/`time`
//! dependency and compiles unchanged on wasm — it never reads the current
//! clock, only the session's own `updated` timestamp.

use crate::panels::agent_pane::state::picker::NeoismAgentPickerOption;

/// Footer hint band shown under the `/sessions` picker.
pub const SESSION_PICKER_FOOTER: &str =
    "pin/unpin ctrl+f   delete ctrl+d   rename ctrl+r";

const MS_PER_DAY: u64 = 86_400_000;

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Whole days since the unix epoch for a unix-millisecond timestamp.
/// Sessions never carry pre-1970 timestamps, so a plain floor-divide is exact.
pub fn day_index_from_unix_ms(ms: u64) -> i64 {
    (ms / MS_PER_DAY) as i64
}

/// Format a unix-millisecond timestamp as a date-group header, e.g.
/// `"Tue May 19 2026"` (weekday, month, zero-padded day, year — all UTC).
pub fn format_date_header(ms: u64) -> String {
    let days = (ms / MS_PER_DAY) as i64;
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday; index 0 = Sunday.
    let weekday = (((days % 7) + 4) % 7) as usize;
    format!(
        "{} {} {:02} {}",
        WEEKDAYS[weekday],
        MONTHS[(month - 1) as usize],
        day,
        year
    )
}

/// Howard Hinnant's `civil_from_days`: days-since-epoch → (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// A session picker option plus the raw `updated` timestamp used to bucket it.
pub struct SessionOptionInput {
    pub option: NeoismAgentPickerOption,
    pub updated_ms: u64,
}

/// Group session options into a "Pinned" section (if any) followed by one
/// date-header section per calendar day, newest first. Header rows are
/// [`NeoismAgentPickerOption::header`] entries the picker widget skips for
/// selection. Untracked sessions (`updated_ms == 0`) collect under an
/// "Untracked" footer section.
pub fn group_session_options(
    mut items: Vec<SessionOptionInput>,
) -> Vec<NeoismAgentPickerOption> {
    // Newest first; the caller may already sort, but be robust.
    items.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));

    let (pinned, unpinned): (Vec<_>, Vec<_>) =
        items.into_iter().partition(|item| item.option.pinned);

    let mut out = Vec::with_capacity(pinned.len() + unpinned.len() + 8);

    if !pinned.is_empty() {
        out.push(NeoismAgentPickerOption::header("Pinned"));
        out.extend(pinned.into_iter().map(|item| item.option));
    }

    let mut last_day: Option<i64> = None;
    for item in unpinned {
        let day = if item.updated_ms == 0 {
            i64::MIN
        } else {
            day_index_from_unix_ms(item.updated_ms)
        };
        if last_day != Some(day) {
            last_day = Some(day);
            let label = if item.updated_ms == 0 {
                "Untracked".to_string()
            } else {
                format_date_header(item.updated_ms)
            };
            out.push(NeoismAgentPickerOption::header(&label));
        }
        out.push(item.option);
    }

    out
}

/// Section label a session at `index` opens, if it starts a new group —
/// `Some("Pinned")`, `Some("Tue May 19 2026")`, or `None` when it continues
/// the previous row's group. `pinned`/`updated_ms` are read positionally so
/// the side panel (which stores a flat, pre-sorted `Vec`) can draw the same
/// headers the picker inserts as rows.
pub fn section_label_at(
    index: usize,
    pinned: impl Fn(usize) -> bool,
    updated_ms: impl Fn(usize) -> u64,
) -> Option<String> {
    let is_pinned = pinned(index);
    let prev_pinned = index.checked_sub(1).map(&pinned);

    if is_pinned {
        return (prev_pinned != Some(true)).then(|| "Pinned".to_string());
    }

    let day = day_bucket(updated_ms(index));
    let prev_day = match index.checked_sub(1) {
        Some(prev) if !pinned(prev) => Some(day_bucket(updated_ms(prev))),
        _ => None,
    };
    if prev_day == Some(day) {
        return None;
    }
    Some(if updated_ms(index) == 0 {
        "Untracked".to_string()
    } else {
        format_date_header(updated_ms(index))
    })
}

fn day_bucket(ms: u64) -> i64 {
    if ms == 0 {
        i64::MIN
    } else {
        day_index_from_unix_ms(ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_dates() {
        // 2026-05-19 00:00:00 UTC = 1_779_580_800 s.
        assert_eq!(format_date_header(1_779_580_800_000), "Tue May 19 2026");
        // 2026-07-07 = 1_783_382_400 s (a Tuesday).
        assert_eq!(format_date_header(1_783_382_400_000), "Tue Jul 07 2026");
        // Epoch.
        assert_eq!(format_date_header(0), "Thu Jan 01 1970");
    }

    #[test]
    fn groups_pinned_first_then_by_day() {
        let mk = |title: &str, ms: u64, pinned: bool| {
            let mut option = NeoismAgentPickerOption::new(title, "", "", title);
            option.pinned = pinned;
            SessionOptionInput {
                option,
                updated_ms: ms,
            }
        };
        let day = 86_400_000u64;
        let out = group_session_options(vec![
            mk("a", 3 * day, false),
            mk("pin", day, true),
            mk("b", 3 * day + 5, false),
            mk("c", 2 * day, false),
        ]);
        let titles: Vec<&str> = out.iter().map(|o| o.title.as_str()).collect();
        // Pinned section, then day-3 group (b then a, newest first), then day-2.
        assert_eq!(titles[0], "Pinned");
        assert_eq!(titles[1], "pin");
        assert!(out[2].is_header);
        assert_eq!(titles[3], "b");
        assert_eq!(titles[4], "a");
        assert!(out[5].is_header);
        assert_eq!(titles[6], "c");
    }
}
