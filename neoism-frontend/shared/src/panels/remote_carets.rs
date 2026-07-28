//! 7C-2: remote collaborator carets over NVIM (code) editor grids —
//! the same colored beam + name flag + top-right initial-dot roster
//! the markdown pane draws, expressed in grid-cell coordinates so one
//! painter serves both the desktop renderer and the wasm chrome.
//!
//! Coordinate contract: cues carry SCREEN rows/cols — the caller has
//! already folded the pane's `win_viewport.topline` AND the gutter
//! (`textoff`) out of the buffer-coordinate presence data and dropped
//! off-screen peers. `scroll_offset_y` is the pane's live smooth-scroll
//! pixel offset (the same one the grid cells render under) so carets
//! ride the animation instead of bouncing against it; the roster stays
//! pinned to the pane corner like markdown's. Geometry is logical px.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::roster::{
    ROSTER_DOT_DIAMETER, ROSTER_DOT_GAP, ROSTER_MARGIN_RIGHT, ROSTER_MARGIN_TOP,
};

/// One remote peer's caret on an editor grid.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorRemoteCaret {
    pub name: String,
    pub color: [u8; 3],
    /// Peer uses the rainbow cursor preset → `color` is ignored and
    /// the caret animates through hues on the shared rainbow clock.
    pub rainbow: bool,
    /// Peer is in insert/replace mode → thin beam; normal → block.
    pub insert: bool,
    /// 0-based BUFFER line — converted to a screen row AT PAINT TIME
    /// from the pane's current topline, so the caret stays glued to
    /// its line while the LOCAL user scrolls (converting once at
    /// presence-arrival time left carets pinned to the screen until
    /// the next remote heartbeat).
    pub line: u64,
    /// 0-based column in grid cells (gutter already added).
    pub col: u32,
}

const DEPTH: f32 = 0.04;
// Above the editor cells and the yank flash (22); below status chrome
// and modals.
const ORDER: u8 = 23;

/// Resolve a peer's paint color: the broadcast rgb, or the live
/// rainbow color when the peer publishes the rainbow preset.
fn remote_peer_rgb(peer: &EditorRemoteCaret) -> [f32; 3] {
    if peer.rainbow {
        let c = crate::cursor_style::rainbow_color_f32(
            crate::cursor_style::rainbow_now_seconds(),
        );
        [c[0], c[1], c[2]]
    } else {
        [
            peer.color[0] as f32 / 255.0,
            peer.color[1] as f32 / 255.0,
            peer.color[2] as f32 / 255.0,
        ]
    }
}

/// Paint every cue against the pane geometry, plus the roster (one
/// colored initial-dot per peer in the buffer, even ones scrolled out
/// of view — markdown's exact visual). Stateless; call per frame after
/// the grid cells.
#[allow(clippy::too_many_arguments)]
pub fn render_editor_remote_carets(
    sugarloaf: &mut Sugarloaf,
    cues: &[EditorRemoteCaret],
    roster: &[EditorRemoteCaret],
    topline: u64,
    pane_x: f32,
    pane_y: f32,
    pane_w: f32,
    pane_h: f32,
    cell_w: f32,
    cell_h: f32,
    // source_line_offset: the grid's whole-row render shift — local
    // wheel scrolling shifts content by rows through the scrollback
    // ring WITHOUT moving nvim's topline, so carets apply the same
    // shift the cells do: output_row = (line - topline) - shift.
    source_line_offset: i32,
    scroll_offset_y: f32,
) {
    if (cues.is_empty() && roster.is_empty())
        || pane_w <= 0.0
        || cell_w <= 0.0
        || cell_h <= 0.0
    {
        return;
    }
    let clip = [pane_x, pane_y, pane_w, pane_h];

    // Roster: markdown's presence-orb pile, pinned to the pane's
    // top-right (NOT scroll-offset — it's chrome, like md's). The right
    // anchor is md's exact math: first dot's RIGHT edge sits at
    // `pane right - ROSTER_MARGIN_RIGHT`.
    let mut dot_x = pane_x + pane_w - ROSTER_MARGIN_RIGHT - ROSTER_DOT_DIAMETER;
    let dot_y = pane_y + ROSTER_MARGIN_TOP;
    for peer in roster.iter().take(6) {
        if dot_x < pane_x {
            break;
        }
        // The peer's deterministic pixel-plasma orb replaces the old
        // colored-circle-with-initial badge — one presence identity the
        // caret flags and chrome cluster all share (seed = display name),
        // animated on the shared process clock (dots are gap-spaced, so
        // no face-pile ring is needed here).
        crate::editor::markdown::render::draw::draw_presence_orb_clipped(
            sugarloaf,
            clip,
            &peer.name,
            dot_x,
            dot_y,
            ROSTER_DOT_DIAMETER,
            crate::editor::crdt::presence_orb_now_seconds(),
            DEPTH,
            ORDER,
        );
        dot_x -= ROSTER_DOT_DIAMETER + ROSTER_DOT_GAP;
    }

    // Carets: colored beam + name flag. Row derives from the CURRENT
    // topline each frame (stays on the buffer line under local
    // scrolling) and rides the live smooth-scroll pixel offset.
    for cue in cues {
        let output_row = cue.line as i64 - topline as i64 - source_line_offset as i64;
        if output_row < -1 || (output_row as f32 * cell_h) > pane_h + cell_h {
            continue;
        }
        let x = pane_x + cue.col as f32 * cell_w;
        let y = pane_y + output_row as f32 * cell_h + scroll_offset_y;
        if x >= pane_x + pane_w || y >= pane_y + pane_h || y + cell_h <= pane_y {
            continue;
        }
        // The caret renders in the PEER'S broadcast color (their own
        // theme's cursor color). Insert/replace = thin beam; normal =
        // translucent block — mirroring how their cursor looks to them.
        // Rainbow peers animate locally on the shared clock.
        let peer_rgb = remote_peer_rgb(cue);
        if cue.insert {
            sugarloaf.rect(
                None,
                x,
                y,
                2.0,
                cell_h,
                [peer_rgb[0], peer_rgb[1], peer_rgb[2], 0.95],
                DEPTH,
                ORDER,
            );
        } else {
            sugarloaf.rect(
                None,
                x,
                y,
                cell_w.max(2.0),
                cell_h,
                [peer_rgb[0], peer_rgb[1], peer_rgb[2], 0.38],
                DEPTH,
                ORDER,
            );
        }
        let color = [peer_rgb[0], peer_rgb[1], peer_rgb[2], 0.92];
        if cue.name.is_empty() {
            continue;
        }
        let name_opts = DrawOpts {
            font_size: (cell_h * 0.58).clamp(8.0, 13.0),
            color: [13, 15, 18, 255],
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let name_w = sugarloaf.text_mut().measure(&cue.name, &name_opts);
        let tag_h = name_opts.font_size + 4.0;
        let tag_x = (x).min(pane_x + pane_w - name_w - 8.0).max(pane_x);
        let tag_y = (y - tag_h).max(pane_y);
        sugarloaf.rounded_rect(
            None,
            tag_x,
            tag_y,
            name_w + 8.0,
            tag_h,
            color,
            DEPTH,
            3.0,
            ORDER,
        );
        sugarloaf
            .text_mut()
            .draw(tag_x + 4.0, tag_y + 2.0, &cue.name, &name_opts);
    }
}
