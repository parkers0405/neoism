// Diagnostics popup — small floating panel that pops up above a status
// line pill when the user clicks it. Lists the items vim.diagnostic.get()
// returned for the current buffer, grouped by severity. Clicking a row
// returns the line number; the screen layer then sends `:<lnum>` to nvim.
//
// Lifecycle / animation: handled by the shared `Popover<T>` widget —
// this file owns only the diagnostics-specific content (items, message
// horizontal scroll) and the visuals (severity glyph, line numbers,
// row layout). ESC dismissal, click-outside-to-close, and the fade
// curve all live in `widgets/overlay.rs`.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use super::status_line::DiagnosticPill;
use crate::editor_snapshot::{DiagnosticItem, DiagnosticSeverity};
use crate::primitives::IdeTheme;
use crate::widgets::popover::{Popover, PopoverAnchor};

const POPUP_WIDTH: f32 = 460.0;
const POPUP_PADDING: f32 = 12.0;
const ROW_HEIGHT: f32 = 22.0;
const ROW_GAP: f32 = 2.0;
const HEADER_HEIGHT: f32 = 22.0;
const FONT_BODY: f32 = 12.0;
const FONT_HEADER: f32 = 13.0;
const FONT_HINT: f32 = 11.0;
const MAX_VISIBLE_ROWS: usize = 10;
const CORNER_RADIUS: f32 = 8.0;
const ANIM_MS: f32 = 160.0;
const POPUP_GAP: f32 = 10.0;

const DEPTH_BG: f32 = 0.05;
const DEPTH_TEXT: f32 = 0.06;
const ORDER: u8 = 30;

/// Severity displayed by the popup. Kept local (rather than re-using
/// [`DiagnosticSeverity`] directly) so the popup's glyph / color
/// switches stay self-contained — same shape, different ordinals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
    Hint,
}

impl Severity {
    /// Translate the shared snapshot severity into the popup's local
    /// enum. Shared exposes `DiagnosticSeverity` already grouped — no
    /// raw u8 lookup needed at this boundary.
    pub fn from_snapshot(s: DiagnosticSeverity) -> Self {
        match s {
            DiagnosticSeverity::Error => Severity::Error,
            DiagnosticSeverity::Warn => Severity::Warn,
            DiagnosticSeverity::Info => Severity::Info,
            DiagnosticSeverity::Hint => Severity::Hint,
        }
    }

    fn glyph(self) -> &'static str {
        // Shared with the status line's diagnostics pills (and Mash Up
        // Pack `status.*` icon overrides) via `primitives::icons`.
        match self {
            Severity::Error => crate::primitives::icons::error_glyph(),
            Severity::Warn => crate::primitives::icons::warn_glyph(),
            Severity::Info => crate::primitives::icons::info_glyph(),
            Severity::Hint => crate::primitives::icons::hint_glyph(),
        }
    }

    fn color(self, theme: &IdeTheme) -> [u8; 4] {
        match self {
            Severity::Error => theme.u8(theme.red),
            Severity::Warn => theme.u8(theme.yellow),
            Severity::Info => theme.u8(theme.blue),
            Severity::Hint => theme.u8(theme.cyan),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PopupItem {
    pub lnum: u64,
    pub severity: Severity,
    pub message: String,
}

impl From<&DiagnosticItem> for PopupItem {
    fn from(d: &DiagnosticItem) -> Self {
        PopupItem {
            // Shared `DiagnosticItem.lnum` is `u32` (1-based, matches
            // nvim's `vim.diagnostic.get`); widen at the POD boundary so
            // the popup's own struct keeps its `u64` shape from the
            // native lift.
            lnum: d.lnum as u64,
            severity: Severity::from_snapshot(d.severity),
            message: d.message.clone(),
        }
    }
}

/// Specialised payload for the diagnostics popover. Lives inside the
/// shared `Popover<T>`, which handles open/close + fade.
pub struct DiagnosticsContent {
    items: Vec<PopupItem>,
    /// Authoritative count reported for the selected severity. The row
    /// payload can be bounded independently, so `items.len()` is not always
    /// the number shown by the status-line pill.
    total_count: u64,
    pill: DiagnosticPill,
    selected: usize,
    scroll_offset: usize,
    /// Per-row horizontal scroll, in logical pixels. Indexed by absolute
    /// item index so wheel scrolling survives row scrolling.
    msg_scroll: Vec<f32>,
    /// Cached row geometry from the last render so wheel-while-hover
    /// can map mouse y → row index. `(abs_idx, top_y, bottom_y, msg_max_width)`.
    row_layout: Vec<(usize, f32, f32, f32)>,
}

impl Default for DiagnosticsContent {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total_count: 0,
            pill: DiagnosticPill::Error,
            selected: 0,
            scroll_offset: 0,
            msg_scroll: Vec::new(),
            row_layout: Vec::new(),
        }
    }
}

pub struct DiagnosticsPopup {
    popover: Popover<DiagnosticsContent>,
    anchor_x: f32,
    anchor_y: f32,
    scale: f32,
    /// Last-render rect cached for outside-click hit-testing.
    last_rect: (f32, f32, f32, f32),
}

impl DiagnosticsPopup {
    pub fn new() -> Self {
        Self {
            popover: Popover::new(DiagnosticsContent::default()).with_anim_ms(ANIM_MS),
            anchor_x: 0.0,
            anchor_y: 0.0,
            scale: 1.0,
            last_rect: (0.0, 0.0, 0.0, 0.0),
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }

    pub fn is_interactive(&self) -> bool {
        self.popover.is_interactive()
    }

    pub fn is_animating(&self) -> bool {
        self.popover.is_animating()
    }

    pub fn open(
        &mut self,
        pill: DiagnosticPill,
        items: Vec<PopupItem>,
        anchor_x: f32,
        anchor_y: f32,
    ) {
        let total_count = items.len().min(u64::MAX as usize) as u64;
        self.open_with_total(pill, items, total_count, anchor_x, anchor_y);
    }

    /// Open with the authoritative severity total from the same diagnostics
    /// snapshot that populated the status-line pill. `items` may contain only
    /// a bounded subset; keeping the two values separate prevents a popup
    /// header such as "Errors — 100" beneath a pill that correctly says 137.
    pub fn open_with_total(
        &mut self,
        pill: DiagnosticPill,
        items: Vec<PopupItem>,
        total_count: u64,
        anchor_x: f32,
        anchor_y: f32,
    ) {
        let content = self.popover.content_mut();
        content.pill = pill;
        content.total_count = total_count.max(items.len().min(u64::MAX as usize) as u64);
        content.msg_scroll = vec![0.0; items.len()];
        content.row_layout.clear();
        content.items = items;
        content.selected = 0;
        content.scroll_offset = 0;
        self.anchor_x = anchor_x;
        self.anchor_y = anchor_y;
        self.popover
            .set_anchor(PopoverAnchor::Point([anchor_x, anchor_y]));
        self.popover.open();
    }

    pub fn pill(&self) -> DiagnosticPill {
        self.popover.content().pill
    }

    pub fn close(&mut self) {
        self.popover.close();
    }

    pub fn refresh_items(&mut self, items: Vec<PopupItem>) {
        let total_count = items.len().min(u64::MAX as usize) as u64;
        self.refresh_items_with_total(items, total_count);
    }

    /// Refresh the visible rows and authoritative severity total atomically.
    /// The popup never paints a new count above stale rows (or vice versa).
    pub fn refresh_items_with_total(&mut self, items: Vec<PopupItem>, total_count: u64) {
        if !self.is_visible() {
            return;
        }
        let content = self.popover.content_mut();
        let prev_lnum = content.items.get(content.selected).map(|i| i.lnum);
        content.items = items;
        content.total_count =
            total_count.max(content.items.len().min(u64::MAX as usize) as u64);
        content.msg_scroll = vec![0.0; content.items.len()];
        content.row_layout.clear();
        if let Some(prev) = prev_lnum {
            content.selected = content
                .items
                .iter()
                .position(|i| i.lnum == prev)
                .unwrap_or(0);
        } else {
            content.selected = 0;
        }
        if content.selected >= content.items.len() {
            content.selected = content.items.len().saturating_sub(1);
        }
        let visible = content.items.len().min(MAX_VISIBLE_ROWS);
        content.scroll_offset = content
            .scroll_offset
            .min(content.items.len().saturating_sub(visible));
    }

    /// Advance the underlying overlay animation + clear items when the
    /// close fade finishes. Idempotent.
    pub fn tick(&mut self) {
        let was_visible = self.popover.is_visible();
        self.popover.tick(0.0);
        if was_visible && !self.popover.is_visible() {
            let content = self.popover.content_mut();
            content.items.clear();
            content.row_layout.clear();
            self.last_rect = (0.0, 0.0, 0.0, 0.0);
        }
    }

    pub fn move_up(&mut self) {
        let content = self.popover.content_mut();
        if content.selected > 0 {
            content.selected -= 1;
            if content.selected < content.scroll_offset {
                content.scroll_offset = content.selected;
            }
        }
    }

    pub fn move_down(&mut self) {
        let content = self.popover.content_mut();
        if content.selected + 1 < content.items.len() {
            content.selected += 1;
            if content.selected >= content.scroll_offset + MAX_VISIBLE_ROWS {
                content.scroll_offset = content.selected + 1 - MAX_VISIBLE_ROWS;
            }
        }
    }

    pub fn scroll_by(&mut self, delta_rows: i32) {
        let content = self.popover.content_mut();
        if content.items.len() <= MAX_VISIBLE_ROWS {
            return;
        }
        let max_offset = content.items.len() - MAX_VISIBLE_ROWS;
        let new = (content.scroll_offset as i32 + delta_rows).clamp(0, max_offset as i32)
            as usize;
        content.scroll_offset = new;
    }

    pub fn contains_point(&self, mx: f32, my: f32) -> bool {
        let (x, y, w, h) = self.last_rect;
        w > 0.0 && h > 0.0 && mx >= x && mx <= x + w && my >= y && my <= y + h
    }

    pub fn row_at_y(&self, my: f32) -> Option<usize> {
        for &(idx, top, bot, _) in &self.popover.content().row_layout {
            if my >= top && my <= bot {
                return Some(idx);
            }
        }
        None
    }

    pub fn scroll_message(&mut self, abs_idx: usize, delta_x: f32) {
        let content = self.popover.content_mut();
        if abs_idx >= content.msg_scroll.len() {
            return;
        }
        let Some(&(_, _, _, msg_max)) = content
            .row_layout
            .iter()
            .find(|(idx, _, _, _)| *idx == abs_idx)
        else {
            return;
        };
        let prev = content.msg_scroll[abs_idx];
        let approx_msg_w = content
            .items
            .get(abs_idx)
            .map(|i| i.message.chars().count() as f32 * 8.0)
            .unwrap_or(0.0);
        let max = (approx_msg_w - msg_max).max(0.0);
        content.msg_scroll[abs_idx] = (prev + delta_x).clamp(0.0, max);
    }

    pub fn selected_lnum(&self) -> Option<u64> {
        let content = self.popover.content();
        content.items.get(content.selected).map(|i| i.lnum)
    }

    pub fn hit_test(&self, mx: f32, my: f32) -> Result<Option<usize>, ()> {
        let (x, y, w, h) = self.last_rect;
        if w <= 0.0 || h <= 0.0 || mx < x || mx > x + w || my < y || my > y + h {
            return Err(());
        }
        let s = self.scale;
        let pad = POPUP_PADDING * s;
        let rows_y0 = y + pad + HEADER_HEIGHT * s + 6.0 * s;
        let row_h = ROW_HEIGHT * s + ROW_GAP * s;
        if my < rows_y0 {
            return Ok(None);
        }
        let row = ((my - rows_y0) / row_h) as usize;
        let content = self.popover.content();
        let visible = content.items.len().min(MAX_VISIBLE_ROWS);
        if row >= visible {
            return Ok(None);
        }
        let abs = content.scroll_offset + row;
        if abs < content.items.len() {
            Ok(Some(abs))
        } else {
            Ok(None)
        }
    }

    pub fn set_selected_index(&mut self, idx: usize) {
        let content = self.popover.content_mut();
        if idx < content.items.len() {
            content.selected = idx;
            if idx < content.scroll_offset {
                content.scroll_offset = idx;
            } else if idx >= content.scroll_offset + MAX_VISIBLE_ROWS {
                content.scroll_offset = idx + 1 - MAX_VISIBLE_ROWS;
            }
        }
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        window_width: f32,
        scale_factor: f32,
        theme: &IdeTheme,
    ) {
        self.tick();
        if !self.is_visible() {
            return;
        }
        let s = self.scale;
        let _ = scale_factor;
        let pad = POPUP_PADDING * s;
        let radius = CORNER_RADIUS * s;
        let header_h = HEADER_HEIGHT * s;
        let row_h = ROW_HEIGHT * s + ROW_GAP * s;
        let eased = self.popover.anim_t();
        let lift = (1.0 - eased) * 6.0 * s;
        let alpha = eased;

        // Snapshot the slice of content we need before we hand
        // `sugarloaf` a mutable borrow — render path measures glyphs
        // through `sugarloaf.text_mut()`, which can't run while we're
        // also borrowing `self.popover.content_mut()` later. We rebuild
        // row_layout into a local Vec and push it back once we're done.
        let (pill, items_len, total_count, scroll_offset, selected, msg_scroll_snapshot) = {
            let c = self.popover.content();
            (
                c.pill,
                c.items.len(),
                c.total_count,
                c.scroll_offset,
                c.selected,
                c.msg_scroll.clone(),
            )
        };
        let visible = items_len.min(MAX_VISIBLE_ROWS);
        let rows_h = visible as f32 * row_h;
        let payload_bounded = total_count > items_len.min(u64::MAX as usize) as u64;
        let footer_hint = items_len > visible || payload_bounded;
        let footer_h = if footer_hint { 16.0 * s } else { 0.0 };
        let inner_h = header_h + 6.0 * s + rows_h + footer_h;
        let total_h = inner_h + pad * 2.0;
        let total_w = POPUP_WIDTH * s;

        // Position: centered horizontally on the pill, just above it.
        // Diagnostics popups always anchor above the pill, so we keep
        // the explicit math here rather than going through Popover's
        // resolve_position — the pill is a *point* anchor and there's
        // no flip case to handle.
        let x = (self.anchor_x - total_w / 2.0)
            .max(8.0 * s)
            .min(window_width - total_w - 8.0 * s);
        let y = (self.anchor_y - total_h - POPUP_GAP * s).max(8.0 * s);
        self.last_rect = (x, y, total_w, total_h);

        sugarloaf.rounded_rect(
            None,
            x - 2.0 * s,
            y - 2.0 * s + lift,
            total_w + 4.0 * s,
            total_h + 4.0 * s,
            theme.f32_alpha(theme.black, 0.45 * alpha),
            DEPTH_BG - 0.001,
            radius + 1.5 * s,
            ORDER,
        );
        sugarloaf.rounded_rect(
            None,
            x,
            y + lift,
            total_w,
            total_h,
            theme.f32_alpha(theme.surface, 0.98 * alpha),
            DEPTH_BG,
            radius,
            ORDER,
        );

        let accent_color = match pill {
            DiagnosticPill::Error => theme.f32_alpha(theme.red, alpha),
            DiagnosticPill::Warn => theme.f32_alpha(theme.yellow, alpha),
        };
        sugarloaf.rounded_rect(
            None,
            x,
            y + lift,
            3.0 * s,
            total_h,
            accent_color,
            DEPTH_TEXT,
            radius,
            ORDER + 1,
        );

        let header_color = match pill {
            DiagnosticPill::Error => theme.u8_alpha(theme.red, alpha),
            DiagnosticPill::Warn => theme.u8_alpha(theme.yellow, alpha),
        };
        let header_clip = [x + pad, y + lift + pad, total_w - pad * 2.0, header_h];
        let header_opts = DrawOpts {
            font_size: FONT_HEADER * s,
            color: header_color,
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        let header_label = match pill {
            DiagnosticPill::Error => format!("  Errors — {total_count}"),
            DiagnosticPill::Warn => format!("  Warnings — {total_count}"),
        };
        sugarloaf.text_mut().draw(
            x + pad + 4.0 * s,
            y + lift + pad,
            header_label.as_str(),
            &header_opts,
        );

        let body_x = x + pad + 4.0 * s;
        let mut row_y = y + lift + pad + header_h + 6.0 * s;
        let mut new_layout: Vec<(usize, f32, f32, f32)> = Vec::with_capacity(visible);

        // Collect rendered items by index — borrowing the slice once is
        // fine because we don't mutate content during the loop. We
        // clone the items so we can hand sugarloaf a `&mut` without
        // running afoul of split borrows on `self.popover`.
        let items_slice: Vec<PopupItem> = {
            let c = self.popover.content();
            c.items
                .iter()
                .skip(scroll_offset)
                .take(visible)
                .cloned()
                .collect()
        };

        for (offset, item) in items_slice.iter().enumerate() {
            let abs_idx = scroll_offset + offset;
            let is_selected = abs_idx == selected;
            let row_clip = [x + 8.0 * s, row_y, total_w - 16.0 * s, ROW_HEIGHT * s];
            if is_selected {
                let bg = theme.f32_alpha(theme.hover, alpha);
                sugarloaf.rounded_rect(
                    None,
                    x + 8.0 * s,
                    row_y,
                    total_w - 16.0 * s,
                    ROW_HEIGHT * s,
                    bg,
                    DEPTH_TEXT,
                    4.0 * s,
                    ORDER + 1,
                );
            }

            let glyph_color = item.severity.color(theme);
            let glyph_opts = DrawOpts {
                font_size: FONT_BODY * s,
                color: with_alpha(glyph_color, alpha),
                bold: true,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            };
            let glyph = item.severity.glyph();
            let glyph_w = sugarloaf.text_mut().measure(glyph, &glyph_opts);

            let lnum_text = format!("{:>4}", item.lnum);
            let lnum_opts = DrawOpts {
                font_size: FONT_BODY * s,
                color: theme.u8_alpha(theme.muted, alpha),
                bold: true,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            };
            let lnum_w = sugarloaf.text_mut().measure(&lnum_text, &lnum_opts);

            let msg_opts = DrawOpts {
                font_size: FONT_BODY * s,
                color: with_alpha(glyph_color, alpha),
                bold: is_selected,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            };

            let row_text_y = row_y + (ROW_HEIGHT * s - FONT_BODY * s) / 2.0;
            let glyph_x = body_x;
            let lnum_x = glyph_x + glyph_w + 8.0 * s;
            let msg_x = lnum_x + lnum_w + 10.0 * s;

            sugarloaf
                .text_mut()
                .draw(glyph_x, row_text_y, glyph, &glyph_opts);
            sugarloaf
                .text_mut()
                .draw(lnum_x, row_text_y, lnum_text.as_str(), &lnum_opts);

            let msg_max = (x + total_w - pad - 4.0 * s) - msg_x;
            let row_scroll = msg_scroll_snapshot.get(abs_idx).copied().unwrap_or(0.0);
            let visible_msg = slice_visible(
                sugarloaf,
                item.message.as_str(),
                &msg_opts,
                row_scroll,
                msg_max,
            );
            sugarloaf
                .text_mut()
                .draw(msg_x, row_text_y, visible_msg.as_str(), &msg_opts);

            new_layout.push((abs_idx, row_y, row_y + ROW_HEIGHT * s, msg_max));
            row_y += row_h;
        }

        if footer_hint {
            let footer_clip = [
                x + pad,
                y + lift + total_h - pad - 18.0 * s,
                total_w - pad * 2.0,
                18.0 * s,
            ];
            let hint_opts = DrawOpts {
                font_size: FONT_HINT * s,
                color: theme.u8_alpha(theme.muted, alpha),
                clip_rect: Some(footer_clip),
                ..DrawOpts::default()
            };
            let footer = if payload_bounded {
                format!("Showing {items_len} of {total_count} received diagnostics")
            } else {
                let extra = items_len - visible;
                format!("+{extra} more — scroll to see all")
            };
            let hint_y = y + lift + total_h - pad - FONT_HINT * s;
            sugarloaf.text_mut().draw(
                x + pad + 4.0 * s,
                hint_y,
                footer.as_str(),
                &hint_opts,
            );
        }

        self.popover.content_mut().row_layout = new_layout;
    }
}

impl Default for DiagnosticsPopup {
    fn default() -> Self {
        Self::new()
    }
}

fn with_alpha(mut c: [u8; 4], alpha: f32) -> [u8; 4] {
    c[3] = (255.0 * alpha.clamp(0.0, 1.0)) as u8;
    c
}

fn slice_visible(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
    scroll_x: f32,
    max_w: f32,
) -> String {
    if scroll_x <= 0.5 {
        return trim_to_fit(sugarloaf, text, opts, max_w);
    }

    let chars: Vec<char> = text.chars().collect();
    let ui = sugarloaf.text_mut();
    let mut buf = [0u8; 4];

    let mut acc = 0.0_f32;
    let mut start = 0;
    for (i, &c) in chars.iter().enumerate() {
        let w = ui.measure(c.encode_utf8(&mut buf), opts);
        acc += w;
        if acc > scroll_x {
            start = i;
            break;
        }
        start = i + 1;
    }

    if start >= chars.len() {
        return "…".to_string();
    }

    let lead = "…";
    let trail = "…";
    let lead_w = ui.measure(lead, opts);
    let trail_w = ui.measure(trail, opts);

    let target = (max_w - lead_w).max(0.0);
    let mut taken_w = 0.0;
    let mut end = start;
    let mut overflow = false;
    for i in start..chars.len() {
        let w = ui.measure(chars[i].encode_utf8(&mut buf), opts);
        if taken_w + w > target {
            overflow = true;
            break;
        }
        taken_w += w;
        end = i + 1;
    }

    let mut out = String::new();
    out.push_str(lead);
    out.extend(chars[start..end].iter());
    if overflow && taken_w + trail_w <= target + lead_w {
        let mut shave_w = 0.0;
        let mut shaved_end = end;
        while shaved_end > start && taken_w - shave_w + trail_w > target - lead_w + lead_w
        {
            let c = chars[shaved_end - 1];
            shave_w += ui.measure(c.encode_utf8(&mut buf), opts);
            shaved_end -= 1;
        }
        out = String::new();
        out.push_str(lead);
        out.extend(chars[start..shaved_end].iter());
        out.push_str(trail);
    }
    out
}

fn trim_to_fit(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
    max_w: f32,
) -> String {
    let ui = sugarloaf.text_mut();
    if ui.measure(text, opts) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let ellipsis = "…";
    let ellipsis_w = ui.measure(ellipsis, opts);
    let target = (max_w - ellipsis_w).max(0.0);
    let mut lo = 0;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate: String = chars[..mid].iter().collect();
        let w = ui.measure(candidate.as_str(), opts);
        if w <= target {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out: String = chars[..lo].iter().collect();
    out.push_str(ellipsis);
    out
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticsPopup, PopupItem, Severity};
    use crate::panels::status_line::DiagnosticPill;

    fn item(lnum: u64) -> PopupItem {
        PopupItem {
            lnum,
            severity: Severity::Error,
            message: format!("error on line {lnum}"),
        }
    }

    #[test]
    fn authoritative_total_is_independent_from_bounded_rows() {
        let mut popup = DiagnosticsPopup::new();
        popup.open_with_total(
            DiagnosticPill::Error,
            vec![item(1), item(2)],
            102,
            100.0,
            100.0,
        );

        let content = popup.popover.content();
        assert_eq!(content.items.len(), 2);
        assert_eq!(content.total_count, 102);
    }

    #[test]
    fn refresh_updates_rows_and_total_as_one_snapshot() {
        let mut popup = DiagnosticsPopup::new();
        popup.open_with_total(
            DiagnosticPill::Warn,
            vec![item(7), item(8)],
            12,
            100.0,
            100.0,
        );
        popup.refresh_items_with_total(vec![item(8)], 5);

        let content = popup.popover.content();
        assert_eq!(content.items.len(), 1);
        assert_eq!(content.total_count, 5);
    }

    #[test]
    fn total_never_claims_fewer_diagnostics_than_received_rows() {
        let mut popup = DiagnosticsPopup::new();
        popup.open_with_total(
            DiagnosticPill::Error,
            vec![item(1), item(2)],
            0,
            100.0,
            100.0,
        );

        assert_eq!(popup.popover.content().total_count, 2);
    }
}
