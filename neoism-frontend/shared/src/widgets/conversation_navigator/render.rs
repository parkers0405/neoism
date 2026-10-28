//! Navigator-only Finder shell. Geometry is shared by paint and pointer routing.
use super::{ConversationNavigator, SEARCH_ROW_HEIGHT};
use crate::panels::{command_palette::render::draw_modal_frame_top, file_tree};
use crate::primitives::{
    geom::intersect_rect, snap_to_device_px, text::truncate_to_fit, IdeTheme,
};
use crate::primitives::paint::Paint;
use sugarloaf::text::DrawOpts;

const INPUT_HEIGHT: f32 = 40.0;
const INPUT_FONT: f32 = 14.0;
const INPUT_PADDING: f32 = 14.0;
const ROW_FONT: f32 = 13.0;
const PADDING: f32 = 4.0;
const SEPARATOR: f32 = 1.0;
const ORDER: u8 = 180;

fn stroke(s: f32) -> f32 {
    (file_tree::FRAME_STROKE * s).max(2.0)
}
pub(super) fn frame_inset(s: f32) -> f32 {
    snap_to_device_px(stroke(s) + PADDING * s, s)
}
pub(super) fn header_height(s: f32) -> f32 {
    snap_to_device_px(frame_inset(s) + (INPUT_HEIGHT + SEPARATOR + PADDING) * s, s)
}

/// Snap the painted frame once; retain whole-row height rather than shaving
/// a pixel from each centered edge. The host, not the unsnapped frame, clips it.
pub(super) fn snap_bounds(
    [x, y, w, h]: [f32; 4],
    s: f32,
    viewport: [f32; 4],
) -> [f32; 4] {
    let [vx, vy, vw, vh] = viewport;
    let left = ((vx * s).ceil() / s).min(vx + vw.max(0.0));
    let top = ((vy * s).ceil() / s).min(vy + vh.max(0.0));
    let right = ((vx + vw.max(0.0)) * s).floor() / s;
    let bottom = ((vy + vh.max(0.0)) * s).floor() / s;
    let w = ((w * s).floor() / s).min((right - left).max(0.0));
    let h = ((h * s).floor() / s).min((bottom - top).max(0.0));
    [
        snap_to_device_px(x, s).clamp(left, (right - w).max(left)),
        snap_to_device_px(y, s).clamp(top, (bottom - h).max(top)),
        w,
        h,
    ]
}

#[derive(Clone, Copy)]
pub(super) struct NavigatorRenderGeometry {
    pub rect: [f32; 4],
    pub first_row: usize,
    pub visible_rows: usize,
    pub row_height: f32,
}
impl NavigatorRenderGeometry {
    pub(super) fn input(self, s: f32) -> [f32; 4] {
        let [x, y, w, h] = self.rect;
        let inset = frame_inset(s);
        [
            x + inset.min(w),
            y + inset.min(h),
            (w - inset * 2.0).max(0.0),
            (INPUT_HEIGHT * s).min((h - inset * 2.0).max(0.0)),
        ]
    }
    pub(super) fn body(self, s: f32) -> [f32; 4] {
        let [x, y, w, h] = self.rect;
        let inset = frame_inset(s);
        let top = snap_to_device_px(y + header_height(s), s).min(y + h);
        let bottom = snap_to_device_px(y + h - inset, s).min(y + h);
        [
            x + inset.min(w),
            top,
            (w - inset * 2.0).max(0.0),
            (bottom - top).max(0.0),
        ]
    }
    pub(super) fn row_y(self, index: usize, s: f32, residual: f32) -> f32 {
        self.body(s)[1]
            + (index - self.first_row) as f32 * self.row_height * s
            + snap_to_device_px(residual * s, s)
    }
    /// Search owns the entire second line, independent of title/source budgets.
    pub(super) fn text_lines(
        self,
        index: usize,
        s: f32,
        residual: f32,
    ) -> ([f32; 4], Option<[f32; 4]>) {
        let body = self.body(s);
        let y = self.row_y(index, s, residual);
        let x = body[0] + 14.0 * s;
        let width = (body[2] - 28.0 * s).max(0.0);
        if self.row_height == SEARCH_ROW_HEIGHT {
            (
                [x, y + 7.0 * s, width, 16.0 * s],
                Some([x, y + 27.0 * s, width, 15.0 * s]),
            )
        } else {
            (
                [
                    x,
                    y + (self.row_height - ROW_FONT) * s / 2.0,
                    width,
                    ROW_FONT * s,
                ],
                None,
            )
        }
    }
    pub(super) fn skeleton_bars(self, index: usize, s: f32) -> Vec<[f32; 4]> {
        let (first, second) = self.text_lines(index, s, 0.0);
        let title_fraction = if second.is_some() {
            [0.74, 0.58, 0.86][index % 3]
        } else {
            [0.42, 0.31, 0.48][index % 3]
        };
        vec![
            [
                first[0],
                first[1] + 3.0 * s,
                first[2] * title_fraction,
                7.0 * s,
            ],
            if let Some(sentence) = second {
                [
                    sentence[0],
                    sentence[1] + 3.0 * s,
                    sentence[2] * [0.94, 0.82, 0.9][index % 3],
                    6.0 * s,
                ]
            } else {
                [
                    first[0] + first[2] * 0.72,
                    first[1] + 3.0 * s,
                    first[2] * 0.24,
                    7.0 * s,
                ]
            },
        ]
    }
    pub(super) fn row_hit(self, index: usize, s: f32, residual: f32) -> Option<[f32; 4]> {
        let body = self.body(s);
        intersect_rect(
            [
                body[0],
                self.row_y(index, s, residual),
                body[2],
                self.row_height * s,
            ],
            body,
        )
    }
}

pub(super) fn skeleton_alpha(now: f32, index: usize) -> f32 {
    let phase = if now.is_finite() {
        now.rem_euclid(10.0)
    } else {
        0.0
    };
    0.10 + 0.08 * ((phase * std::f32::consts::TAU - index as f32 * 0.7).sin() + 1.0) / 2.0
}

/// Shorten only the leading context when it would hide the query. Width is
/// measured, and all cuts are Unicode character boundaries, as in the excerpt.
pub(super) fn fit_excerpt(
    text: &str,
    query: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> String {
    let chars: Vec<char> = text.chars().collect();
    let Some((matched, length)) = super::query_match(&chars, query) else {
        return text.to_owned();
    };
    let mut start = 0;
    while start < matched {
        let prefix = format!(
            "{}{}",
            if start > 0 { "…" } else { "" },
            chars[start..matched + length].iter().collect::<String>()
        );
        if measure(&prefix) <= width {
            break;
        }
        start += 1;
    }
    format!(
        "{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..].iter().collect::<String>()
    )
}

fn quad(
    sugarloaf: &mut (impl Paint + ?Sized),
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
    radius: f32,
) {
    if let Some([x, y, w, h]) = intersect_rect(rect, clip) {
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            w,
            h,
            color,
            0.2,
            radius.min(w / 2.0).min(h / 2.0),
            ORDER,
        );
    }
}

impl ConversationNavigator {
    pub(super) fn input_status(&self) -> Option<&'static str> {
        if self.error.is_some() || self.filtered.is_empty() {
            None
        } else if self.indexing_status().is_some() {
            Some("Indexing…")
        } else if self.loading {
            Some("Loading…")
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint(
        &self,
        sugarloaf: &mut (impl Paint + ?Sized),
        theme: &IdeTheme,
        geometry: NavigatorRenderGeometry,
        s: f32,
        residual: f32,
        cursor_offset: f32,
        now: f32,
    ) {
        let [x, y, w, h] = geometry.rect;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        // This is the existing Finder/command-palette top-only frame helper,
        // not InlinePicker's permission/model chrome. Clamp stroke on tiny hosts.
        let mut fill = theme.f32(theme.panel_bg());
        fill[3] = 1.0;
        draw_modal_frame_top(
            sugarloaf,
            x,
            y,
            w,
            h,
            stroke(s).min(w / 2.0).min(h / 2.0),
            (8.0 * s).min(w / 2.0).min(h / 2.0),
            theme.f32(theme.surface),
            fill,
            0.1,
            ORDER,
        );
        let input = geometry.input(s);
        let body = geometry.body(s);
        // Partial input bands on very small hosts stay empty, never overpaint.
        if input[2] > 0.0 && input[3] >= INPUT_HEIGHT * s {
            let opts = DrawOpts {
                font_size: INPUT_FONT * s,
                color: theme.u8(theme.muted),
                clip_rect: Some(input),
                ..DrawOpts::default()
            };
            let pad = INPUT_PADDING * s;
            let status = self.input_status().unwrap_or("");
            let status = truncate_to_fit(
                status,
                (input[2] * 0.4 - pad).max(0.0),
                sugarloaf,
                &opts,
            );
            let status_w = sugarloaf.overlay_text_mut().measure(&status, &opts);
            let right = input[0] + input[2] - pad;
            let text_right = right
                - if status.is_empty() {
                    0.0
                } else {
                    status_w + 10.0 * s
                };
            let text_x = input[0] + pad;
            let text_width = (text_right - text_x - 1.5 * s).max(0.0);
            let placeholder = if self.projects() {
                "Find a project…"
            } else {
                "Search titles and messages…"
            };
            let display = truncate_to_fit(
                if self.query.is_empty() {
                    placeholder
                } else {
                    &self.query
                },
                text_width,
                sugarloaf,
                &opts,
            );
            let text_y = input[1] + (input[3] - INPUT_FONT * s) / 2.0;
            let query_clip = intersect_rect(
                [text_x, input[1], (text_right - text_x).max(0.0), input[3]],
                input,
            );
            if let Some(clip) = query_clip {
                let query_opts = DrawOpts {
                    color: if self.query.is_empty() {
                        theme.u8(theme.muted)
                    } else {
                        theme.u8(theme.fg)
                    },
                    clip_rect: Some(clip),
                    ..opts
                };
                let measured = sugarloaf.overlay_text_mut().draw(
                    text_x,
                    text_y,
                    &display,
                    &query_opts,
                );
                if !now.is_finite() || (now * 2.0).floor() as i64 % 2 == 0 {
                    let caret_x = (text_x
                        + if self.query.is_empty() { 0.0 } else { measured })
                    .min(text_right - 1.5 * s);
                    quad(
                        sugarloaf,
                        [caret_x, input[1] + 12.0 * s, 1.5 * s, 18.0 * s],
                        clip,
                        theme.f32(theme.fg),
                        0.0,
                    );
                }
            }
            sugarloaf
                .overlay_text_mut()
                .draw(right - status_w, text_y, &status, &opts);
            quad(
                sugarloaf,
                [
                    input[0],
                    input[1] + INPUT_HEIGHT * s,
                    input[2],
                    SEPARATOR * s,
                ],
                geometry.rect,
                theme.f32(theme.border),
                0.0,
            );
        }
        if body[2] <= 0.0 || body[3] <= 0.0 {
            return;
        }
        let row_opts = DrawOpts {
            font_size: ROW_FONT * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(body),
            ..DrawOpts::default()
        };
        if self.show_skeletons() {
            for index in 0..geometry.visible_rows {
                let mut color = theme.f32(theme.muted);
                color[3] = skeleton_alpha(now, index);
                for bar in geometry.skeleton_bars(index, s) {
                    quad(sugarloaf, bar, body, color, 3.0 * s);
                }
            }
            return;
        }
        if self.error.is_some() || self.filtered.is_empty() {
            let message = self.error.as_deref().unwrap_or_else(|| {
                if self.indexing_status().is_some() {
                    "Indexing conversation history…"
                } else if self.loading {
                    "Loading…"
                } else if self.projects() {
                    "No matching projects"
                } else if self.query.trim().is_empty() {
                    "No conversations yet"
                } else {
                    "No matching conversations"
                }
            });
            let message = truncate_to_fit(
                message,
                (body[2] - 28.0 * s).max(0.0),
                sugarloaf,
                &row_opts,
            );
            sugarloaf.overlay_text_mut().draw(
                body[0] + 14.0 * s,
                body[1] + 9.5 * s,
                &message,
                &row_opts,
            );
            return;
        }
        if let Some(selected) = self
            .selected
            .filter(|i| !self.rows[self.filtered[*i]].is_disabled)
        {
            let selected_y = body[1]
                + (selected as f32 - geometry.first_row as f32) * geometry.row_height * s
                + snap_to_device_px(residual * s, s)
                + snap_to_device_px(cursor_offset * s, s);
            quad(
                sugarloaf,
                [body[0], selected_y, body[2], geometry.row_height * s],
                body,
                theme.f32(theme.hover),
                3.0 * s,
            );
        }
        for index in geometry.first_row
            ..(geometry.first_row + geometry.visible_rows + 1).min(self.filtered.len())
        {
            let Some(hit) = geometry.row_hit(index, s, residual) else {
                continue;
            };
            let row = &self.rows[self.filtered[index]];
            let (first_line, sentence_line) = geometry.text_lines(index, s, residual);
            let text_y = first_line[1];
            let left = first_line[0];
            let available = first_line[2];
            let opts = DrawOpts {
                color: if row.is_disabled {
                    theme.u8(theme.muted)
                } else {
                    theme.u8(theme.fg)
                },
                clip_rect: Some(hit),
                ..row_opts
            };
            if row.is_current {
                quad(
                    sugarloaf,
                    [
                        body[0] + 5.0 * s,
                        text_y + (ROW_FONT - 4.0) * s / 2.0,
                        4.0 * s,
                        4.0 * s,
                    ],
                    hit,
                    if row.is_disabled {
                        theme.f32(theme.muted)
                    } else {
                        theme.f32(theme.fg)
                    },
                    2.0 * s,
                );
            }
            if let Some(sentence) = sentence_line {
                let title = truncate_to_fit(&row.title, available, sugarloaf, &opts);
                sugarloaf
                    .overlay_text_mut()
                    .draw(left, text_y, &title, &opts);
                let sentence_opts = DrawOpts {
                    font_size: 12.0 * s,
                    color: theme.u8(theme.muted),
                    ..opts
                };
                let excerpt =
                    fit_excerpt(&row.description, &self.query, sentence[2], |prefix| {
                        sugarloaf.overlay_text_mut().measure(prefix, &sentence_opts)
                    });
                let excerpt =
                    truncate_to_fit(&excerpt, sentence[2], sugarloaf, &sentence_opts);
                sugarloaf.overlay_text_mut().draw(
                    sentence[0],
                    sentence[1],
                    &excerpt,
                    &sentence_opts,
                );
                continue;
            }
            let has_secondary = !row.description.is_empty() || !row.footer.is_empty();
            let title_budget = if has_secondary {
                available * 0.36
            } else {
                available
            };
            let title = truncate_to_fit(&row.title, title_budget, sugarloaf, &opts);
            let title_w = sugarloaf
                .overlay_text_mut()
                .draw(left, text_y, &title, &opts);
            let mut next_x = left + title_w + 10.0 * s;
            let muted = DrawOpts {
                color: theme.u8(theme.muted),
                ..opts
            };
            // Budget source/project independently so a long excerpt cannot erase
            // duplicate-project identity, paths or the disabled reason.
            if !row.footer.is_empty() {
                let remaining = (body[0] + body[2] - 14.0 * s - next_x).max(0.0);
                let budget = if row.description.is_empty() {
                    remaining
                } else {
                    remaining * 0.45
                };
                let footer = truncate_to_fit(&row.footer, budget, sugarloaf, &muted);
                next_x += sugarloaf
                    .overlay_text_mut()
                    .draw(next_x, text_y, &footer, &muted)
                    + 10.0 * s;
            }
            let remaining = (body[0] + body[2] - 14.0 * s - next_x).max(0.0);
            let description =
                truncate_to_fit(&row.description, remaining, sugarloaf, &muted);
            sugarloaf
                .overlay_text_mut()
                .draw(next_x, text_y, &description, &muted);
        }
    }
}
