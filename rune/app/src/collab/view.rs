//! Presence on screen, as GPUiX draws it: everyone in the workspace as orbs on the activity
//! rail (`chrome_topbar.rs` peer cluster), the people in a file on its row of the tree
//! (`file_tree/render.rs`), and their carets, selections and name flags in the editor
//! (`editor_presence.rs`). An orb is the person's name as a flowing pixel disc (`ui/orb.rs`);
//! their colour is the caret's, the selection's and the flag's.

use super::byte_col;
use crate::app::App;
use crate::crdt::{PeerPresence, PresenceAvatarPeer};
use crate::ui::theme::rgb;
use rune::{Color, El, el, row, text};

/// The rail shows this many people, this user included, then "+N" (`CHROME_PEER_CAP`).
const RAIL_CAP: usize = 4;
/// A tree row shows this many (`PRESENCE_ROW_CAP`), each overlapping the last by 38%.
const ROW_CAP: usize = 3;
const ROW_STEP: f32 = 0.62;
/// Orb diameter on the rail: GPUiX's 28 in its 52 pt rail, at this rail's width.
const RAIL_ORB: f32 = 30.0;
/// Rail slot height, the buttons' own.
const SLOT_H: f32 = 51.5;

fn color_of([r, g, b]: [u8; 3]) -> Color { rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)) }

/// The animated rainbow preset: 120 degrees a second on the shared clock (`rainbow_color_f32`).
fn rainbow(t: f32) -> Color {
    let (h, s, l) = ((t * 120.0).rem_euclid(360.0), 0.85_f32, 0.6_f32);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 { 0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x), 3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x) };
    let m = l - c / 2.0;
    let byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    color_of([byte(r), byte(g), byte(b)])
}

/// An orb on the backdrop disc that separates it from what it overlaps.
fn ringed(app: &App, name: &str, size: f32) -> El {
    let pad = app.ty.scale().max(1.0);
    el().w(size + 2.0 * pad).h(size + 2.0 * pad).radius(size / 2.0 + pad).fill(rgb(app.pal.bg)).center()
        .child(app.orbs.view(name, size))
}

fn tooltip_text(peer: &PresenceAvatarPeer) -> String {
    if peer.client_os.as_deref() == Some("windows") { format!("{} (Windows)", peer.display_name) } else { peer.display_name.clone() }
}

/// The people in the workspace, under the rail's buttons: this user first, one slot each.
/// Hovering one names them beside the rail.
pub fn rail(app: &App) -> Vec<El> {
    let peers = app.collab.rail_peers(app.presence_color());
    let s = app.ty.scale();
    let p = app.pal;
    let mut slots = Vec::new();
    for peer in peers.iter().take(RAIL_CAP) {
        let id = peer.peer_id.clone();
        let hovered = app.peer_hover.as_deref() == Some(id.as_str());
        let track = id.clone();
        let mut slot = el().key(format!("presence:{id}")).w(crate::ui::chrome::RAIL_W).h(SLOT_H).center()
            .child(el().center().child(ringed(app, &peer.display_name, RAIL_ORB))
                .scale(if hovered { 1.12 } else { 1.0 }).scale_motion(rune::Spring::new(0.22, 0.0)))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut rune::Tx| {
                if inside { a.peer_hover = Some(track.clone()); } else if a.peer_hover.as_deref() == Some(track.as_str()) { a.peer_hover = None; }
            });
        if hovered {
            // The name, beside the rail and level with the orb, over whatever is there.
            slot = slot.child(row().key("presence-tooltip").h(20.0 * s).px(7.0 * s).center_y().radius(5.0 * s).fill(rgb(p.surface))
                .child(text(tooltip_text(peer)).font(app.ty.sized(11.0)).color(rgb(p.fg)))
                .absolute(crate::ui::chrome::RAIL_W + 4.0 * s, (SLOT_H - 20.0 * s) / 2.0).raise());
        }
        slots.push(slot);
    }
    if peers.len() > RAIL_CAP {
        slots.push(el().key("presence-more").w(crate::ui::chrome::RAIL_W).center()
            .child(text(format!("+{}", peers.len() - RAIL_CAP)).font(app.ty.sized(13.0 * 0.82)).color(rgb(p.dim))));
    }
    slots
}

/// The people with their cursor in the file at `host_path`, for the right end of its tree
/// row: overlapping orbs, the first on top, and "+N" past three.
pub fn tree_row(app: &App, host_path: &str) -> Option<El> {
    if !app.collab.anyone() { return None; }
    let peers = app.collab.peers_in(&App::buffer_id(host_path));
    if peers.is_empty() { return None; }
    let s = app.ty.scale();
    let d = (13.0 * s + 2.0).clamp(14.0, 18.0);
    let pad = s.max(1.0);
    let shown = peers.len().min(ROW_CAP);
    let width = d + (shown - 1) as f32 * ROW_STEP * d + 2.0 * pad;
    let mut group = el().w(width).h(d + 2.0 * pad);
    // Right to left, so the leftmost orb is drawn last and lies on top.
    for (i, peer) in peers.iter().take(shown).enumerate().rev() {
        group = group.child(ringed(app, &peer.display_name, d).absolute(i as f32 * ROW_STEP * d, 0.0));
    }
    let mut cluster = row().key("tree-presence").center_y().gap(4.0).ml(8.0 * s);
    if peers.len() > shown {
        cluster = cluster.child(text(format!("+{}", peers.len() - shown)).font(app.ty.sized(13.0 * 0.82)).color(rgb(app.pal.dim)));
    }
    Some(cluster.child(group))
}

/// What one person's cursor puts on one line of a file. Rows cache on it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mark {
    name: String,
    color: [u8; 3],
    /// The rainbow preset's step, so its rows repaint as the hue turns.
    rainbow: Option<u32>,
    /// The caret's byte column on this line, and whether it is an insert beam.
    caret: Option<(usize, bool)>,
    /// The selected bytes of this line, and whether the selection runs on past its end.
    band: Option<(usize, usize, bool)>,
}

/// Other people's carets and selections on line `index` of the buffer whose shared document
/// is `buffer_id`.
pub fn line_marks(app: &App, buffer_id: Option<&str>, index: usize, line: &str) -> Vec<Mark> {
    let Some(buffer_id) = buffer_id.filter(|_| app.collab.anyone()) else { return Vec::new() };
    let mut marks = Vec::new();
    for peer in app.collab.peers_in(buffer_id) {
        let mark = mark_on(peer, index, line);
        if mark.caret.is_some() || mark.band.is_some() { marks.push(mark); }
    }
    marks
}

fn mark_on(peer: &PeerPresence, index: usize, line: &str) -> Mark {
    let caret = (peer.cursor.line as usize == index).then(|| (byte_col(line, peer.cursor.column), peer.insert));
    let band = peer.selection.and_then(|selection| {
        let (a, b) = ((selection.anchor.line, selection.anchor.column), (selection.head.line, selection.head.column));
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if start == end || index < start.0 as usize || index > end.0 as usize { return None; }
        let from = if index == start.0 as usize { byte_col(line, start.1) } else { 0 };
        let runs_on = index < end.0 as usize;
        let to = if runs_on { line.len() } else { byte_col(line, end.1) };
        (from < to || runs_on).then_some((from, to, runs_on))
    });
    Mark {
        name: peer.display_name.clone(), color: [peer.color.r, peer.color.g, peer.color.b],
        rainbow: peer.rainbow.then(|| (crate::cursor_style::rainbow_now_seconds() * 12.0) as u32), caret, band,
    }
}

/// One wrapped row of a line, as the editor lays it out.
pub struct RowGeometry<'a> {
    /// The x of a byte column of the line on this row.
    pub x_of: &'a dyn Fn(usize) -> f32,
    /// The bytes of the line this row shows, and whether it is the line's last row.
    pub start: usize,
    pub end: usize,
    pub last: bool,
    pub line_len: usize,
    pub line_h: f32,
    /// One cell's advance, and the code font's size.
    pub adv: f32,
    pub font: f32,
    /// The file's first row: a name flag has no row above to sit in.
    pub top: bool,
}

/// Draw `mark` on a row: the selection under the text's own highlights, then the caret (a
/// two-point beam in insert mode, a translucent block otherwise) with the person's name
/// flagged above it. The caret blinks on the shared half-second clock without a commit.
pub fn decorate(app: &App, mut visual: El, mark: &Mark, g: &RowGeometry) -> El {
    let color = match mark.rainbow { Some(step) => rainbow(step as f32 / 12.0), None => color_of(mark.color) };
    if let Some((a, b, runs_on)) = mark.band {
        let (from, to) = (a.max(g.start), b.min(g.end));
        let band = if from < to { Some((( g.x_of)(from), (g.x_of)(to) - (g.x_of)(from))) }
            else if runs_on && g.last && a >= g.line_len { Some(((g.x_of)(g.line_len), g.adv * 0.5)) } else { None };
        if let Some((x, w)) = band {
            let w = if runs_on && g.last && to >= g.line_len { w + g.adv * 0.5 } else { w };
            visual = visual.child(el().key(format!("peer-selection:{}", mark.name)).w(w).h(g.line_h).absolute(x, 0.0).fill(color.alpha(0.2)));
        }
    }
    let Some((byte, insert)) = mark.caret else { return visual };
    let here = (byte >= g.start && byte < g.end) || (g.last && byte >= g.end);
    if !here { return visual; }
    let x = (g.x_of)(byte.min(g.line_len));
    let caret_h = (g.font * 1.2).min(g.line_h).round();
    let y = (g.line_h - caret_h) / 2.0;
    let (w, alpha) = if insert { (2.0, 0.95) } else { (g.adv, 0.38) };
    visual = visual.child(el().key(format!("peer-caret:{}", mark.name)).w(w).h(caret_h).absolute(x, y).fill(color.alpha(alpha)).live_opacity(&app.peer_blink));
    if mark.name.is_empty() { return visual; }
    // The flag: the person's orb and their name on their colour, directly above the caret.
    let size = (g.font * 0.58).max(8.0);
    let tall = size + 4.0;
    let flag = row().key(format!("peer-flag:{}", mark.name)).h(tall).gap(2.0).center_y()
        .child(app.orbs.view(&mark.name, tall))
        .child(row().h(tall).px(4.0).center_y().radius(3.0).fill(color.alpha(0.9))
            .child(text(mark.name.clone()).font(rune::TextStyle::new(size, app.ty.weight).mono()).color(rgb(app.pal.bg))))
        .absolute(x, if g.top { 0.0 } else { y - tall });
    visual.child(flag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt::{PeerCursor, PeerSelection, PresenceColor};

    fn peer(cursor: (u32, u32), selection: Option<((u32, u32), (u32, u32))>, insert: bool) -> PeerPresence {
        let at = |(line, column)| PeerCursor { line, column, offset: None };
        PeerPresence {
            buffer_id: "file:///w/a.rs".into(), peer_id: "ada@host".into(), display_name: "Ada".into(), client_os: None, color: PresenceColor { r: 1, g: 2, b: 3 },
            cursor: at(cursor), selection: selection.map(|(a, b)| PeerSelection { anchor: at(a), head: at(b) }), insert, rainbow: false, updated_at_ms: 0,
        }
    }

    #[test]
    fn a_caret_marks_only_its_line_at_its_byte_column() {
        let p = peer((1, 2), None, true);
        assert_eq!(mark_on(&p, 0, "héllo").caret, None);
        // Column 2 in UTF-16 is byte 3 after the two-byte é.
        assert_eq!(mark_on(&p, 1, "héllo").caret, Some((3, true)));
        assert_eq!(mark_on(&p, 1, "h").caret, Some((1, true)), "a caret past a shorter line sits at its end");
    }

    #[test]
    fn a_selection_bands_each_line_it_crosses_whichever_way_it_was_dragged() {
        let lines = ["first line", "second", "third line"];
        for selection in [((0, 6), (2, 5)), ((2, 5), (0, 6))] {
            let p = peer((2, 5), Some(selection), false);
            assert_eq!(mark_on(&p, 0, lines[0]).band, Some((6, 10, true)));
            assert_eq!(mark_on(&p, 1, lines[1]).band, Some((0, 6, true)));
            assert_eq!(mark_on(&p, 2, lines[2]).band, Some((0, 5, false)));
        }
        let p = peer((1, 0), Some(((0, 0), (1, 0))), false);
        assert_eq!(mark_on(&p, 1, lines[1]).band, None, "nothing of the last line is selected");
        assert_eq!(mark_on(&peer((0, 3), Some(((0, 3), (0, 3))), false), 0, lines[0]).band, None, "an empty selection");
        // A blank line inside a selection still counts: its stub shows the selection runs on.
        assert_eq!(mark_on(&peer((2, 1), Some(((0, 0), (2, 1))), false), 1, "").band, Some((0, 0, true)));
    }
}
