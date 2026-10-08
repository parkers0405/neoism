use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::session_group::SESSION_PICKER_FOOTER;
use crate::panels::agent_pane::state::picker::{
    NeoismAgentPicker, NeoismAgentPickerKind,
};
use crate::panels::agent_pane::state::NeoismAgentPane;

use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::inline_picker::{
    InlinePickerRow, InlinePickerStatus, InlinePickerView,
};

pub trait AgentPickerPane {
    fn picker_mut(&mut self) -> Option<&mut NeoismAgentPicker>;

    /// In-progress inline rename buffer for the `/sessions` picker, if the
    /// host is currently renaming the selected session. Defaults to `None`
    /// for panes without rename state.
    fn picker_rename_buffer(&self) -> Option<String> {
        None
    }
}

impl AgentPickerPane for NeoismAgentPane {
    fn picker_mut(&mut self) -> Option<&mut NeoismAgentPicker> {
        self.picker_mut()
    }

    fn picker_rename_buffer(&self) -> Option<String> {
        self.session_rename_buffer()
    }
}

pub fn render_picker(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentPickerPane,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    max_rows: usize,
    min_y: f32,
) {
    let rename = pane.picker_rename_buffer();
    let Some(picker) = pane.picker_mut() else {
        return;
    };
    if picker.kind == NeoismAgentPickerKind::Usage {
        render_usage_picker(sugarloaf, picker, input_rect, theme, s, max_rows, min_y);
        return;
    }
    picker.set_visible_row_limit(max_rows);
    let is_session = picker.kind == NeoismAgentPickerKind::Session;
    let footer_hint = is_session.then_some(SESSION_PICKER_FOOTER);
    // Rename only applies while a session picker is open.
    let rename = rename.filter(|_| is_session);
    // Slash / @file / skill-mention pickers type into the composer (which
    // owns the caret), so suppress the search-row caret for them.
    let show_search_caret = !matches!(
        picker.kind,
        NeoismAgentPickerKind::Slash
            | NeoismAgentPickerKind::FileMention
            | NeoismAgentPickerKind::SkillMention
    );
    let empty_message = (!matches!(
        picker.kind,
        NeoismAgentPickerKind::ConnectLabel | NeoismAgentPickerKind::ConnectSecret
    ))
    .then_some("No results");
    let list_scroll_offset = picker.tick_list_scroll();
    let cursor_offset = picker.tick_cursor();
    let rows = picker
        .options()
        .iter()
        .map(|option| InlinePickerRow {
            title: &option.title,
            description: &option.description,
            footer: &option.footer,
            is_header: option.is_header,
            is_current: option.is_current,
            is_pinned: option.pinned,
            status: (picker.kind == NeoismAgentPickerKind::Mcp).then(|| {
                match option.footer.as_str() {
                    "connected" => InlinePickerStatus::Good,
                    "needs auth" | "needs registration" | "ready" => {
                        InlinePickerStatus::Warning
                    }
                    "failed" => InlinePickerStatus::Error,
                    _ => InlinePickerStatus::Muted,
                }
            }),
        })
        .collect::<Vec<_>>();
    if let Some(render_state) = crate::widgets::inline_picker::render_limited(
        sugarloaf,
        InlinePickerView {
            title: &picker.title,
            query: &picker.query,
            selected: picker.selected,
            scroll_offset: picker.scroll_offset,
            list_scroll_offset,
            cursor_offset,
            rows: &rows,
            footer_hint,
            rename: rename.as_deref(),
            show_search_caret,
            wrap_prompt: false,
            search_placeholder: picker.search_placeholder.as_deref().unwrap_or("Search"),
            loading: picker.loading,
            empty_message,
            loading_elapsed: picker.loading_elapsed(),
        },
        input_rect,
        theme,
        s,
        max_rows,
        min_y,
    ) {
        picker.set_last_rect(render_state.rect);
        picker.set_footer_h(render_state.footer_h);
        // cursor rect is intentionally NOT updated here — the caret stays
        // in the input text area while the picker dropdown is visible.
    }
}

/// Dedicated read-only usage content inside the existing themed picker shell.
/// The cached layout is also the host's geometry; no inferred row-height ratios.
pub fn render_usage_picker(
    sugarloaf: &mut Sugarloaf,
    picker: &mut NeoismAgentPicker,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    max_rows: usize,
    min_y: f32,
) {
    let g = picker.usage_layout(input_rect, scale, max_rows, min_y);
    let s = g.scale;
    let [x, y, w, h] = g.rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let residual = picker.tick_list_scroll();
    for (inset, color, order) in [(0.0, theme.border, 181), (s, theme.bg, 182)] {
        sugarloaf.overlay_rounded_rect(
            x + inset,
            y + inset,
            (w - inset * 2.0).max(0.0),
            (h - inset * 2.0).max(0.0),
            theme.f32(color),
            0.0,
            (14.0 * s - inset).max(0.0),
            order,
        );
    }
    let header = [
        x + s,
        y + s,
        (w - 2.0 * s).max(0.0),
        (g.body[1] - y - s).max(0.0),
    ];
    usage_text(
        sugarloaf,
        "esc",
        [x + w - 38.0 * s, y + 12.0 * s, 28.0 * s, 18.0 * s],
        header,
        theme,
        theme.muted,
        s,
        false,
    );
    let count = picker.usage_accounts.len();
    let title = format!(
        "Codex  ·  {count} {}",
        if count == 1 { "account" } else { "accounts" }
    );
    usage_text(
        sugarloaf,
        &title,
        [
            x + 14.0 * s,
            y + 12.0 * s,
            (w - 64.0 * s).max(0.0),
            18.0 * s,
        ],
        header,
        theme,
        theme.fg,
        s,
        true,
    );
    let body = [
        g.body[0] + s,
        g.body[1],
        (g.body[2] - 2.0 * s).max(0.0),
        g.body[3],
    ];
    let message = if let Some(error) = picker.usage_error.as_deref() {
        Some(error)
    } else if count == 0 {
        Some("No Codex accounts connected")
    } else {
        None
    };
    if picker.loading {
        crate::widgets::inline_picker::render_loading_skeleton(
            sugarloaf,
            body,
            theme,
            s,
            picker.loading_elapsed(),
        );
    } else if let Some(message) = message {
        usage_text(
            sugarloaf,
            message,
            [
                x + 14.0 * s,
                body[1] + 16.0 * s,
                (w - 28.0 * s).max(0.0),
                20.0 * s,
            ],
            body,
            theme,
            theme.muted,
            s,
            false,
        );
    } else {
        let row_h = picker.row_height() * s;
        let now = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
            .unwrap_or(0);
        for (ix, account) in picker
            .usage_accounts
            .iter()
            .enumerate()
            .skip(picker.scroll_offset)
        {
            let row_y =
                body[1] + (ix - picker.scroll_offset) as f32 * row_h + residual * s;
            if row_y >= body[1] + body[3] {
                break;
            }
            if account.is_default {
                let mut color = theme.f32(theme.hover);
                let bg = theme.f32(theme.bg);
                for channel in 0..3 {
                    color[channel] = bg[channel] * 0.65 + color[channel] * 0.35;
                }
                usage_rect(sugarloaf, [x + s, row_y, w - 2.0 * s, row_h], body, color);
            }
            let left_w = if g.stacked {
                w - 28.0 * s
            } else {
                w * 0.34 - 20.0 * s
            };
            let label = if account.label.trim().is_empty() {
                "Account"
            } else {
                &account.label
            };
            usage_text(
                sugarloaf,
                label,
                [x + 14.0 * s, row_y + 12.0 * s, left_w, 20.0 * s],
                body,
                theme,
                theme.fg,
                s,
                true,
            );
            let plan = account.plan_label();
            let plan = if account.is_default {
                format!("{plan} · default")
            } else {
                plan
            };
            usage_text(
                sugarloaf,
                &plan,
                [x + 14.0 * s, row_y + 33.0 * s, left_w, 18.0 * s],
                body,
                theme,
                theme.muted,
                s,
                false,
            );
            let wx = if g.stacked {
                x + 14.0 * s
            } else {
                x + w * 0.34
            };
            let ww = (x + w - 14.0 * s - wx).max(0.0);
            let wy = row_y + if g.stacked { 58.0 * s } else { 12.0 * s };
            for (wi, window) in account.windows.iter().enumerate() {
                let top = wy + wi as f32 * 60.0 * s;
                let remaining = window.remaining_percent();
                let percent = remaining
                    .map(|p| format!("{p:.0}% left"))
                    .unwrap_or_else(|| "Unavailable".into());
                let opts = DrawOpts {
                    font_size: 13.0 * s,
                    bold: true,
                    ..DrawOpts::default()
                };
                let percent_w = sugarloaf
                    .overlay_text_mut()
                    .measure(&percent, &opts)
                    .min(ww);
                let label = window.limit_label();
                usage_text(
                    sugarloaf,
                    &label,
                    [wx, top, (ww - percent_w - 8.0 * s).max(0.0), 18.0 * s],
                    body,
                    theme,
                    theme.fg,
                    s,
                    false,
                );
                usage_text(
                    sugarloaf,
                    &percent,
                    [wx + ww - percent_w, top, percent_w, 18.0 * s],
                    body,
                    theme,
                    theme.fg,
                    s,
                    true,
                );
                usage_rect(
                    sugarloaf,
                    [wx, top + 23.0 * s, ww, 3.0 * s],
                    body,
                    theme.f32(theme.border),
                );
                if let Some(p) = remaining {
                    usage_rect(
                        sugarloaf,
                        [wx, top + 23.0 * s, ww * (p / 100.0) as f32, 3.0 * s],
                        body,
                        theme.f32(theme.green),
                    );
                }
                usage_text(
                    sugarloaf,
                    &window.reset_label(now),
                    [wx, top + 33.0 * s, ww, 18.0 * s],
                    body,
                    theme,
                    theme.muted,
                    s,
                    false,
                );
            }
            if let Some(error) = account.error.as_deref() {
                usage_text(
                    sugarloaf,
                    error,
                    [
                        wx,
                        wy + account.windows.len() as f32 * 60.0 * s,
                        ww,
                        20.0 * s,
                    ],
                    body,
                    theme,
                    if account.auth_type == "api" {
                        theme.muted
                    } else {
                        theme.red
                    },
                    s,
                    false,
                );
            } else if account.windows.is_empty() {
                usage_text(
                    sugarloaf,
                    "Usage unavailable",
                    [wx, wy, ww, 20.0 * s],
                    body,
                    theme,
                    theme.muted,
                    s,
                    false,
                );
            }
            usage_rect(
                sugarloaf,
                [x + 14.0 * s, row_y + row_h - s, (w - 28.0 * s).max(0.0), s],
                body,
                theme.f32(theme.border),
            );
        }
        let total_h = count as f32 * row_h;
        if total_h > body[3] && body[3] > 0.0 {
            let thumb_h = (body[3] * body[3] / total_h).max(12.0 * s).min(body[3]);
            let scroll = picker.scroll_offset as f32 * row_h - residual * s;
            let thumb_y = body[1]
                + (body[3] - thumb_h) * (scroll / (total_h - body[3])).clamp(0.0, 1.0);
            usage_rect(
                sugarloaf,
                [x + w - 5.0 * s, thumb_y, 2.0 * s, thumb_h],
                body,
                theme.f32(theme.dim),
            );
        }
    }
    if g.footer_h > 0.0 {
        let clip = [
            x + s,
            y + h - g.footer_h,
            (w - 2.0 * s).max(0.0),
            (g.footer_h - s).max(0.0),
        ];
        usage_text(
            sugarloaf,
            "Enter refresh · Esc close",
            [
                x + 14.0 * s,
                clip[1] + 6.0 * s,
                (w - 28.0 * s).max(0.0),
                18.0 * s,
            ],
            clip,
            theme,
            theme.muted,
            s,
            false,
        );
    }
}

fn usage_intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let w = (a[0] + a[2]).min(b[0] + b[2]) - x;
    let h = (a[1] + a[3]).min(b[1] + b[3]) - y;
    (w > 0.0 && h > 0.0).then_some([x, y, w, h])
}

fn usage_rect(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
) {
    if let Some([x, y, w, h]) = usage_intersect(rect, clip) {
        sugarloaf.overlay_rounded_rect(x, y, w, h, color, 0.0, 0.0, 184);
    }
}

#[allow(clippy::too_many_arguments)]
fn usage_text(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    rect: [f32; 4],
    clip: [f32; 4],
    theme: &IdeTheme,
    color: u32,
    s: f32,
    bold: bool,
) {
    let Some(clip) = usage_intersect(rect, clip) else {
        return;
    };
    let opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(color),
        bold,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let mut text = text.replace(['\n', '\r'], " ");
    if sugarloaf.overlay_text_mut().measure(&text, &opts) > rect[2] {
        while !text.is_empty()
            && sugarloaf
                .overlay_text_mut()
                .measure(&format!("{text}…"), &opts)
                > rect[2]
        {
            text.pop();
        }
        text.push('…');
    }
    sugarloaf
        .overlay_text_mut()
        .draw(rect[0], rect[1], &text, &opts);
}
