//! The diagnostics popup (`panels/diagnostics_popup.rs`): a small panel above the status
//! line's error or warning pill listing that severity's diagnostics in the file, each with
//! its line. Up/Down move the selection, Enter or a click goes to the line, Escape or a click
//! outside closes it.

use super::lsp::{Diagnostic, Severity};
use crate::app::{App, Focus};
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{El, Key, KeyEvent, Tx, WheelEvent, col, el, row, text};

const WIDTH: f32 = 460.0;
const PADDING: f32 = 12.0;
const ROW_H: f32 = 22.0;
const ROW_GAP: f32 = 2.0;
const HEADER_H: f32 = 22.0;
const BODY_GAP: f32 = 6.0;
const FOOTER_H: f32 = 16.0;
const FONT_BODY: f32 = 12.0;
const FONT_HEADER: f32 = 13.0;
const FONT_HINT: f32 = 11.0;
const MAX_ROWS: usize = 10;
const RADIUS: f32 = 8.0;
/// The gap between the popup and the pill it opens over.
const GAP: f32 = 10.0;

/// The open popup: which pill it belongs to, the selected row and the first row shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagPopup {
    pub severity: Severity,
    pub selected: usize,
    pub top: usize,
}

/// The pill's rect key (`App::rects`).
pub fn pill_key(severity: Severity) -> &'static str {
    if severity == Severity::Error { "diag-pill:error" } else { "diag-pill:warn" }
}

fn glyph(severity: Severity) -> &'static str {
    match severity { Severity::Error => "\u{ea87}", Severity::Warn => "\u{f071}", _ => "\u{f05a}" }
}

impl App {
    /// The active file's diagnostics of `severity`, in line order.
    fn diagnostics_of(&self, severity: Severity) -> Vec<&Diagnostic> {
        let mut items: Vec<&Diagnostic> = self.active_buffer().map(|b| b.lsp.diagnostics.iter().filter(|d| d.severity == severity).collect()).unwrap_or_default();
        items.sort_by_key(|d| (d.line, d.col));
        items
    }

    /// A press on a status pill: open its popup, or close it when it is the one showing.
    pub fn toggle_diag_popup(&mut self, severity: Severity) {
        self.diag_popup = match self.diag_popup {
            Some(open) if open.severity == severity => None,
            _ => Some(DiagPopup { severity, selected: 0, top: 0 }),
        };
    }

    /// Keys while the popup is open. Returns whether the key was taken.
    pub fn diag_popup_key(&mut self, event: &KeyEvent) -> bool {
        let Some(mut popup) = self.diag_popup else { return false };
        if event.mods.alt || event.mods.ctrl || event.mods.cmd { return false; }
        let count = self.diagnostics_of(popup.severity).len();
        match &event.key {
            Key::Escape => { self.diag_popup = None; return true; }
            Key::Enter => { self.activate_diagnostic(popup.selected); return true; }
            Key::Up => popup.selected = popup.selected.saturating_sub(1),
            Key::Down => popup.selected = (popup.selected + 1).min(count.saturating_sub(1)),
            _ => return false,
        }
        popup.top = scrolled(popup.top, popup.selected, count);
        self.diag_popup = Some(popup);
        true
    }

    /// Go to the popup's `index`th diagnostic and close the popup.
    pub fn activate_diagnostic(&mut self, index: usize) {
        let Some(popup) = self.diag_popup.take() else { return };
        let Some((line, col)) = self.diagnostics_of(popup.severity).get(index).map(|d| (d.line, d.col)) else { return };
        self.focus = Focus::Content;
        self.editor_follow = true;
        if let Some(buffer) = self.active_buffer_mut() {
            let line = line.min(buffer.doc.lines.len().saturating_sub(1));
            let col = col.min(buffer.doc.lines.get(line).map_or(0, |text| text.len()));
            buffer.doc.set_cursor(super::vim::Pos { line, col });
            buffer.doc.follow_cursor = true;
        }
    }
}

/// The first row to show so that `selected` is among the `MAX_ROWS` visible of `count`.
fn scrolled(top: usize, selected: usize, count: usize) -> usize {
    let top = if selected < top { selected } else if selected >= top + MAX_ROWS { selected + 1 - MAX_ROWS } else { top };
    top.min(count.saturating_sub(MAX_ROWS))
}

pub fn view(app: &App) -> El {
    let Some(popup) = app.diag_popup else { return el() };
    let items = app.diagnostics_of(popup.severity);
    let Some(anchor) = app.rects.get(pill_key(popup.severity)).copied().filter(|_| !items.is_empty()) else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let accent = rgb(if popup.severity == Severity::Error { p.red } else { p.yellow });
    let visible = items.len().min(MAX_ROWS);
    let footer = items.len() > visible;
    let pad = PADDING * s;
    let row_h = (ROW_H + ROW_GAP) * s;
    let w = (WIDTH * s).min((app.window[0] - 16.0 * s).max(1.0));
    let h = pad * 2.0 + HEADER_H * s + BODY_GAP * s + visible as f32 * row_h + if footer { FOOTER_H * s } else { 0.0 };
    // Centred on the pill, just above it.
    let x = (anchor[0] + anchor[2] / 2.0 - w / 2.0).clamp(8.0 * s, (app.window[0] - w - 8.0 * s).max(8.0 * s));
    let y = (anchor[1] - h - GAP * s).max(8.0 * s);
    let body = TextStyle::new(FONT_BODY * s, 400.0).mono();
    let bold = TextStyle::new(FONT_BODY * s, 700.0).mono();

    let title = if popup.severity == Severity::Error { "Errors" } else { "Warnings" };
    let mut list = col().w_full();
    let top = popup.top.min(items.len().saturating_sub(visible));
    for (index, item) in items.iter().enumerate().skip(top).take(visible) {
        let selected = index == popup.selected;
        let ink = super::lsp_view::severity_color(p, item.severity, 1.0, 1.0);
        list = list.child(row().key(format!("diagnostic-row:{index}")).w_full().h(ROW_H * s).mb(ROW_GAP * s).px(4.0 * s).center_y().radius(4.0 * s)
            .fill(if selected { rgb(p.hover) } else { rune::Color::TRANSPARENT }).instant_changes()
            .child(text(glyph(item.severity)).font(TextStyle::new(FONT_BODY * s, 700.0).family(NERD)).color(ink))
            .child(text(format!("{:>4}", item.line + 1)).font(bold).color(rgb(p.muted)).ml(8.0 * s))
            .child(text(item.message.replace('\n', "  ")).font(if selected { bold } else { body }).color(ink).ml(10.0 * s).truncate().grow(1.0).shrink())
            .on_pointer_move(move |a: &mut App, _| { if let Some(open) = a.diag_popup.as_mut().filter(|open| open.selected != index) { open.selected = index; } })
            .on_press(move |a: &mut App| a.activate_diagnostic(index)));
    }
    let count = items.len();
    let mut card = col().key("diagnostics-popup").w(w).h(h).absolute(x, y).radius(RADIUS * s).fill(rgb(p.surface).alpha(0.98))
        .border(2.0 * s, rgb(p.black).alpha(0.45)).clip_rounded(RADIUS * s)
        .on_press(|_: &mut App| {})
        .on_wheel(move |a: &mut App, e: WheelEvent, _: &mut Tx| {
            let Some(open) = a.diag_popup.as_mut() else { return false };
            let step = if e.dy > 0.0 { -1 } else if e.dy < 0.0 { 1 } else { 0 };
            open.top = (open.top as isize + step).clamp(0, count.saturating_sub(MAX_ROWS) as isize) as usize;
            true
        })
        // The severity's bar down the left edge.
        .child(el().w(3.0 * s).h(h).absolute(0.0, 0.0).fill(accent))
        .child(col().w_full().padding(pad).pl(pad + 4.0 * s)
            .child(row().w_full().h(HEADER_H * s).child(text(format!("{title} \u{2014} {count}")).font(TextStyle::new(FONT_HEADER * s, 700.0).mono()).color(accent)))
            .child(el().h(BODY_GAP * s))
            .child(list));
    if footer {
        card = card.child(text(format!("+{} more \u{2014} scroll to see all", items.len() - visible)).font(TextStyle::new(FONT_HINT * s, 400.0).mono()).color(rgb(p.muted))
            .absolute(pad + 4.0 * s, h - pad - FONT_HINT * s * 1.2));
    }
    el().key("diagnostics-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.diag_popup = None)
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_scrolls_to_keep_the_selection_in_its_ten_rows() {
        assert_eq!(scrolled(0, 3, 25), 0);
        assert_eq!(scrolled(0, 10, 25), 1, "one past the last visible row");
        assert_eq!(scrolled(5, 2, 25), 2, "above the first visible row");
        assert_eq!(scrolled(20, 24, 25), 15, "never past the end");
        assert_eq!(scrolled(0, 2, 4), 0, "a short list never scrolls");
    }

    #[test]
    fn a_pill_toggles_its_own_popup_and_switches_from_the_other() {
        let mut app = App::new();
        app.toggle_diag_popup(Severity::Error);
        assert_eq!(app.diag_popup.map(|p| p.severity), Some(Severity::Error));
        app.toggle_diag_popup(Severity::Warn);
        assert_eq!(app.diag_popup.map(|p| p.severity), Some(Severity::Warn));
        app.toggle_diag_popup(Severity::Warn);
        assert_eq!(app.diag_popup, None);
    }
}
