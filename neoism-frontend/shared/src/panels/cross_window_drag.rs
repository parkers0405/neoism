//! Cross-window tab-drag policy.
//!
//! Single-window tab drag (reorder + tear-out into a new pane) is
//! handled inside [`crate::panels::buffer_tabs`]. This module covers
//! the next step up: dragging a tab from one OS window into another
//! OS window owned by the same daemon.
//!
//! The host pipeline is:
//!   1. Mouse press on a tab in window A starts a buffer-tab drag.
//!   2. The cursor leaves window A — winit fires `CursorLeft` and the
//!      host stores the global cursor coordinate snapshot.
//!   3. Mouse release fires inside window A (winit captures the
//!      pointer for the duration of the drag), but the in-window
//!      destination is `None` so `handle_buffer_tabs_drag_release`
//!      asks this module whether the release belongs to some other
//!      window owned by the same router.
//!   4. If yes, the host serialises the tab into a
//!      [`CrossWindowTabPayload`], drops it on the source side, and
//!      calls into the destination window's `Screen` to open the
//!      payload's target (mirroring `tear_out_*_tab_to_pane`).
//!
//! This module is intentionally pure: it has no IO, no winit deps,
//! and no `Screen` mention. It just decides "given a global cursor
//! position and a list of candidate window rectangles, which window
//! owns the drop, if any?" plus offers a small payload shape that the
//! host can move across windows. Tests live below.
//!
//! Same-daemon cross-window drag ships in C5; cross-daemon (drag a tab
//! from laptop → phone via Tailscale) reuses
//! [`CrossWindowTabPayload`] as the on-wire body of a future
//! `WorkspaceClientMessage::DropTabIntoWindow` variant. The protocol
//! variant is *not* added in C5 — see the C5 task Result for the
//! cross-daemon follow-up plan.
//!
//! ## Manual test
//!
//! 1. `cargo run -p neoism` and open a workspace with at least two
//!    files in the buffer-tab strip.
//! 2. `Cmd-N` (or whatever the host's "new window" binding is) to
//!    open a second window from the same daemon process.
//! 3. Drag a tab from window A's strip; while still holding the mouse
//!    button, move the cursor across the desktop into window B's
//!    title-bar/empty area and release.
//! 4. The tab should disappear from window A and open as a new
//!    editor surface in window B (mirroring how `tear_out_file_tab_to_pane`
//!    behaves within a single window).
//! 5. Cancel path: release the cursor over neither window's bounds —
//!    the tab snaps back into window A's strip via the existing
//!    `reinsert_tab_plan` path.

use std::path::PathBuf;

/// What kind of editor surface a torn-out tab represents on the
/// destination side. Mirrors the desktop fork's `TabDragReleaseKind`
/// ordering (markdown beats raw file, agent wins if `agent_tag` is
/// set).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CrossWindowTabKind {
    /// Rust-rendered markdown document tab.
    Markdown { path: PathBuf },
    /// Plain file tab (code-pane-backed).
    File { path: PathBuf },
    /// Agent CLI / Neoism agent tab. `agent_tag` is a daemon-local
    /// stringified discriminator (the desktop fork uses
    /// `crate::neoism::icon::AgentKind`'s `as_str`); the destination
    /// resolves it back to its native enum.
    Agent {
        #[serde(default)]
        agent_tag: Option<String>,
        #[serde(default)]
        path: Option<PathBuf>,
    },
}

/// On-the-wire payload for a cross-window tab move. Stays small and
/// `Clone + Serialize + Deserialize` so the future cross-daemon hop
/// (laptop → phone via Tailscale) can put it straight onto the
/// `WorkspaceClientMessage` enum without restructuring.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CrossWindowTabPayload {
    /// What to open on the destination.
    pub kind: CrossWindowTabKind,
    /// Display title for the source pane's "tear-out animation" and
    /// for any toast the destination wants to surface. Optional so
    /// hosts that don't track titles can leave it `None`.
    #[serde(default)]
    pub title: Option<String>,
    /// `true` when the source tab had unsaved edits. Carried so the
    /// destination can preserve the dot indicator if it cares.
    #[serde(default)]
    pub modified: bool,
}

/// Axis-aligned rectangle of an OS window in *physical* screen
/// coordinates (the same coordinate space winit's
/// `Window::outer_position` and `Window::outer_size` use). The drop
/// classifier doesn't care about scale factor — it's pure hit-testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowRect {
    /// Strict contains: the right and bottom edges are exclusive so
    /// adjacent windows don't both claim the seam pixel.
    #[inline]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let w = self.width as i32;
        let h = self.height as i32;
        x >= self.x && y >= self.y && x < self.x + w && y < self.y + h
    }
}

/// Candidate window in the same daemon, paired with its current
/// physical outer rect. `id` is opaque to the policy — the host
/// supplies whatever `WindowId`-shaped key it uses to look the
/// window up afterwards. We intentionally take `u64` so the policy
/// stays winit-free; the host converts winit's `WindowId` via
/// `Into<u64>` (every winit backend exposes it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrossWindowCandidate {
    pub id: u64,
    pub rect: WindowRect,
}

/// Pick the destination window for a tab drop.
///
/// Returns `Some(window_id)` for the first candidate whose rect
/// contains the global cursor position, *excluding* the source
/// window. Returns `None` if the cursor is outside every candidate
/// (drop missed every window — the host should snap the tab back
/// into the source strip via the existing reinsert path).
///
/// `source_id == Some(id)` filters that id out so a drop that
/// happens to land back inside the source window's rect (e.g. the
/// user dragged out and back in before releasing) still gets handled
/// by the source's normal in-window drop pipeline rather than by
/// the cross-window path. Pass `None` for `source_id` to consider
/// every candidate (useful for tests).
///
/// Iteration order matters: callers should pass candidates in
/// stacking order (front-most first) so two overlapping windows
/// resolve to the on-top one. The router stores windows in a hash
/// map so it loses stacking order by default — callers that care
/// can sort by `winit_window.has_focus()` first or just accept the
/// hash-iteration order, which is fine for the common "windows are
/// side-by-side" case.
pub fn pick_destination_window(
    cursor_x: i32,
    cursor_y: i32,
    source_id: Option<u64>,
    candidates: &[CrossWindowCandidate],
) -> Option<u64> {
    candidates
        .iter()
        .find(|c| Some(c.id) != source_id && c.rect.contains(cursor_x, cursor_y))
        .map(|c| c.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> WindowRect {
        WindowRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn cand(id: u64, x: i32, y: i32, w: u32, h: u32) -> CrossWindowCandidate {
        CrossWindowCandidate {
            id,
            rect: rect(x, y, w, h),
        }
    }

    #[test]
    fn rect_contains_inside_and_corners() {
        let r = rect(10, 20, 100, 50);
        assert!(r.contains(10, 20)); // top-left corner is inclusive
        assert!(r.contains(50, 40));
        assert!(r.contains(109, 69)); // last pixel inside
        assert!(!r.contains(110, 70)); // right/bottom edges exclusive
        assert!(!r.contains(9, 20));
        assert!(!r.contains(10, 19));
    }

    #[test]
    fn no_candidates_returns_none() {
        assert_eq!(pick_destination_window(0, 0, None, &[]), None);
    }

    #[test]
    fn cursor_outside_all_candidates_returns_none() {
        let cs = [cand(1, 0, 0, 100, 100), cand(2, 200, 0, 100, 100)];
        assert_eq!(pick_destination_window(500, 500, None, &cs), None);
    }

    #[test]
    fn cursor_inside_single_candidate_picks_it() {
        let cs = [cand(7, 100, 100, 200, 200)];
        assert_eq!(pick_destination_window(150, 150, None, &cs), Some(7));
    }

    #[test]
    fn cursor_inside_source_window_is_skipped() {
        // Single window in the list, but it's the source — drop must
        // be ignored so the host falls back to its in-window pipeline.
        let cs = [cand(7, 100, 100, 200, 200)];
        assert_eq!(pick_destination_window(150, 150, Some(7), &cs), None);
    }

    #[test]
    fn cursor_picks_first_matching_non_source() {
        // Two windows side by side; cursor in the second. Source is
        // the first.
        let cs = [cand(1, 0, 0, 100, 100), cand(2, 100, 0, 100, 100)];
        assert_eq!(pick_destination_window(150, 50, Some(1), &cs), Some(2));
    }

    #[test]
    fn overlapping_windows_pick_first_in_order() {
        // Two overlapping windows; iteration order resolves the tie.
        let cs = [cand(1, 0, 0, 200, 200), cand(2, 50, 50, 200, 200)];
        assert_eq!(pick_destination_window(100, 100, None, &cs), Some(1));
        // Same hit area but source = 1 → falls through to 2.
        assert_eq!(pick_destination_window(100, 100, Some(1), &cs), Some(2));
    }

    #[test]
    fn payload_roundtrips_through_serde() {
        let payload = CrossWindowTabPayload {
            kind: CrossWindowTabKind::Markdown {
                path: PathBuf::from("/tmp/notes.md"),
            },
            title: Some("notes.md".into()),
            modified: true,
        };
        let wire = serde_json::to_string(&payload).expect("serialize");
        let back: CrossWindowTabPayload =
            serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(payload, back);
    }

    #[test]
    fn agent_payload_roundtrips_with_optional_path() {
        let payload = CrossWindowTabPayload {
            kind: CrossWindowTabKind::Agent {
                agent_tag: Some("claude".into()),
                path: None,
            },
            title: None,
            modified: false,
        };
        let wire = serde_json::to_string(&payload).expect("serialize");
        let back: CrossWindowTabPayload =
            serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(payload, back);
    }
}
