//! Tags view panel — ported from `frontends/neoism/src/workspace/tags_view.rs`.
//!
//! Renders a scrollable, click-to-toggle list of `#tag` groups with the
//! files they appear in. Native builds resolve the data from the local
//! `neoism-workspace-index` (sqlite-backed note graph); on wasm the
//! refresh path is a no-op and the panel stays empty until the daemon
//! pushes tag data over the wire.
//!
//! Lifted verbatim aside from import rewrites (sugarloaf reroute,
//! primitives reroute, `web_time::Instant` for wasm) and a cfg-gated
//! refresh path.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use web_time::Duration;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::{draw_text_with_occlusion, truncate_to_fit};

const REFRESH_AFTER: Duration = Duration::from_secs(2);
const DEPTH: f32 = 0.0;
const ORDER_BG: u8 = 17;
const ORDER_ROW: u8 = 18;

#[derive(Debug, Clone)]
pub struct NeoismTagsPane {
    path: PathBuf,
    workspace_root: PathBuf,
    tags: Vec<TagGroup>,
    expanded: BTreeSet<String>,
    row_hits: Vec<TagRowHit>,
    scroll_top: f32,
    content_height: f32,
    stale: bool,
    last_refresh: Option<Instant>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct TagGroup {
    tag: String,
    files: Vec<TagFile>,
}

#[derive(Debug, Clone)]
struct TagFile {
    path: PathBuf,
    label: String,
    title: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct TagRowHit {
    rect: [f32; 4],
    action: TagsViewAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagsViewAction {
    ToggleTag(String),
    OpenFile { path: PathBuf, line: usize },
}

impl NeoismTagsPane {
    pub fn new(path: PathBuf, workspace_root: PathBuf) -> Self {
        let mut pane = Self {
            path,
            workspace_root,
            tags: Vec::new(),
            expanded: BTreeSet::new(),
            row_hits: Vec::new(),
            scroll_top: 0.0,
            content_height: 0.0,
            stale: true,
            last_refresh: None,
            error: None,
        };
        pane.refresh();
        pane
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    pub fn refresh_if_needed(&mut self) -> bool {
        if !self.stale
            && self.last_refresh.is_some_and(|last| {
                Instant::now().saturating_duration_since(last) < REFRESH_AFTER
            })
        {
            return false;
        }
        self.refresh()
    }

    pub fn click_at(&mut self, x: f32, y: f32) -> Option<TagsViewAction> {
        let action = self
            .row_hits
            .iter()
            .find(|hit| point_in_rect(x, y, hit.rect))
            .map(|hit| hit.action.clone())?;
        if let TagsViewAction::ToggleTag(tag) = &action {
            if !self.expanded.remove(tag) {
                self.expanded.insert(tag.clone());
            }
        }
        Some(action)
    }

    pub fn scroll_by(&mut self, delta: f32, viewport_h: f32) {
        let max_scroll = (self.content_height - viewport_h).max(0.0);
        self.scroll_top = (self.scroll_top + delta).clamp(0.0, max_scroll);
    }

    fn refresh(&mut self) -> bool {
        self.stale = false;
        self.last_refresh = Some(Instant::now());
        match load_tag_groups(&self.workspace_root) {
            Ok(tags) => {
                self.tags = tags;
                self.error = None;
                self.expanded
                    .retain(|tag| self.tags.iter().any(|group| &group.tag == tag));
                true
            }
            Err(err) => {
                self.error = Some(err);
                true
            }
        }
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        rect: [f32; 4],
        theme: &IdeTheme,
        mouse: Option<[f32; 2]>,
        chrome_scale: f32,
        occlusion_rects: &[[f32; 4]],
    ) {
        let [x, y, w, h] = rect;
        if w <= 8.0 || h <= 8.0 {
            return;
        }

        self.row_hits.clear();
        let s = chrome_scale.clamp(0.75, 2.0);
        let clip = Some(rect);
        sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), DEPTH, ORDER_BG);

        let title_opts = DrawOpts {
            font_size: 22.0 * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let file_opts = DrawOpts {
            font_size: 14.0 * s,
            color: theme.u8_alpha(theme.fg, 0.82),
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let muted_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.dim),
            clip_rect: clip,
            ..DrawOpts::default()
        };
        let tag_opts = DrawOpts {
            font_size: 15.0 * s,
            color: theme.u8(theme.green),
            bold: true,
            clip_rect: clip,
            ..DrawOpts::default()
        };

        let pad_x = 30.0 * s;
        let top_pad = 28.0 * s;
        let tag_row_h = 38.0 * s;
        let file_row_h = 30.0 * s;
        let content_x = x + pad_x;
        let content_w = (w - pad_x * 2.0).max(120.0);
        let mut cursor_y = y + top_pad - self.scroll_top;

        if cursor_y + 34.0 * s >= y && cursor_y <= y + h {
            draw_text_with_occlusion(
                sugarloaf,
                content_x,
                cursor_y,
                "Tags",
                &title_opts,
                occlusion_rects,
            );
        }
        cursor_y += 46.0 * s;

        if let Some(error) = &self.error {
            let error_text = truncate_to_fit(error, content_w, sugarloaf, &muted_opts);
            draw_text_with_occlusion(
                sugarloaf,
                content_x,
                cursor_y,
                &error_text,
                &muted_opts,
                occlusion_rects,
            );
            self.content_height = cursor_y - y + 42.0 * s + self.scroll_top;
            return;
        }

        if self.tags.is_empty() {
            draw_text_with_occlusion(
                sugarloaf,
                content_x,
                cursor_y,
                "No tags",
                &muted_opts,
                occlusion_rects,
            );
            self.content_height = cursor_y - y + 42.0 * s + self.scroll_top;
            return;
        }

        for group in &self.tags {
            let row_rect = [
                content_x - 10.0 * s,
                cursor_y - 7.0 * s,
                content_w,
                tag_row_h,
            ];
            let hovered = mouse.is_some_and(|[mx, my]| point_in_rect(mx, my, row_rect));
            if row_rect[1] + row_rect[3] >= y && row_rect[1] <= y + h {
                if hovered {
                    sugarloaf.rect(
                        None,
                        row_rect[0],
                        row_rect[1],
                        row_rect[2],
                        row_rect[3],
                        theme.f32_alpha(theme.hover, 0.72),
                        DEPTH,
                        ORDER_ROW,
                    );
                }
                let expanded = self.expanded.contains(&group.tag);
                let chevron = if expanded { "\u{f078}" } else { "\u{f054}" };
                draw_text_with_occlusion(
                    sugarloaf,
                    content_x,
                    cursor_y,
                    chevron,
                    &muted_opts,
                    occlusion_rects,
                );
                draw_text_with_occlusion(
                    sugarloaf,
                    content_x + 24.0 * s,
                    cursor_y,
                    &format!("#{}", group.tag),
                    &tag_opts,
                    occlusion_rects,
                );
            }
            self.row_hits.push(TagRowHit {
                rect: row_rect,
                action: TagsViewAction::ToggleTag(group.tag.clone()),
            });
            cursor_y += tag_row_h;

            if !self.expanded.contains(&group.tag) {
                continue;
            }
            for file in &group.files {
                let file_rect = [
                    content_x + 22.0 * s,
                    cursor_y - 4.0 * s,
                    content_w - 22.0 * s,
                    file_row_h,
                ];
                let hovered =
                    mouse.is_some_and(|[mx, my]| point_in_rect(mx, my, file_rect));
                if file_rect[1] + file_rect[3] >= y && file_rect[1] <= y + h {
                    if hovered {
                        sugarloaf.rect(
                            None,
                            file_rect[0],
                            file_rect[1],
                            file_rect[2],
                            file_rect[3],
                            theme.f32_alpha(theme.hover, 0.56),
                            DEPTH,
                            ORDER_ROW,
                        );
                    }
                    let title = if file.title.trim().is_empty() {
                        file.label.as_str()
                    } else {
                        file.title.as_str()
                    };
                    let title_max = (content_w * 0.46).max(80.0);
                    let path_x = content_x + content_w * 0.5;
                    let line_x = content_x + content_w - 64.0 * s;
                    let title_text =
                        truncate_to_fit(title, title_max, sugarloaf, &file_opts);
                    let path_text = truncate_to_fit(
                        &file.label,
                        (line_x - path_x - 10.0 * s).max(40.0),
                        sugarloaf,
                        &muted_opts,
                    );
                    draw_text_with_occlusion(
                        sugarloaf,
                        content_x + 46.0 * s,
                        cursor_y,
                        &title_text,
                        &file_opts,
                        occlusion_rects,
                    );
                    draw_text_with_occlusion(
                        sugarloaf,
                        path_x,
                        cursor_y,
                        &path_text,
                        &muted_opts,
                        occlusion_rects,
                    );
                    draw_text_with_occlusion(
                        sugarloaf,
                        line_x,
                        cursor_y,
                        &format!(":{}", file.line),
                        &muted_opts,
                        occlusion_rects,
                    );
                }
                self.row_hits.push(TagRowHit {
                    rect: file_rect,
                    action: TagsViewAction::OpenFile {
                        path: file.path.clone(),
                        line: file.line,
                    },
                });
                cursor_y += file_row_h;
            }
            cursor_y += 4.0 * s;
        }
        self.content_height = cursor_y - y + self.scroll_top + 24.0 * s;
        let max_scroll = (self.content_height - h).max(0.0);
        self.scroll_top = self.scroll_top.clamp(0.0, max_scroll);
    }
}

// The persistent Obsidian-style tag/graph index is disabled for now.
#[cfg(not(target_arch = "wasm32"))]
fn load_tag_groups(_root: &Path) -> Result<Vec<TagGroup>, String> {
    Ok(Vec::new())
}

// Wasm: workspace-index pulls sqlx/tokio/notify (native-only). The
// web host receives tag data from the daemon, so until that channel
// is wired up the panel just renders the "No tags" empty state.
#[cfg(target_arch = "wasm32")]
fn load_tag_groups(_root: &Path) -> Result<Vec<TagGroup>, String> {
    Ok(Vec::new())
}

fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}
