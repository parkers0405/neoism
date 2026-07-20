//! Floating LSP details popup anchored to the status-line LSP pill.
//!
//! This is an overlay panel, not an editor popup. It uses Sugarloaf's
//! late overlay primitives and an opaque frame so editor text behind it
//! cannot show through. The panel shows every LSP candidate for the
//! active buffer, then a larger details area for the selected server.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

use crate::panels::status_line::{DiagnosticCounts, LspStatus, PillRect};
use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::scrollbar;

const DEPTH_BG: f32 = 0.08;
const DEPTH_ELEMENT: f32 = 0.16;
const ORDER: u8 = 19;

const PANEL_RADIUS: f32 = 8.0;
const HEADER_HEIGHT: f32 = 30.0;
const ROW_HEIGHT: f32 = 38.0;
const DETAIL_HEIGHT: f32 = 260.0;
const HORIZONTAL_PAD: f32 = 12.0;
const VERTICAL_PAD: f32 = 8.0;
const PANEL_MIN_WIDTH: f32 = 460.0;
const PANEL_MAX_WIDTH: f32 = 640.0;
const MAX_VISIBLE_ROWS: usize = 5;
const GAP: f32 = 6.0;

/// Per-server state badge. Drives the dot color + label suffix in
/// every popup row. Mirrors the snapshot states the lua side emits
/// (`active` / `initializing` / `missing` / `errored`) plus a local
/// `Disabled` state for future enable/disable wiring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspServerState {
    Active,
    Ready,
    Initializing,
    Missing,
    Errored,
    Disabled,
}

impl Default for LspServerState {
    fn default() -> Self {
        LspServerState::Missing
    }
}

impl LspServerState {
    pub fn from_str(s: &str) -> Self {
        match s {
            // "attached" is the Rust engine's live-client state; without it the
            // popup fell through to the catch-all and mislabeled a working
            // server as "Binary missing".
            "active" | "daemon" | "attached" => LspServerState::Active,
            "ready" | "configured" | "available" => LspServerState::Ready,
            "initializing" | "starting" => LspServerState::Initializing,
            "errored" | "error" | "failed" => LspServerState::Errored,
            "disabled" => LspServerState::Disabled,
            _ => LspServerState::Missing,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LspServerState::Active => "Attached",
            LspServerState::Ready => "Available",
            LspServerState::Initializing => "Starting",
            LspServerState::Missing => "Binary missing",
            LspServerState::Errored => "Errored",
            LspServerState::Disabled => "Disabled",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LspServerRow {
    /// Human-facing server name, e.g. `rust-analyzer`.
    pub name: String,
    /// Optional binary basename or absolute managed binary path.
    pub binary: Option<String>,
    /// Filetype the server is registered for.
    pub filetype: Option<String>,
    /// Lifecycle state.
    pub state: LspServerState,
    /// Last `vim.notify` text attributed to this server.
    pub message: Option<String>,
    /// Level of `message`: "info" / "warn" / "error".
    pub level: Option<String>,
    /// Runtime source: "managed", "path", "missing", or "unknown".
    pub source: Option<String>,
    /// Diagnostics attributed to this server for the active buffer.
    pub diagnostics: DiagnosticCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspPopupClickAction {
    Consumed,
    CopyMessage(String),
}

pub struct LspPopup {
    visible: bool,
    anchor: PillRect,
    servers: Vec<LspServerRow>,
    status: Option<LspStatus>,
    diagnostics: DiagnosticCounts,
    buffer_label: Option<String>,
    selected_index: usize,
    hovered_index: Option<usize>,
    scroll_offset: usize,
    wheel_accumulator: f32,
    detail_scroll_offset: usize,
    detail_wheel_accumulator: f32,
    last_scroll: Option<Instant>,
    detail_last_scroll: Option<Instant>,
    row_layout: Vec<(usize, [f32; 4])>,
    message_copy_rect: [f32; 4],
    details_rect: [f32; 4],
    last_rect: [f32; 4],
}

impl Default for LspPopup {
    fn default() -> Self {
        Self {
            visible: false,
            anchor: PillRect::default(),
            servers: Vec::new(),
            status: None,
            diagnostics: DiagnosticCounts::default(),
            buffer_label: None,
            selected_index: 0,
            hovered_index: None,
            scroll_offset: 0,
            wheel_accumulator: 0.0,
            detail_scroll_offset: 0,
            detail_wheel_accumulator: 0.0,
            last_scroll: None,
            detail_last_scroll: None,
            row_layout: Vec::new(),
            message_copy_rect: [0.0; 4],
            details_rect: [0.0; 4],
            last_rect: [0.0; 4],
        }
    }
}

impl LspPopup {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rect for the host's text-occlusion registry.
    pub fn occlusion_rect(&self) -> Option<[f32; 4]> {
        if !self.is_visible() || self.last_rect[2] <= 0.0 {
            return None;
        }
        Some(self.last_rect)
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    pub fn open(&mut self, anchor: PillRect) {
        self.visible = true;
        self.anchor = anchor;
        self.clamp_state();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.hovered_index = None;
        self.detail_scroll_offset = 0;
        self.detail_wheel_accumulator = 0.0;
    }

    pub fn set_servers(&mut self, servers: Vec<LspServerRow>) {
        let previous_selected = self
            .servers
            .get(self.selected_index)
            .map(|server| server.name.clone());
        let selected_name = self
            .servers
            .get(self.selected_index)
            .map(|server| server.name.clone());
        self.servers = servers;
        if let Some(selected_name) = selected_name {
            if let Some(index) = self
                .servers
                .iter()
                .position(|server| server.name == selected_name)
            {
                self.selected_index = index;
            }
        }
        self.clamp_state();
        let current_selected = self
            .servers
            .get(self.selected_index)
            .map(|server| server.name.clone());
        if previous_selected != current_selected {
            self.detail_scroll_offset = 0;
            self.detail_wheel_accumulator = 0.0;
        }
    }

    pub fn set_status(&mut self, status: Option<LspStatus>) {
        self.status = status;
    }

    pub fn set_diagnostics(&mut self, counts: DiagnosticCounts) {
        self.diagnostics = counts;
    }

    pub fn set_buffer_label(&mut self, label: Option<String>) {
        self.buffer_label = label;
    }

    pub fn contains_point(&self, x: f32, y: f32, scale: f32) -> bool {
        if !self.visible {
            return false;
        }
        let rect = if self.last_rect[2] > 0.0 {
            self.last_rect
        } else {
            self.panel_rect(scale)
        };
        x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
    }

    pub fn hover(&mut self, x: f32, y: f32, scale: f32) -> bool {
        if !self.visible {
            return false;
        }
        let previous_hover = self.hovered_index;
        let next = self
            .row_at_point(x, y)
            .filter(|_| self.contains_point(x, y, scale));
        self.hovered_index = next;
        previous_hover != self.hovered_index
    }

    pub fn click(&mut self, x: f32, y: f32, scale: f32) -> Option<LspPopupClickAction> {
        if !self.contains_point(x, y, scale) {
            return None;
        }
        if point_in_rect(x, y, self.message_copy_rect) {
            if let Some(message) = self
                .selected_server()
                .and_then(|server| server.message.as_deref())
                .filter(|message| !message.is_empty())
            {
                return Some(LspPopupClickAction::CopyMessage(message.to_string()));
            }
        }
        if let Some(index) = self.row_at_point(x, y) {
            let previous = self.selected_index;
            self.selected_index = index.min(self.servers.len().saturating_sub(1));
            self.ensure_selected_visible();
            if self.selected_index != previous {
                self.detail_scroll_offset = 0;
                self.detail_wheel_accumulator = 0.0;
            }
        }
        Some(LspPopupClickAction::Consumed)
    }

    pub fn scroll_at(&mut self, x: f32, y: f32, delta_pixels: f32, scale: f32) -> bool {
        if !self.contains_point(x, y, scale) {
            return false;
        }
        if point_in_rect(x, y, self.details_rect) {
            return self.scroll_detail_pixels(delta_pixels, scale);
        }
        self.scroll_pixels(delta_pixels)
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32) -> bool {
        if !self.visible || self.servers.len() <= MAX_VISIBLE_ROWS || delta_pixels == 0.0
        {
            return false;
        }
        let row_h = ROW_HEIGHT.max(1.0);
        self.wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.wheel_accumulator.abs() >= row_h {
            let sign = self.wheel_accumulator.signum();
            self.wheel_accumulator -= sign * row_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows == 0 {
            return false;
        }
        let old = self.scroll_offset;
        let max_offset = self.max_scroll_offset();
        if rows < 0 {
            self.scroll_offset = self
                .scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize);
        } else {
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset);
        }
        self.ensure_selected_visible();
        self.last_scroll = Some(Instant::now());
        old != self.scroll_offset
    }

    fn scroll_detail_pixels(&mut self, delta_pixels: f32, scale: f32) -> bool {
        if !self.visible || delta_pixels == 0.0 {
            return false;
        }
        let max_offset = self.max_detail_scroll_offset(scale);
        if max_offset == 0 {
            return false;
        }
        let line_h = (15.0 * scale.max(0.5)).max(1.0);
        self.detail_wheel_accumulator += delta_pixels;
        let mut rows = 0i32;
        while self.detail_wheel_accumulator.abs() >= line_h {
            let sign = self.detail_wheel_accumulator.signum();
            self.detail_wheel_accumulator -= sign * line_h;
            rows += if sign > 0.0 { -1 } else { 1 };
        }
        if rows == 0 {
            return false;
        }
        let old = self.detail_scroll_offset;
        if rows < 0 {
            self.detail_scroll_offset = self
                .detail_scroll_offset
                .saturating_sub(rows.unsigned_abs() as usize);
        } else {
            self.detail_scroll_offset = self
                .detail_scroll_offset
                .saturating_add(rows as usize)
                .min(max_offset);
        }
        self.detail_last_scroll = Some(Instant::now());
        old != self.detail_scroll_offset
    }

    pub fn render(&mut self, sugarloaf: &mut Sugarloaf, theme: &IdeTheme, scale: f32) {
        if !self.visible {
            self.row_layout.clear();
            self.message_copy_rect = [0.0; 4];
            self.details_rect = [0.0; 4];
            return;
        }
        self.clamp_state();
        let s = scale.clamp(0.5, 3.0);
        let panel = self.panel_rect(s);
        if panel[2] <= 0.0 || panel[3] <= 0.0 {
            return;
        }
        self.last_rect = panel;
        self.row_layout.clear();

        let [x, y, w, h] = panel;
        let clip = Some(panel);
        let pad_x = HORIZONTAL_PAD * s;
        let radius = PANEL_RADIUS * s;

        sugarloaf.overlay_rounded_rect(
            x - 2.0 * s,
            y - 2.0 * s,
            w + 4.0 * s,
            h + 4.0 * s,
            theme.f32_alpha(theme.black, 0.45),
            DEPTH_BG - 0.01,
            radius + 1.0 * s,
            ORDER,
        );
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            w,
            h,
            theme.f32(theme.surface),
            DEPTH_BG,
            radius,
            ORDER,
        );
        paint_outline(sugarloaf, panel, theme.f32(theme.border), s);

        let header_h = HEADER_HEIGHT * s;
        let header_y = y + (header_h - 12.0 * s) * 0.5;
        let title_opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let status_opts = DrawOpts {
            font_size: 11.0 * s,
            color: status_color(self.status, theme),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let glyph = crate::primitives::icons::lsp_glyph();
        sugarloaf
            .overlay_text_mut()
            .draw(x + pad_x, header_y, glyph, &status_opts);
        let glyph_w = sugarloaf.overlay_text_mut().measure(glyph, &status_opts);
        let title = self
            .buffer_label
            .as_deref()
            .filter(|label| !label.is_empty())
            .unwrap_or("LSP");
        let title_max = w - pad_x * 2.0 - glyph_w - 100.0 * s;
        let title =
            truncate_to_fit(title, title_max.max(40.0 * s), sugarloaf, &title_opts);
        sugarloaf.overlay_text_mut().draw(
            x + pad_x + glyph_w + 8.0 * s,
            header_y,
            &title,
            &title_opts,
        );

        let summary = match self.servers.len() {
            0 => "No servers".to_string(),
            1 => "1 server".to_string(),
            n => format!("{n} servers"),
        };
        let summary_w = sugarloaf.overlay_text_mut().measure(&summary, &status_opts);
        sugarloaf.overlay_text_mut().draw(
            x + w - pad_x - summary_w,
            header_y,
            &summary,
            &status_opts,
        );

        let sep_y = y + header_h;
        sugarloaf.overlay_rect(
            x + pad_x,
            sep_y,
            w - pad_x * 2.0,
            1.0_f32.max(s),
            theme.f32_alpha(theme.border, 0.75),
            DEPTH_ELEMENT,
            ORDER,
        );

        let list_top = sep_y + VERTICAL_PAD * s;
        let visible_rows = self.visible_rows();
        let row_h = ROW_HEIGHT * s;
        let list_h = row_h * visible_rows.max(1) as f32;
        let list_clip = [x + pad_x, list_top, w - pad_x * 2.0, list_h];
        if self.servers.is_empty() {
            self.draw_empty_state(sugarloaf, theme, list_clip, s);
        } else {
            self.draw_server_rows(sugarloaf, theme, list_clip, s);
        }

        if self.servers.len() > visible_rows {
            self.draw_scrollbar(sugarloaf, list_clip, visible_rows, s);
        }

        let details_top = list_top + list_h + GAP * s;
        let details = [x + pad_x, details_top, w - pad_x * 2.0, DETAIL_HEIGHT * s];
        self.details_rect = details;
        self.draw_details(sugarloaf, theme, details, s);
    }

    fn draw_empty_state(
        &self,
        sugarloaf: &mut Sugarloaf,
        theme: &IdeTheme,
        rect: [f32; 4],
        s: f32,
    ) {
        let label = match self.status {
            Some(LspStatus::Initializing) => "LSP is starting",
            Some(LspStatus::Missing) => "No LSP installed for this buffer",
            Some(LspStatus::Active) => "Attached, waiting for server snapshot",
            None => "No LSP candidates for this filetype",
        };
        let opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(rect),
            ..DrawOpts::default()
        };
        sugarloaf.overlay_text_mut().draw(
            rect[0],
            rect[1] + (rect[3] - 12.0 * s) * 0.5,
            label,
            &opts,
        );
    }

    fn draw_server_rows(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        theme: &IdeTheme,
        rect: [f32; 4],
        s: f32,
    ) {
        let [x, y, w, _] = rect;
        let row_h = ROW_HEIGHT * s;
        let name_opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.fg),
            clip_rect: Some(rect),
            ..DrawOpts::default()
        };
        let meta_opts = DrawOpts {
            font_size: 10.5 * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(rect),
            ..DrawOpts::default()
        };

        let end = (self.scroll_offset + self.visible_rows()).min(self.servers.len());
        for (slot, index) in (self.scroll_offset..end).enumerate() {
            let row_y = y + slot as f32 * row_h;
            let row_rect = [x, row_y, w, row_h];
            self.row_layout.push((index, row_rect));
            let selected = index == self.selected_index;
            let hovered = self.hovered_index == Some(index);
            if selected || hovered {
                sugarloaf.overlay_rounded_rect(
                    x,
                    row_y + 2.0 * s,
                    w,
                    row_h - 4.0 * s,
                    theme.f32_alpha(if selected { theme.hover } else { theme.bg }, 0.9),
                    DEPTH_ELEMENT,
                    5.0 * s,
                    ORDER,
                );
            }

            let server = &self.servers[index];
            let dot = 7.0 * s;
            let dot_x = x + 8.0 * s;
            let dot_y = row_y + (row_h - dot) * 0.5;
            sugarloaf.overlay_rounded_rect(
                dot_x,
                dot_y,
                dot,
                dot,
                state_color_f32(server.state, theme),
                DEPTH_ELEMENT,
                dot * 0.5,
                ORDER,
            );

            let text_x = dot_x + dot + 9.0 * s;
            let name_y = row_y + 6.0 * s;
            let badge = server.state.label();
            let badge_opts = DrawOpts {
                font_size: 10.5 * s,
                color: state_color(server.state, theme),
                bold: true,
                clip_rect: Some(rect),
                ..DrawOpts::default()
            };
            let badge_w = sugarloaf.overlay_text_mut().measure(badge, &badge_opts);
            let diag_text = diagnostics_inline(server.diagnostics);
            let diag_opts = DrawOpts {
                font_size: 10.5 * s,
                color: diagnostics_color(server.diagnostics, theme),
                bold: diagnostics_total(server.diagnostics) > 0,
                clip_rect: Some(rect),
                ..DrawOpts::default()
            };
            let diag_w = diag_text
                .as_ref()
                .map(|text| sugarloaf.overlay_text_mut().measure(text, &diag_opts))
                .unwrap_or(0.0);
            let right_reserved = badge_w + diag_w + 22.0 * s;
            let name_max = (x + w - right_reserved - text_x).max(40.0 * s);
            let name = truncate_to_fit(&server.name, name_max, sugarloaf, &name_opts);
            sugarloaf
                .overlay_text_mut()
                .draw(text_x, name_y, &name, &name_opts);

            let meta = row_meta(server);
            if !meta.is_empty() {
                let meta = truncate_to_fit(&meta, name_max, sugarloaf, &meta_opts);
                sugarloaf.overlay_text_mut().draw(
                    text_x,
                    row_y + row_h - 14.0 * s,
                    &meta,
                    &meta_opts,
                );
            }

            let badge_x = x + w - 8.0 * s - badge_w;
            sugarloaf
                .overlay_text_mut()
                .draw(badge_x, name_y, badge, &badge_opts);
            if let Some(diag_text) = diag_text {
                let diag_x = (badge_x - diag_w - 10.0 * s).max(text_x);
                sugarloaf.overlay_text_mut().draw(
                    diag_x,
                    row_y + row_h - 14.0 * s,
                    &diag_text,
                    &diag_opts,
                );
            }
        }
    }

    fn draw_details(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        theme: &IdeTheme,
        rect: [f32; 4],
        s: f32,
    ) {
        self.message_copy_rect = [0.0; 4];
        let [x, y, w, h] = rect;
        let clip = Some(rect);
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            w,
            h,
            theme.f32_alpha(theme.bg, 0.92),
            DEPTH_BG + 0.02,
            6.0 * s,
            ORDER,
        );

        let heading_opts = DrawOpts {
            font_size: 11.0 * s,
            color: theme.u8(theme.dim),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        sugarloaf.overlay_text_mut().draw(
            x + 10.0 * s,
            y + 8.0 * s,
            "Server Details",
            &heading_opts,
        );

        let Some(server) = self.selected_server().cloned() else {
            return;
        };
        if server
            .message
            .as_deref()
            .is_some_and(|message| !message.is_empty())
        {
            let copy_opts = DrawOpts {
                font_size: 10.5 * s,
                color: theme.u8(theme.fg),
                bold: true,
                clip_rect: clip,
                ..DrawOpts::default()
            };
            let copy_label = "\u{f0c5} Copy";
            let copy_w =
                sugarloaf.overlay_text_mut().measure(copy_label, &copy_opts) + 18.0 * s;
            let copy_h = 20.0 * s;
            let copy_x = x + w - copy_w - 8.0 * s;
            let copy_y = y + 5.0 * s;
            self.message_copy_rect = [copy_x, copy_y, copy_w, copy_h];
            sugarloaf.overlay_rounded_rect(
                copy_x,
                copy_y,
                copy_w,
                copy_h,
                theme.f32_alpha(theme.hover, 0.86),
                DEPTH_ELEMENT,
                5.0 * s,
                ORDER,
            );
            sugarloaf.overlay_text_mut().draw(
                copy_x + 9.0 * s,
                copy_y + 4.0 * s,
                copy_label,
                &copy_opts,
            );
        }
        let label_opts = DrawOpts {
            font_size: 10.5 * s,
            color: theme.u8(theme.muted),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let value_opts = DrawOpts {
            font_size: 10.5 * s,
            color: theme.u8(theme.fg),
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let mut row_y = y + 30.0 * s;
        let value_x = x + 92.0 * s;
        let value_w = (w - (value_x - x) - 12.0 * s).max(40.0 * s);
        draw_label_value(
            sugarloaf,
            x + 10.0 * s,
            value_x,
            row_y,
            value_w,
            "Name",
            &server.name,
            &label_opts,
            &value_opts,
        );
        row_y += 18.0 * s;
        draw_label_value(
            sugarloaf,
            x + 10.0 * s,
            value_x,
            row_y,
            value_w,
            "State",
            server.state.label(),
            &label_opts,
            &value_opts,
        );
        row_y += 18.0 * s;
        draw_label_value(
            sugarloaf,
            x + 10.0 * s,
            value_x,
            row_y,
            value_w,
            "Binary",
            server.binary.as_deref().unwrap_or("not reported"),
            &label_opts,
            &value_opts,
        );
        row_y += 18.0 * s;
        draw_label_value(
            sugarloaf,
            x + 10.0 * s,
            value_x,
            row_y,
            value_w,
            "Source",
            source_label(server.source.as_deref()),
            &label_opts,
            &value_opts,
        );
        row_y += 18.0 * s;
        draw_label_value(
            sugarloaf,
            x + 10.0 * s,
            value_x,
            row_y,
            value_w,
            "Filetype",
            server.filetype.as_deref().unwrap_or("not reported"),
            &label_opts,
            &value_opts,
        );
        row_y += 18.0 * s;
        let diag_value = diagnostics_details(server.diagnostics);
        let diag_value_opts = DrawOpts {
            font_size: 10.5 * s,
            color: diagnostics_color(server.diagnostics, theme),
            bold: diagnostics_total(server.diagnostics) > 0,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        draw_label_value(
            sugarloaf,
            x + 10.0 * s,
            value_x,
            row_y,
            value_w,
            "Diagnostics",
            &diag_value,
            &label_opts,
            &diag_value_opts,
        );

        if let Some(message) = server.message.as_deref().filter(|msg| !msg.is_empty()) {
            row_y += 22.0 * s;
            sugarloaf.overlay_text_mut().draw(
                x + 10.0 * s,
                row_y,
                "Message",
                &label_opts,
            );
            let message_x = value_x;
            let line_h = 15.0 * s;
            let message_clip = [
                message_x,
                row_y - 2.0 * s,
                value_w,
                (y + h - row_y - 8.0 * s).max(line_h),
            ];
            let max_chars = ((value_w / (6.3 * s)).floor() as usize).max(20);
            let lines = wrap_text(message, max_chars, usize::MAX);
            let visible_lines = ((message_clip[3] / line_h).floor() as usize).max(1);
            let max_offset = lines.len().saturating_sub(visible_lines);
            let offset = self.detail_scroll_offset.min(max_offset);
            let message_opts = DrawOpts {
                font_size: 10.5 * s,
                color: message_color(server.level.as_deref(), theme),
                clip_rect: Some(message_clip),
                ..DrawOpts::default()
            };
            for line in lines.iter().skip(offset).take(visible_lines) {
                sugarloaf
                    .overlay_text_mut()
                    .draw(message_x, row_y, line, &message_opts);
                row_y += line_h;
            }
            if lines.len() > visible_lines {
                self.draw_detail_scrollbar(
                    sugarloaf,
                    message_clip,
                    lines.len(),
                    visible_lines,
                    offset,
                    s,
                );
            }
        }
    }

    fn draw_scrollbar(
        &self,
        sugarloaf: &mut Sugarloaf,
        list_clip: [f32; 4],
        visible_rows: usize,
        s: f32,
    ) {
        if self.servers.is_empty() || visible_rows == 0 {
            return;
        }
        let _ = s;
        let track_h = (list_clip[3] - scrollbar::SCROLLBAR_MARGIN * 2.0).max(1.0);
        let ratio = visible_rows as f32 / self.servers.len() as f32;
        let thumb_h = (track_h * ratio).clamp(scrollbar::min_thumb_height(), track_h);
        let max_offset = self.max_scroll_offset().max(1);
        let offset_ratio = self.scroll_offset as f32 / max_offset as f32;
        let thumb_y = list_clip[1]
            + scrollbar::SCROLLBAR_MARGIN
            + offset_ratio * (track_h - thumb_h);
        let thumb_x = list_clip[0] + list_clip[2] - scrollbar::width() - 2.0;
        let opacity =
            scrollbar::opacity_from_last_scroll(self.last_scroll, false).max(0.55);
        scrollbar::draw_track_overlay(
            sugarloaf,
            thumb_x,
            list_clip[1] + scrollbar::SCROLLBAR_MARGIN,
            track_h,
            opacity,
            DEPTH_ELEMENT + 0.02,
            ORDER,
        );
        scrollbar::draw_thumb_overlay(
            sugarloaf,
            thumb_x,
            thumb_y,
            thumb_h,
            opacity,
            false,
            DEPTH_ELEMENT + 0.02,
            ORDER,
        );
    }

    fn draw_detail_scrollbar(
        &self,
        sugarloaf: &mut Sugarloaf,
        rect: [f32; 4],
        total_lines: usize,
        visible_lines: usize,
        offset: usize,
        s: f32,
    ) {
        if total_lines <= visible_lines || visible_lines == 0 {
            return;
        }
        let _ = s;
        let track_h = (rect[3] - scrollbar::SCROLLBAR_MARGIN * 2.0).max(1.0);
        let ratio = visible_lines as f32 / total_lines as f32;
        let thumb_h = (track_h * ratio).clamp(scrollbar::min_thumb_height(), track_h);
        let max_offset = total_lines.saturating_sub(visible_lines).max(1);
        let offset_ratio = offset as f32 / max_offset as f32;
        let thumb_y =
            rect[1] + scrollbar::SCROLLBAR_MARGIN + offset_ratio * (track_h - thumb_h);
        let thumb_x = rect[0] + rect[2] - scrollbar::width() - 2.0;
        let opacity =
            scrollbar::opacity_from_last_scroll(self.detail_last_scroll, false).max(0.55);
        scrollbar::draw_track_overlay(
            sugarloaf,
            thumb_x,
            rect[1] + scrollbar::SCROLLBAR_MARGIN,
            track_h,
            opacity,
            DEPTH_ELEMENT + 0.02,
            ORDER,
        );
        scrollbar::draw_thumb_overlay(
            sugarloaf,
            thumb_x,
            thumb_y,
            thumb_h,
            opacity,
            false,
            DEPTH_ELEMENT + 0.02,
            ORDER,
        );
    }

    fn selected_server(&self) -> Option<&LspServerRow> {
        self.servers.get(self.selected_index)
    }

    fn row_at_point(&self, x: f32, y: f32) -> Option<usize> {
        self.row_layout.iter().find_map(|(index, rect)| {
            (x >= rect[0]
                && y >= rect[1]
                && x <= rect[0] + rect[2]
                && y <= rect[1] + rect[3])
                .then_some(*index)
        })
    }

    fn visible_rows(&self) -> usize {
        self.servers.len().clamp(1, MAX_VISIBLE_ROWS)
    }

    fn max_scroll_offset(&self) -> usize {
        self.servers.len().saturating_sub(MAX_VISIBLE_ROWS)
    }

    fn max_detail_scroll_offset(&self, scale: f32) -> usize {
        let Some(server) = self.selected_server() else {
            return 0;
        };
        let Some(message) = server.message.as_deref().filter(|msg| !msg.is_empty())
        else {
            return 0;
        };
        let s = scale.max(0.5);
        let value_x_offset = 92.0 * s;
        let value_w = (self.details_rect[2] - value_x_offset - 12.0 * s).max(40.0 * s);
        let max_chars = ((value_w / (6.3 * s)).floor() as usize).max(20);
        let lines = wrap_text(message, max_chars, usize::MAX);
        let visible_lines = self.detail_visible_lines(s);
        lines.len().saturating_sub(visible_lines)
    }

    fn detail_visible_lines(&self, scale: f32) -> usize {
        if self.details_rect[3] <= 0.0 {
            return 0;
        }
        let s = scale.max(0.5);
        let message_top_offset = (30.0 + 18.0 * 5.0 + 22.0) * s;
        let line_h = (15.0 * s).max(1.0);
        let available = (self.details_rect[3] - message_top_offset - 8.0 * s).max(line_h);
        ((available / line_h).floor() as usize).max(1)
    }

    fn ensure_selected_visible(&mut self) {
        let visible = self.visible_rows();
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + visible {
            self.scroll_offset = self.selected_index + 1 - visible;
        }
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }

    fn clamp_state(&mut self) {
        if self.servers.is_empty() {
            self.selected_index = 0;
            self.hovered_index = None;
            self.scroll_offset = 0;
            return;
        }
        self.selected_index = self.selected_index.min(self.servers.len() - 1);
        self.hovered_index = self
            .hovered_index
            .filter(|index| *index < self.servers.len());
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
        self.ensure_selected_visible();
    }

    fn panel_rect(&self, scale: f32) -> [f32; 4] {
        let s = scale.max(0.5);
        let visible_rows = self.visible_rows();
        let h = HEADER_HEIGHT * s
            + VERTICAL_PAD * 2.0 * s
            + ROW_HEIGHT * s * visible_rows.max(1) as f32
            + GAP * s
            + DETAIL_HEIGHT * s;
        let w_min = PANEL_MIN_WIDTH * s;
        let w_max = PANEL_MAX_WIDTH * s;
        let w = (self.anchor.w.max(w_min)).min(w_max);
        let gap = 7.0 * s;
        let x = (self.anchor.x + self.anchor.w - w).max(8.0 * s);
        let y = (self.anchor.y - h - gap).max(8.0 * s);
        [x, y, w, h]
    }
}

fn paint_outline(sugarloaf: &mut Sugarloaf, rect: [f32; 4], color: [f32; 4], s: f32) {
    let [x, y, w, h] = rect;
    let t = 1.0_f32.max(s);
    for slab in [
        [x, y, w, t],
        [x, y + h - t, w, t],
        [x, y, t, h],
        [x + w - t, y, t, h],
    ] {
        sugarloaf.overlay_rect(
            slab[0],
            slab[1],
            slab[2],
            slab[3],
            color,
            DEPTH_ELEMENT,
            ORDER,
        );
    }
}

fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    rect[2] > 0.0
        && rect[3] > 0.0
        && x >= rect[0]
        && y >= rect[1]
        && x <= rect[0] + rect[2]
        && y <= rect[1] + rect[3]
}

fn status_color(status: Option<LspStatus>, theme: &IdeTheme) -> [u8; 4] {
    match status {
        Some(LspStatus::Active) => theme.u8(theme.green),
        Some(LspStatus::Initializing) => theme.u8(theme.yellow),
        Some(LspStatus::Missing) => theme.u8(theme.red),
        None => theme.u8(theme.muted),
    }
}

fn state_color_f32(state: LspServerState, theme: &IdeTheme) -> [f32; 4] {
    match state {
        LspServerState::Active => theme.f32(theme.green),
        LspServerState::Ready => theme.f32(theme.blue),
        LspServerState::Initializing => theme.f32(theme.yellow),
        LspServerState::Missing => theme.f32(theme.muted),
        LspServerState::Errored => theme.f32(theme.red),
        LspServerState::Disabled => theme.f32_alpha(theme.muted, 0.5),
    }
}

fn state_color(state: LspServerState, theme: &IdeTheme) -> [u8; 4] {
    match state {
        LspServerState::Active => theme.u8(theme.green),
        LspServerState::Ready => theme.u8(theme.blue),
        LspServerState::Initializing => theme.u8(theme.yellow),
        LspServerState::Missing => theme.u8(theme.muted),
        LspServerState::Errored => theme.u8(theme.red),
        LspServerState::Disabled => theme.u8(theme.muted),
    }
}

fn message_color(level: Option<&str>, theme: &IdeTheme) -> [u8; 4] {
    match level {
        Some("error") => theme.u8(theme.red),
        Some("warn") | Some("warning") => theme.u8(theme.yellow),
        _ => theme.u8(theme.dim),
    }
}

fn diagnostics_color(counts: DiagnosticCounts, theme: &IdeTheme) -> [u8; 4] {
    if counts.error > 0 {
        theme.u8(theme.red)
    } else if counts.warn > 0 {
        theme.u8(theme.yellow)
    } else if counts.info > 0 {
        theme.u8(theme.blue)
    } else if counts.hint > 0 {
        theme.u8(theme.cyan)
    } else {
        theme.u8(theme.dim)
    }
}

fn source_label(source: Option<&str>) -> &'static str {
    match source {
        Some("managed") => "Managed by Neoism",
        Some("path") => "PATH binary",
        Some("missing") => "Missing binary",
        Some("attached") => "Attached client",
        Some("unknown") | None => "Not reported",
        Some(_) => "Not reported",
    }
}

fn row_meta(server: &LspServerRow) -> String {
    let mut parts = Vec::new();
    if let Some(filetype) = server.filetype.as_deref().filter(|ft| !ft.is_empty()) {
        parts.push(filetype.to_string());
    }
    let source = source_label(server.source.as_deref());
    if source != "Not reported" {
        parts.push(source.to_string());
    } else if let Some(binary) = server.binary.as_deref().filter(|bin| !bin.is_empty()) {
        parts.push(binary.to_string());
    }
    parts.join(" | ")
}

fn diagnostics_total(counts: DiagnosticCounts) -> u64 {
    counts.error + counts.warn + counts.info + counts.hint
}

fn diagnostics_inline(counts: DiagnosticCounts) -> Option<String> {
    (diagnostics_total(counts) > 0).then(|| {
        let mut parts = Vec::new();
        if counts.error > 0 {
            parts.push(format!("E {}", counts.error));
        }
        if counts.warn > 0 {
            parts.push(format!("W {}", counts.warn));
        }
        if counts.info > 0 {
            parts.push(format!("I {}", counts.info));
        }
        if counts.hint > 0 {
            parts.push(format!("H {}", counts.hint));
        }
        parts.join("  ")
    })
}

fn diagnostics_details(counts: DiagnosticCounts) -> String {
    if diagnostics_total(counts) == 0 {
        "0".to_string()
    } else {
        format!(
            "E {}   W {}   I {}   H {}",
            counts.error, counts.warn, counts.info, counts.hint
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_label_value(
    sugarloaf: &mut Sugarloaf,
    label_x: f32,
    value_x: f32,
    y: f32,
    value_w: f32,
    label: &str,
    value: &str,
    label_opts: &DrawOpts,
    value_opts: &DrawOpts,
) {
    sugarloaf
        .overlay_text_mut()
        .draw(label_x, y, label, label_opts);
    let value = truncate_to_fit(value, value_w, sugarloaf, value_opts);
    sugarloaf
        .overlay_text_mut()
        .draw(value_x, y, &value, value_opts);
}

fn truncate_to_fit(
    text: &str,
    max_width: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> String {
    if max_width <= 0.0 || sugarloaf.overlay_text_mut().measure(text, opts) <= max_width {
        return text.to_string();
    }
    let suffix = "...";
    let suffix_w = sugarloaf.overlay_text_mut().measure(suffix, opts);
    let mut out = String::new();
    for ch in text.chars() {
        out.push(ch);
        if sugarloaf.overlay_text_mut().measure(&out, opts) + suffix_w > max_width {
            out.pop();
            break;
        }
    }
    if out.is_empty() {
        suffix.to_string()
    } else {
        out.push_str(suffix);
        out
    }
}

fn wrap_text(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if max_lines == 0 {
        return lines;
    }
    let max_chars = max_chars.max(1);
    let mut truncated = false;

    'outer: for raw_line in text.lines() {
        if raw_line.trim().is_empty() {
            if lines.len() >= max_lines {
                truncated = true;
                break;
            }
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let word_len = word.chars().count();
            if word_len > max_chars {
                if !current.is_empty() {
                    if lines.len() >= max_lines {
                        truncated = true;
                        break 'outer;
                    }
                    lines.push(current);
                }
                let mut chunk = String::new();
                for ch in word.chars() {
                    chunk.push(ch);
                    if chunk.chars().count() >= max_chars {
                        if lines.len() >= max_lines {
                            truncated = true;
                            break 'outer;
                        }
                        lines.push(chunk);
                        chunk = String::new();
                    }
                }
                current = chunk;
                continue;
            }

            let sep = usize::from(!current.is_empty());
            if current.chars().count() + sep + word_len > max_chars && !current.is_empty()
            {
                if lines.len() >= max_lines {
                    truncated = true;
                    break 'outer;
                }
                lines.push(current);
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        if !current.is_empty() {
            if lines.len() >= max_lines {
                truncated = true;
                break;
            }
            lines.push(current);
        }
    }

    if truncated {
        if let Some(last) = lines.last_mut() {
            last.push_str("...");
        }
    }
    lines
}
