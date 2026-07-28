//! Wave 7G: the markdown pane's "who's here" roster — pure dedupe and
//! layout logic for the collaborator dots drawn in the top-right
//! corner of the pane content area. Drawing lives in
//! `render/virtualized/roster.rs`; click-to-jump in `interactions.rs`.

use super::types::MarkdownRemoteCursor;

/// Dot diameter in logical px — small enough to read as chrome, big
/// enough to hold an initial and take a click.
pub const ROSTER_DOT_DIAMETER: f32 = 18.0;
/// Horizontal gap between dots.
pub const ROSTER_DOT_GAP: f32 = 6.0;
/// Inset from the pane's top edge.
pub const ROSTER_MARGIN_TOP: f32 = 10.0;
/// Inset from the pane's right edge — clears the scrollbar gutter
/// (6px track + 6px margin) with room to spare.
pub const ROSTER_MARGIN_RIGHT: f32 = 22.0;

/// One deduped collaborator on the open document.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownRosterEntry {
    pub name: String,
    pub color: [u8; 3],
    /// Peer publishes the rainbow preset — the dot animates locally.
    pub rainbow: bool,
    /// Zero-based source line of the peer's caret — the jump target.
    pub line: usize,
}

/// Roster entries from the pane's per-frame remote cursors. Upstream
/// the presence store keys cursors by peer id (one per peer), but the
/// pane snapshot only carries `(name, color)` — so dedupe on that
/// pair: duplicates collapse into one dot keeping the LAST cursor
/// line seen, while same-named peers with distinct colors (distinct
/// peer ids) stay separate dots. Sorted by name for a stable row.
pub fn markdown_roster_entries(
    cursors: &[MarkdownRemoteCursor],
) -> Vec<MarkdownRosterEntry> {
    let mut entries: Vec<MarkdownRosterEntry> = Vec::new();
    for cursor in cursors {
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.name == cursor.name && entry.color == cursor.color)
        {
            existing.line = cursor.line;
            existing.rainbow = cursor.rainbow;
        } else {
            entries.push(MarkdownRosterEntry {
                name: cursor.name.clone(),
                color: cursor.color,
                rainbow: cursor.rainbow,
                line: cursor.line,
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.color.cmp(&b.color)));
    entries
}

/// Dot rects for `count` entries, right-aligned so the LAST entry's
/// right edge sits at `right` and earlier entries extend leftwards.
/// Returned in entry order, one `[x, y, w, h]` per entry.
pub fn markdown_roster_dot_rects(
    count: usize,
    right: f32,
    top: f32,
    diameter: f32,
    gap: f32,
) -> Vec<[f32; 4]> {
    (0..count)
        .map(|ix| {
            let dots_after = (count - 1 - ix) as f32;
            let x = right - diameter - dots_after * (diameter + gap);
            [x, top, diameter, diameter]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(name: &str, color: [u8; 3], line: usize) -> MarkdownRemoteCursor {
        MarkdownRemoteCursor {
            name: name.to_string(),
            color,
            rainbow: false,
            line,
            col_utf16: 0,
        }
    }

    #[test]
    fn markdown_roster_dedupes_same_name_and_color_keeping_last_line() {
        let entries = markdown_roster_entries(&[
            cursor("fern", [1, 2, 3], 4),
            cursor("fern", [1, 2, 3], 9),
        ]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line, 9);
    }

    #[test]
    fn markdown_roster_keeps_same_name_distinct_colors_separate() {
        let entries = markdown_roster_entries(&[
            cursor("fern", [1, 2, 3], 4),
            cursor("fern", [9, 9, 9], 7),
        ]);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn markdown_roster_entries_sort_by_name() {
        let entries = markdown_roster_entries(&[
            cursor("zoe", [0, 0, 0], 1),
            cursor("amy", [0, 0, 0], 2),
            cursor("mel", [0, 0, 0], 3),
        ]);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["amy", "mel", "zoe"]);
    }

    #[test]
    fn markdown_roster_dot_rects_right_align_in_entry_order() {
        let rects = markdown_roster_dot_rects(3, 100.0, 10.0, 18.0, 6.0);
        assert_eq!(rects.len(), 3);
        // Last dot hugs the right edge.
        assert_eq!(rects[2], [82.0, 10.0, 18.0, 18.0]);
        // Earlier dots step left by diameter + gap.
        assert_eq!(rects[1][0], 58.0);
        assert_eq!(rects[0][0], 34.0);
        // All share the top and size.
        assert!(rects
            .iter()
            .all(|r| r[1] == 10.0 && r[2] == 18.0 && r[3] == 18.0));
    }

    #[test]
    fn markdown_roster_dot_rects_empty() {
        assert!(markdown_roster_dot_rects(0, 100.0, 10.0, 18.0, 6.0).is_empty());
    }
}
