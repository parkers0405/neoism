//! Inbox list chrome stays compact; menus use the same small overlay treatment as the top bar.
use super::*;
use crate::primitives::{draw_text_with_occlusion, visible_rect_fragments, IdeTheme};
use neoism_paint::text::DrawOpts;
use crate::primitives::paint::Paint;
fn tone(theme: &IdeTheme, tone: format::Tone) -> u32 {
    match tone {
        format::Tone::Good => theme.green,
        format::Tone::Bad => theme.red,
        format::Tone::Pending => theme.yellow,
        format::Tone::Muted => theme.dim,
    }
}
impl PullRequestInbox {
    pub(super) fn render_list(
        &mut self,
        sl: &mut dyn Paint,
        r: Rect,
        base: DrawOpts,
        s: f32,
        theme: &IdeTheme,
    ) {
        let opts = DrawOpts {
            clip_rect: Some([r.x, r.y, r.w, r.h]),
            ..base
        };
        let dim = DrawOpts {
            font_size: 11.0 * s,
            color: theme.u8(theme.dim),
            ..opts
        };
        let x = r.x + 24.0 * s;
        let width = (r.w - 48.0 * s).max(1.0);
        let mut y = r.y + 24.0 * s;
        y += self.label(
            sl,
            "Pull Requests",
            x,
            y,
            width,
            DrawOpts {
                font_size: 25.0 * s,
                ..opts
            },
        ) + 8.0 * s;
        let scope = if self.repository.is_empty() {
            if self.list_error.is_some() {
                "Repository scope unavailable".to_string()
            } else if self.list_loading() {
                String::new()
            } else {
                "No repository scope available".to_string()
            }
        } else if self.viewer.is_empty() {
            format!("Repository · {}", self.repository)
        } else {
            format!("{} · signed in as {}", self.repository, self.viewer)
        };
        if scope.is_empty() {
            skeleton(
                sl,
                Rect::new(x, y, width, 16.0 * s),
                opts.clip_rect.unwrap(),
                &self.occlusions,
                theme,
                s,
                1,
            );
        }
        let scope = format::ellipsis(&scope, width, |t| sl.text_mut().measure(t, &dim));
        draw_text_with_occlusion(sl, x, y, &scope, &dim, &self.occlusions);
        y += 32.0 * s;
        let search_r = Rect::new(x, y, width, 32.0 * s);
        self.hits.push((search_r, Hit::Search));
        let search_opts = DrawOpts {
            font_size: 12.0 * s,
            clip_rect: Some([x, y, width, 30.0 * s]),
            ..opts
        };
        let mut cursor = self.search_cursor.min(self.query.len());
        while !self.query.is_char_boundary(cursor) {
            cursor -= 1;
        }
        let prefix = sl.text_mut().measure(&self.query[..cursor], &search_opts);
        let total = sl.text_mut().measure(&self.query, &search_opts);
        let (offset, caret) =
            format::search_geometry(prefix, total, width, self.search_offset, s.max(1.0));
        self.search_offset = if self.search_focus {
            offset
        } else {
            (total - width).max(0.0)
        };
        if self.query.is_empty() {
            draw_text_with_occlusion(
                sl,
                x,
                y + 5.0 * s,
                "Search pull requests, or label:bug",
                &DrawOpts {
                    color: theme.u8(theme.dim),
                    ..search_opts
                },
                &self.occlusions,
            );
        } else {
            draw_text_with_occlusion(
                sl,
                x - self.search_offset,
                y + 5.0 * s,
                &self.query,
                &search_opts,
                &self.occlusions,
            );
        }
        if self.search_focus {
            for v in visible_rect_fragments(
                [x + caret, y + 6.0 * s, s.max(1.0), 15.0 * s],
                [x, y, width, 30.0 * s],
                &self.occlusions,
            ) {
                sl.quad(
                    None,
                    v[0],
                    v[1],
                    v[2],
                    v[3],
                    theme.f32(theme.fg),
                    [0.0; 4],
                    0.0,
                    21,
                );
            }
        }
        for v in visible_rect_fragments(
            [x, y + 30.0 * s, width, s],
            [r.x, r.y, r.w, r.h],
            &self.occlusions,
        ) {
            sl.quad(
                None,
                v[0],
                v[1],
                v[2],
                v[3],
                theme.f32(theme.muted),
                [0.0; 4],
                0.0,
                20,
            );
        }
        y += 44.0 * s;
        let mut bx = x;
        for (label, hit) in [
            (
                format!("Sort: {} ⌄", format::sort_label(self.sort)),
                Hit::Sort,
            ),
            (
                format!("Filter: {} ⌄", format::filter_label(self.filter)),
                Hit::Filter,
            ),
            (
                (if self.list_error.is_some() {
                    "Retry"
                } else {
                    "Refresh"
                })
                .into(),
                Hit::RefreshList,
            ),
        ] {
            let w = sl.text_mut().measure(&label, &dim) + 18.0;
            if bx + w > x + width && bx > x {
                bx = x;
                y += 28.0 * s;
            }
            bx += self.button(sl, &label, bx, y, x + width - bx, dim, hit);
        }
        if let Some(menu) = self.menu {
            if let Some((r, _)) = self.hits.iter().find(|(_, h)| {
                matches!(
                    (menu, h),
                    (Menu::Sort, Hit::Sort) | (Menu::Filter, Hit::Filter)
                )
            }) {
                self.menu_anchor = *r;
            }
        }
        y += 38.0 * s;
        let clip = Rect::new(x, y, width, (r.y + r.h - y - 12.0 * s).max(0.0));
        let row_opts = DrawOpts {
            clip_rect: Some([clip.x, clip.y, clip.w, clip.h]),
            ..opts
        };
        let dim = DrawOpts {
            clip_rect: row_opts.clip_rect,
            ..dim
        };
        let top = y;
        y -= self.list_scroll;
        if let Some(error) = &self.list_error {
            y += self.label(
                sl,
                error,
                x,
                y,
                width,
                DrawOpts {
                    color: theme.u8(theme.yellow),
                    ..row_opts
                },
            ) + 12.0 * s;
        }
        let now = format::now_seconds();
        let items = self.items.clone();
        for (g, name) in [
            "Authored by you",
            "Review requested",
            "Assigned to you",
            "Other pull requests",
        ]
        .iter()
        .enumerate()
        {
            let rows: Vec<_> = items.iter().filter(|i| group(i) == g).collect();
            if rows.is_empty() {
                continue;
            }
            y += self.label(sl, &format!("{} · {}", name, rows.len()), x, y, width, dim)
                + 10.0 * s;
            for item in rows {
                let height = 58.0 * s;
                let visible = Rect::new(
                    x,
                    y.max(clip.y),
                    width,
                    ((y + height).min(clip.y + clip.h) - y.max(clip.y)).max(0.0),
                );
                if visible.h > 0.0 {
                    if self.selected == Some(item.summary.number) {
                        for v in visible_rect_fragments(
                            [x - 6.0 * s, visible.y, width + 12.0 * s, visible.h],
                            [r.x, clip.y, r.w, clip.h],
                            &self.occlusions,
                        ) {
                            sl.quad(
                                None,
                                v[0],
                                v[1],
                                v[2],
                                v[3],
                                theme.f32_alpha(theme.fg, 0.045),
                                [0.0; 4],
                                0.0,
                                20,
                            );
                        }
                    }
                    self.hits.push((visible, Hit::Select(item.summary.number)));
                    let state = if item.summary.draft {
                        "◌"
                    } else if item.summary.state.eq_ignore_ascii_case("closed") {
                        "⊗"
                    } else {
                        "⑂"
                    };
                    let state_color = if item.summary.draft {
                        theme.dim
                    } else if item.summary.state.eq_ignore_ascii_case("open") {
                        theme.green
                    } else if item.summary.state.eq_ignore_ascii_case("closed") {
                        theme.red
                    } else {
                        theme.accent
                    };
                    draw_text_with_occlusion(
                        sl,
                        x,
                        y,
                        state,
                        &DrawOpts {
                            color: theme.u8(state_color),
                            ..row_opts
                        },
                        &self.occlusions,
                    );
                    let plus = format!("+{}", item.additions);
                    let minus = format!("−{}", item.deletions);
                    let pw = sl.text_mut().measure(&plus, &dim);
                    let mw = sl.text_mut().measure(&minus, &dim);
                    let count_width = pw + mw + 8.0 * s;
                    let title =
                        format!("#{}  {}", item.summary.number, item.summary.title);
                    let title = format::ellipsis(
                        &title,
                        (width - 24.0 * s - count_width - 12.0 * s).max(0.0),
                        |t| sl.text_mut().measure(t, &row_opts),
                    );
                    draw_text_with_occlusion(
                        sl,
                        x + 24.0 * s,
                        y,
                        &title,
                        &row_opts,
                        &self.occlusions,
                    );
                    draw_text_with_occlusion(
                        sl,
                        x + width - count_width,
                        y + 2.0 * s,
                        &plus,
                        &DrawOpts {
                            color: theme.u8(theme.green),
                            ..dim
                        },
                        &self.occlusions,
                    );
                    draw_text_with_occlusion(
                        sl,
                        x + width - mw,
                        y + 2.0 * s,
                        &minus,
                        &DrawOpts {
                            color: theme.u8(theme.red),
                            ..dim
                        },
                        &self.occlusions,
                    );
                    let age = format::updated(&item.updated_at, now);
                    let age_width = sl.text_mut().measure(&age, &dim);
                    let meta = format!("{} · {}", item.summary.author, item.repository);
                    let meta = format::ellipsis(
                        &meta,
                        (width - age_width - 78.0 * s).max(0.0),
                        |t| sl.text_mut().measure(t, &dim),
                    );
                    draw_text_with_occlusion(
                        sl,
                        x + width - 42.0 * s - age_width,
                        y + 23.0 * s,
                        &age,
                        &dim,
                        &self.occlusions,
                    );
                    draw_text_with_occlusion(
                        sl,
                        x + 24.0 * s,
                        y + 23.0 * s,
                        &meta,
                        &dim,
                        &self.occlusions,
                    );
                    for (index, status) in [
                        format::checks(&item.summary.checks_state),
                        format::review(&item.summary.review_decision),
                    ]
                    .iter()
                    .enumerate()
                    {
                        draw_text_with_occlusion(
                            sl,
                            x + width - (30.0 - 18.0 * index as f32) * s,
                            y + 23.0 * s,
                            status.glyph,
                            &DrawOpts {
                                color: theme.u8(tone(theme, status.tone)),
                                ..dim
                            },
                            &self.occlusions,
                        );
                    }
                }
                y += height;
            }
            y += 12.0 * s;
        }
        if items.is_empty() && self.list_error.is_none() {
            let loading = self.list_loading();
            if loading {
                skeleton(
                    sl,
                    Rect::new(x, y, width, clip.h),
                    row_opts.clip_rect.unwrap(),
                    &self.occlusions,
                    theme,
                    s,
                    9,
                );
                y += 234.0 * s;
            } else {
                y += self.label(sl, "No pull requests found", x, y, width, row_opts)
                    + 24.0 * s;
            }
        }
        if self.next_cursor.is_some() {
            if self.list_loading() {
                skeleton(
                    sl,
                    Rect::new(x, y, width.min(160.0 * s), 24.0 * s),
                    row_opts.clip_rect.unwrap(),
                    &self.occlusions,
                    theme,
                    s,
                    1,
                );
            } else {
                self.button(sl, "Load more", x, y, width, row_opts, Hit::More);
            }
            y += 36.0 * s;
        }
        self.list_max = (y + self.list_scroll - top - clip.h).max(0.0);
        self.list_scroll = self.list_scroll.min(self.list_max);
    }
    pub(super) fn render_menu(
        &mut self,
        sl: &mut dyn Paint,
        base: DrawOpts,
        s: f32,
        theme: &IdeTheme,
    ) {
        self.menu_hits.clear();
        let Some(menu) = self.menu else { return };
        if self.list_rect.w <= 0.0 {
            self.menu = None;
            return;
        }
        let labels: Vec<&str> = match menu {
            Menu::Sort => format::SORTS
                .iter()
                .map(|v| format::sort_label(*v))
                .collect(),
            Menu::Filter => format::FILTERS
                .iter()
                .map(|v| format::filter_label(*v))
                .collect(),
        };
        let opts = DrawOpts {
            font_size: 12.0 * s,
            ..base
        };
        let width = (labels
            .iter()
            .map(|t| sl.text_mut().measure(t, &opts))
            .fold(0.0, f32::max)
            + 48.0 * s)
            .min(self.list_rect.w);
        let h = labels.len() as f32 * 30.0 * s + 8.0 * s;
        let x = self
            .menu_anchor
            .x
            .min(self.list_rect.x + self.list_rect.w - width)
            .max(self.list_rect.x);
        let y = (self.menu_anchor.y + self.menu_anchor.h + 4.0 * s)
            .min(self.list_rect.y + self.list_rect.h - h)
            .max(self.list_rect.y);
        let clip = [
            self.list_rect.x,
            self.list_rect.y,
            self.list_rect.w,
            self.list_rect.h,
        ];
        for v in visible_rect_fragments([x, y, width, h], clip, &self.occlusions) {
            sl.overlay_rect(v[0], v[1], v[2], v[3], theme.f32(theme.panel_bg()), 0.0, 30);
        }
        for (i, label) in labels.iter().enumerate() {
            let row = [
                x + 4.0 * s,
                y + 4.0 * s + i as f32 * 30.0 * s,
                (width - 8.0 * s).max(0.0),
                30.0 * s,
            ];
            let selected = match menu {
                Menu::Sort => format::SORTS[i] == self.sort,
                Menu::Filter => format::FILTERS[i] == self.filter,
            };
            for v in visible_rect_fragments(row, clip, &self.occlusions) {
                self.menu_hits.push((Rect::new(v[0], v[1], v[2], v[3]), i));
                if i == self.menu_index {
                    sl.overlay_rect(
                        v[0],
                        v[1],
                        v[2],
                        v[3],
                        theme.f32_alpha(theme.fg, 0.07),
                        0.0,
                        31,
                    );
                }
                let opts = DrawOpts {
                    clip_rect: Some(v),
                    ..opts
                };
                if selected {
                    sl.overlay_text_mut().draw(
                        x + 10.0 * s,
                        row[1] + 6.0 * s,
                        "✓",
                        &opts,
                    );
                }
                sl.overlay_text_mut()
                    .draw(x + 30.0 * s, row[1] + 6.0 * s, label, &opts);
            }
        }
    }
}
fn group(i: &PullRequestInboxItem) -> usize {
    if i.authored {
        0
    } else if i.review_requested {
        1
    } else if i.assigned {
        2
    } else {
        3
    }
}
