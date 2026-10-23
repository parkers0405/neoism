use super::*;
use crate::primitives::{draw_text_with_occlusion, IdeTheme};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use sugarloaf::{text::DrawOpts, Sugarloaf};

// Use one metric and option set for wrapping and drawing, including long unbroken titles.
fn wrap(text: &str, width: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut rows = vec![];
    let mut row = String::new();
    for c in text.chars() {
        if c == '\n' {
            rows.push(std::mem::take(&mut row));
            continue;
        }
        let mut next = row.clone();
        next.push(c);
        if !row.is_empty() && measure(&next) > width.max(1.0) {
            if let Some(i) = row.rfind(' ') {
                let rest = row[i + 1..].to_string();
                rows.push(row[..i].to_string());
                row = rest;
            } else {
                rows.push(std::mem::take(&mut row));
            }
        }
        row.push(c);
    }
    rows.push(row);
    rows
}
pub(super) fn panes(
    rect: Rect,
    selected: bool,
    narrow_detail: bool,
    scale: f32,
) -> (Rect, Rect) {
    let zero = Rect::new(rect.x, rect.y, 0.0, 0.0);
    if !selected {
        return (rect, zero);
    }
    if rect.w < 900.0 * scale {
        return if narrow_detail {
            (zero, rect)
        } else {
            (rect, zero)
        };
    }
    let left = (rect.w * 0.40).clamp(320.0 * scale, 480.0 * scale);
    (
        Rect::new(rect.x, rect.y, left, rect.h),
        Rect::new(rect.x + left, rect.y, rect.w - left, rect.h),
    )
}
pub(super) fn clipped_control(r: Rect, clip: [f32; 4]) -> Rect {
    let x = r.x.max(clip[0]);
    let y = r.y.max(clip[1]);
    Rect::new(
        x,
        y,
        ((r.x + r.w).min(clip[0] + clip[2]) - x).max(0.0),
        ((r.y + r.h).min(clip[1] + clip[3]) - y).max(0.0),
    )
}

pub(super) fn content_viewport(r: Rect, x: f32, width: f32, natural_y: f32) -> Rect {
    let top = natural_y.max(r.y).min(r.y + r.h);
    Rect::new(x, top, width, (r.y + r.h - top).max(0.0))
}

impl PullRequestInbox {
    pub(super) fn label(
        &self,
        sl: &mut Sugarloaf,
        text: &str,
        x: f32,
        y: f32,
        width: f32,
        opts: DrawOpts,
    ) -> f32 {
        let h = opts.font_size * 1.5;
        let rows = wrap(text, width, |s| sl.text_mut().measure(s, &opts));
        for (i, row) in rows.iter().enumerate() {
            draw_text_with_occlusion(
                sl,
                x,
                y + i as f32 * h,
                row,
                &opts,
                &self.occlusions,
            );
        }
        rows.len() as f32 * h
    }
    pub(super) fn button(
        &mut self,
        sl: &mut Sugarloaf,
        text: &str,
        x: f32,
        y: f32,
        max: f32,
        opts: DrawOpts,
        hit: Hit,
    ) -> f32 {
        let w = (sl.text_mut().measure(text, &opts) + 18.0).min(max.max(0.0));
        let r = Rect::new(x, y, w, opts.font_size * 2.2);
        let clip = opts.clip_rect.unwrap_or([r.x, r.y, r.w, r.h]);
        let r = clipped_control(r, clip);
        if r.w > 0.0 && r.h > 0.0 {
            self.hits.push((r, hit));
        }
        draw_text_with_occlusion(
            sl,
            x,
            y + 3.0,
            text,
            &DrawOpts {
                clip_rect: Some([r.x, r.y, r.w, r.h]),
                ..opts
            },
            &self.occlusions,
        );
        w
    }
    pub fn render(
        &mut self,
        sl: &mut Sugarloaf,
        rect: Rect,
        mono_font_id: usize,
        font_size: f32,
        theme: &IdeTheme,
    ) {
        self.hits.clear();
        self.child_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        let s = (font_size / 12.0).clamp(0.7, 2.5);
        sl.quad(
            None,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            theme.f32(theme.bg),
            [0.0; 4],
            0.0,
            20,
        );
        let opts = DrawOpts {
            font_id: crate::primitives::prose_font::ui_prose_font_id(sl.font_library()),
            font_size: 13.0 * s,
            bold: false,
            color: theme.u8(theme.fg),
            clip_rect: Some([rect.x, rect.y, rect.w, rect.h]),
            ..DrawOpts::default()
        };
        let (list, detail) = panes(rect, self.selected.is_some(), self.narrow_detail, s);
        self.list_rect = list;
        self.detail_rect = detail;
        if list.w > 0.0 {
            self.render_list(sl, list, opts, s, theme);
        }
        if detail.w > 0.0 {
            self.render_detail(sl, detail, opts, s, theme, font_size, mono_font_id);
        } else if let Some(c) = self.child() {
            c.pointer_leave();
        }
        self.render_menu(sl, opts, s, theme);
    }
    fn render_detail(
        &mut self,
        sl: &mut Sugarloaf,
        r: Rect,
        base: DrawOpts,
        s: f32,
        theme: &IdeTheme,
        font_size: f32,
        mono_font_id: usize,
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
        let mut y = r.y + 24.0 * s - self.detail_scroll;
        if self.list_rect.w > 0.0 {
            sl.quad(
                None,
                r.x,
                r.y,
                s,
                r.h,
                theme.f32_alpha(theme.fg, 0.08),
                [0.0; 4],
                0.0,
                20,
            );
        } else {
            self.button(sl, "← Pull Requests", x, y, width, opts, Hit::Back);
            y += 36.0 * s;
        }
        let Some(number) = self.selected else { return };
        let Some(item) = self.metadata.get(&number).cloned() else {
            if self.detail_loading(number) {
                skeleton(
                    sl,
                    Rect::new(x, y, width, r.h),
                    opts.clip_rect.unwrap(),
                    &self.occlusions,
                    theme,
                    s,
                    6,
                );
            }
            self.label(
                sl,
                self.detail_errors
                    .get(&number)
                    .map(String::as_str)
                    .unwrap_or(""),
                x,
                y,
                width,
                opts,
            );
            if self.detail_errors.contains_key(&number) {
                self.button(sl, "Retry", x, y + 36.0 * s, width, opts, Hit::Refresh);
            }
            return;
        };
        let repo_url = item
            .summary
            .url
            .split("/pull/")
            .next()
            .unwrap_or("")
            .to_string();
        let w = self.button(
            sl,
            &format!("{} ↗", item.repository),
            x,
            y,
            width,
            dim,
            Hit::Browser(repo_url),
        );
        if w + 80.0 * s > width {
            y += 28.0 * s;
            self.button(
                sl,
                &format!("#{} ↗", number),
                x,
                y,
                width,
                dim,
                Hit::Browser(item.summary.url.clone()),
            );
        } else {
            self.button(
                sl,
                &format!("#{} ↗", number),
                x + w,
                y,
                width - w,
                dim,
                Hit::Browser(item.summary.url.clone()),
            );
        }
        y += 36.0 * s;
        y += self.label(
            sl,
            &item.summary.title,
            x,
            y,
            width,
            DrawOpts {
                font_size: 23.0 * s,
                ..opts
            },
        ) + 12.0 * s;
        y += self.label(
            sl,
            &format!(
                "{} · {} · {}\n{} ← {}",
                item.summary.author,
                format::updated(&item.updated_at, format::now_seconds()),
                format::state_label(&item.summary.state, item.summary.draft),
                item.summary.base_branch,
                item.summary.head_branch
            ),
            x,
            y,
            width,
            dim,
        ) + 12.0 * s;
        let mut bx = x;
        let ready = self.detail_ready();
        let loaded = self.children.contains_key(&number);
        let retry = self.detail_errors.contains_key(&number);
        for (label, hit, enabled) in [
            ("Checkout", Hit::Checkout, ready && !self.has_drafts()),
            ("Review…", Hit::Review, ready),
            (
                if retry {
                    "Retry"
                } else if loaded {
                    "Refresh"
                } else {
                    ""
                },
                Hit::Refresh,
                ready || retry,
            ),
        ] {
            let w = sl.text_mut().measure(label, &dim) + 18.0;
            if label.is_empty() {
                skeleton(
                    sl,
                    Rect::new(
                        bx,
                        y + 6.0 * s,
                        (64.0 * s).min((x + width - bx).max(0.0)),
                        14.0 * s,
                    ),
                    opts.clip_rect.unwrap(),
                    &self.occlusions,
                    theme,
                    s,
                    1,
                );
                continue;
            }
            if bx + w > x + width && bx > x {
                bx = x;
                y += 28.0 * s;
            }
            if enabled {
                bx += self.button(sl, label, bx, y, x + width - bx, dim, hit);
            } else {
                draw_text_with_occlusion(
                    sl,
                    bx,
                    y + 3.0,
                    label,
                    &DrawOpts {
                        color: theme.u8(theme.muted),
                        clip_rect: Some([
                            bx,
                            y.max(r.y),
                            (x + width - bx).max(0.0),
                            ((y + 28.0 * s).min(r.y + r.h) - y.max(r.y)).max(0.0),
                        ]),
                        ..dim
                    },
                    &self.occlusions,
                );
                bx += w.min((x + width - bx).max(0.0));
            }
        }
        y += 38.0 * s;
        bx = x;
        for (label, tab) in [
            ("Summary", DetailTab::Summary),
            ("Timeline", DetailTab::Timeline),
            ("Code", DetailTab::Code),
        ] {
            let w = self.button(sl, label, bx, y, x + width - bx, opts, Hit::Tab(tab));
            if self.tab() == tab {
                for v in crate::primitives::visible_rect_fragments(
                    [bx, y + 27.0 * s, (w - 18.0).max(0.0), s],
                    opts.clip_rect.unwrap(),
                    &self.occlusions,
                ) {
                    sl.quad(
                        None,
                        v[0],
                        v[1],
                        v[2],
                        v[3],
                        theme.f32(theme.accent),
                        [0.0; 4],
                        0.0,
                        20,
                    );
                }
            }
            bx += w;
        }
        y += 42.0 * s;
        let mut notices = Vec::new();
        if let Some(error) = self.detail_errors.get(&number) {
            notices.push(error.clone());
        }
        if let Some(error) = &self.error {
            if !notices.contains(error) {
                notices.push(error.clone());
            }
        }
        if let Some(p) = self
            .children
            .get(&number)
            .and_then(|c| c.pull_request.as_ref())
        {
            if let Some(message) = &p.message {
                if !notices.contains(message) {
                    notices.push(message.clone());
                }
            }
            if p.stale {
                notices.push(
                    "Head changed. Drafts preserved; reopen a fresh review to submit."
                        .into(),
                );
            }
            if let Some(warning) = p.coverage_warning() {
                notices.push(warning.into());
            }
        }
        for notice in notices {
            y += self.label(
                sl,
                &notice,
                x,
                y,
                width,
                DrawOpts {
                    font_size: 11.0 * s,
                    color: theme.u8(theme.yellow),
                    ..opts
                },
            ) + 8.0 * s;
        }
        self.prelude_height = (y + self.detail_scroll - r.y).max(0.0);
        let content = if self.sheet_open() {
            r
        } else {
            content_viewport(r, x, width, y)
        };
        if self.tab() != DetailTab::Summary {
            self.detail_max = self.prelude_height;
            self.child_rect = content;
            if self.sheet_open() {
                self.hits.retain(|(hit, _)| !r.contains(hit.x, hit.y));
            }
            let occlusions = self.occlusions.clone();
            if let Some(c) = self.child() {
                c.set_text_occlusions(&occlusions);
                c.render(sl, content, mono_font_id, font_size, theme);
            } else {
                if self.detail_loading(number) {
                    skeleton(
                        sl,
                        content,
                        opts.clip_rect.unwrap(),
                        &self.occlusions,
                        theme,
                        s,
                        8,
                    );
                }
            }
            return;
        }
        let top = r.y;
        for text in [
            format!(
                "Labels: {}",
                if item.labels.is_empty() {
                    "None".into()
                } else {
                    item.labels.join(", ")
                }
            ),
            format!(
                "Reviewers: {}",
                if item.reviewers.is_empty() {
                    "None".into()
                } else {
                    item.reviewers.join(", ")
                }
            ),
        ] {
            y += self.label(
                sl,
                &text,
                x,
                y,
                width,
                DrawOpts {
                    color: theme.u8(theme.dim),
                    ..opts
                },
            ) + 10.0 * s;
        }
        y += 16.0 * s;
        if let Some(body) = self
            .children
            .get(&number)
            .and_then(|c| c.pull_request.as_ref())
            .and_then(|p| p.detail.as_ref())
            .map(|d| d.body.clone())
        {
            y = self.markdown(
                sl,
                if body.trim().is_empty() {
                    "No description provided"
                } else {
                    &body
                },
                x,
                y,
                width,
                opts,
                s,
                theme,
            );
        } else {
            if self.detail_loading(number) {
                skeleton(
                    sl,
                    Rect::new(x, y, width, r.h),
                    opts.clip_rect.unwrap(),
                    &self.occlusions,
                    theme,
                    s,
                    4,
                );
                y += 104.0 * s;
            }
            y += self.label(
                sl,
                self.detail_errors
                    .get(&number)
                    .map(String::as_str)
                    .unwrap_or(""),
                x,
                y,
                width,
                opts,
            );
        }
        self.detail_max = (y + self.detail_scroll - top - r.h).max(0.0);
        self.detail_scroll = self.detail_scroll.min(self.detail_max);
    }
    // The editor renderer requires a mutable MarkdownPane/CRDT; hosted prose must not create one.
    // Parse CommonMark into proportional inline runs, preserving semantic blocks and link targets.
    fn markdown(
        &mut self,
        sl: &mut Sugarloaf,
        body: &str,
        x: f32,
        mut y: f32,
        width: f32,
        opts: DrawOpts,
        s: f32,
        theme: &IdeTheme,
    ) -> f32 {
        let mut bx = x;
        let mut heading = false;
        let mut emphasis = false;
        let mut strong = false;
        let mut link: Option<String> = None;
        for event in Parser::new_ext(
            body,
            pulldown_cmark::Options::ENABLE_STRIKETHROUGH
                | pulldown_cmark::Options::ENABLE_TASKLISTS,
        ) {
            match event {
                Event::Start(Tag::Heading { .. }) => {
                    heading = true;
                }
                Event::Start(Tag::Emphasis) => emphasis = true,
                Event::Start(Tag::Strong) => strong = true,
                Event::Start(Tag::Link { dest_url, .. }) => {
                    link = Some(dest_url.to_string())
                }
                Event::Start(Tag::Item) => {
                    self.label(sl, "•", x, y, 18.0 * s, opts);
                    bx = x + 18.0 * s;
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    self.label(
                        sl,
                        "│",
                        x,
                        y,
                        18.0 * s,
                        DrawOpts {
                            color: theme.u8(theme.dim),
                            ..opts
                        },
                    );
                    bx = x + 18.0 * s;
                }
                Event::End(TagEnd::Heading(_)) => {
                    heading = false;
                    bx = x;
                    y += 32.0 * s;
                }
                Event::End(
                    TagEnd::Paragraph
                    | TagEnd::Item
                    | TagEnd::CodeBlock
                    | TagEnd::BlockQuote(_),
                ) => {
                    bx = x;
                    y += 28.0 * s;
                }
                Event::End(TagEnd::Emphasis) => emphasis = false,
                Event::End(TagEnd::Strong) => strong = false,
                Event::End(TagEnd::Link) => link = None,
                Event::SoftBreak => {
                    bx += sl.text_mut().measure(" ", &opts);
                }
                Event::HardBreak => {
                    bx = x;
                    y += 20.0 * s;
                }
                Event::Rule => {
                    bx = x;
                    y += 20.0 * s;
                }
                Event::TaskListMarker(done) => {
                    let t = if done { "☑ " } else { "☐ " };
                    self.label(sl, t, bx, y, 30.0 * s, opts);
                    bx += 24.0 * s;
                }
                Event::Text(text)
                | Event::Code(text)
                | Event::Html(text)
                | Event::InlineHtml(text) => {
                    let run = DrawOpts {
                        font_size: if heading {
                            19.0 * s
                        } else if strong {
                            14.0 * s
                        } else {
                            opts.font_size
                        },
                        italic: emphasis,
                        color: if link.is_some() {
                            theme.u8(theme.accent)
                        } else {
                            opts.color
                        },
                        ..opts
                    };
                    // Word runs retain inline styles while measuring/drawing identical options.
                    for word in text.split_inclusive(char::is_whitespace) {
                        for part in wrap(word, width, |t| sl.text_mut().measure(t, &run))
                        {
                            let w = sl.text_mut().measure(&part, &run);
                            if bx + w > x + width && bx > x {
                                bx = x;
                                y += run.font_size * 1.5;
                            }
                            draw_text_with_occlusion(
                                sl,
                                bx,
                                y,
                                &part,
                                &run,
                                &self.occlusions,
                            );
                            if let Some(url) = &link {
                                if url.starts_with("https://")
                                    || url.starts_with("http://")
                                {
                                    let clip = opts.clip_rect.unwrap();
                                    let top = y.max(clip[1]);
                                    let bottom =
                                        (y + run.font_size * 1.5).min(clip[1] + clip[3]);
                                    if bottom > top {
                                        self.hits.push((
                                            Rect::new(bx, top, w, bottom - top),
                                            Hit::Browser(url.clone()),
                                        ));
                                    }
                                }
                            }
                            bx += w;
                        }
                        if word.ends_with('\n') {
                            bx = x;
                            y += run.font_size * 1.5;
                        }
                    }
                }
                _ => {}
            }
        }
        y + 28.0 * s
    }
}
