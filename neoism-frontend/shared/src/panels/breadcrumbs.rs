// Breadcrumbs strip — the path-component row that sits between the
// buffer tabs and the editor body, identical in spirit to Zed/VSCode's
// `app › src › renderer › buffer.rs` row. Pure view: dispatcher pushes
// the active path each frame; we render `›`-separated segments in the
// muted text color, with the file leaf highlighted.
//
// Colors come from `IdeTheme` so the theme picker repaints the strip live.

use std::cell::RefCell;
use std::path::Path;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::file_tree::icons::icon_for_file;
use crate::primitives::{truncate_to_fit, IdeTheme};

// Folder glyph (Devicons U+F07B closed folder) drawn before each
// non-leaf segment so the breadcrumb reads as "directory ▸ directory ▸
// file" with type cues rather than bare strings. Same color as the
// file_tree folder glyphs so the row connects visually with the
// adjacent tree column.
const FOLDER_GLYPH: &str = "\u{f07b}";
const ICON_FONT_SIZE: f32 = 12.0;
const ICON_GAP: f32 = 10.0;
const ICON_BASELINE_LIFT: f32 = 1.5;

pub const BREADCRUMBS_HEIGHT: f32 = 26.0;
const FONT_SIZE: f32 = 11.5;
const PADDING_X: f32 = 14.0;
const SEGMENT_GAP: f32 = 6.0;
const SEPARATOR: &str = "›";

const DEPTH: f32 = 0.0;
const ORDER_BG: u8 = 18;
const ORDER_BUTTON: u8 = 19;
const ORDER_TEXT: u8 = 20;
const KERNEL_GLYPH: &str = "\u{f085}";
const CHEVRON_DOWN: &str = "\u{f078}";
const CHEVRON_UP: &str = "\u{f077}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreadcrumbAction {
    RunNotebookCell,
    RunNotebookCellAndBelow,
    RunAllNotebookCells,
    ClearNotebookCellOutput,
    ClearNotebookOutputs,
    InterruptNotebookKernel,
    RestartNotebookKernel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BreadcrumbActionItem {
    pub label: String,
    pub action: BreadcrumbAction,
}

impl BreadcrumbActionItem {
    pub fn new(label: impl Into<String>, action: BreadcrumbAction) -> Self {
        Self {
            label: label.into(),
            action,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BreadcrumbActionHit {
    rect: [f32; 4],
    action: BreadcrumbAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BreadcrumbKernelSelector {
    label: String,
    open: bool,
}

pub struct Breadcrumbs {
    visible: bool,
    /// Path components from project root down to the active file —
    /// dispatcher decides what root to anchor to (cwd by default).
    segments: Vec<String>,
    /// Trailing "winbar" tail appended after the file leaf — typically
    /// the enclosing function name + line:col, fed from the host's
    /// cursor-moved updates. None hides the tail entirely.
    tail: Option<String>,
    actions: Vec<BreadcrumbActionItem>,
    action_hits: RefCell<Vec<BreadcrumbActionHit>>,
    hovered_action: RefCell<Option<BreadcrumbAction>>,
    kernel_selector: Option<BreadcrumbKernelSelector>,
    kernel_hit: RefCell<Option<[f32; 4]>>,
    kernel_hovered: RefCell<bool>,
    scale: f32,
}

impl Breadcrumbs {
    pub fn new() -> Self {
        Breadcrumbs {
            visible: false,
            segments: Vec::new(),
            tail: None,
            actions: Vec::new(),
            action_hits: RefCell::new(Vec::new()),
            hovered_action: RefCell::new(None),
            kernel_selector: None,
            kernel_hit: RefCell::new(None),
            kernel_hovered: RefCell::new(false),
            scale: 1.0,
        }
    }

    /// Set the trailing winbar segment to the enclosing symbol name
    /// (function/method/class/struct/etc) — same surface as nvim-navic.
    /// Empty/whitespace symbol clears the tail entirely; we explicitly
    /// don't show line:col, the status bar already carries that.
    pub fn set_tail(&mut self, _line: u64, _col: u64, symbol: &str) {
        let sym = symbol.trim();
        if sym.is_empty() {
            self.tail = None;
        } else {
            self.tail = Some(sym.to_string());
        }
    }

    pub fn clear_tail(&mut self) {
        self.tail = None;
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
    }

    /// Effective row height in logical pixels, including hairline.
    pub fn height(&self) -> f32 {
        BREADCRUMBS_HEIGHT * self.scale
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    #[allow(dead_code)]
    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
    }

    #[allow(dead_code)]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Replace the segment list. Empty list hides the strip — no
    /// "ghost row" of just chevrons when there's no active buffer.
    pub fn set_segments(&mut self, segments: Vec<String>) {
        self.visible = !segments.is_empty();
        self.segments = segments;
        self.actions.clear();
        self.action_hits.borrow_mut().clear();
        self.kernel_selector = None;
        *self.kernel_hit.borrow_mut() = None;
        *self.hovered_action.borrow_mut() = None;
        *self.kernel_hovered.borrow_mut() = false;
    }

    pub fn set_actions(&mut self, actions: Vec<BreadcrumbActionItem>) {
        self.visible = !actions.is_empty();
        self.segments.clear();
        self.tail = None;
        let previous_hover = *self.hovered_action.borrow();
        let keep_hover = previous_hover
            .is_some_and(|hover| actions.iter().any(|item| item.action == hover));
        self.actions = actions;
        self.action_hits.borrow_mut().clear();
        if !keep_hover {
            *self.hovered_action.borrow_mut() = None;
        }
    }

    pub fn set_notebook_kernel(&mut self, label: Option<String>, open: bool) {
        self.kernel_selector = label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .map(|label| BreadcrumbKernelSelector { label, open });
        if self.kernel_selector.is_some() {
            self.visible = true;
        } else {
            *self.kernel_hit.borrow_mut() = None;
            *self.kernel_hovered.borrow_mut() = false;
        }
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<BreadcrumbAction> {
        self.action_hits
            .borrow()
            .iter()
            .find(|hit| point_in_rect(x, y, hit.rect))
            .map(|hit| hit.action)
    }

    pub fn set_action_hover_at(&self, x: f32, y: f32) -> bool {
        let next = self.action_at(x, y);
        let mut hovered = self.hovered_action.borrow_mut();
        if *hovered == next {
            return false;
        }
        *hovered = next;
        true
    }

    pub fn clear_action_hover(&self) -> bool {
        let mut hovered = self.hovered_action.borrow_mut();
        if hovered.is_none() {
            return false;
        }
        *hovered = None;
        true
    }

    pub fn action_hovered(&self) -> bool {
        self.hovered_action.borrow().is_some()
    }

    pub fn kernel_selector_at(&self, x: f32, y: f32) -> bool {
        self.kernel_hit
            .borrow()
            .as_ref()
            .is_some_and(|rect| point_in_rect(x, y, *rect))
    }

    pub fn set_kernel_hover_at(&self, x: f32, y: f32) -> bool {
        let next = self.kernel_selector_at(x, y);
        let mut hovered = self.kernel_hovered.borrow_mut();
        if *hovered == next {
            return false;
        }
        *hovered = next;
        true
    }

    pub fn clear_kernel_hover(&self) -> bool {
        let mut hovered = self.kernel_hovered.borrow_mut();
        if !*hovered {
            return false;
        }
        *hovered = false;
        true
    }

    pub fn kernel_hovered(&self) -> bool {
        *self.kernel_hovered.borrow()
    }

    pub fn clear_notebook_hover(&self) -> bool {
        self.clear_action_hover() | self.clear_kernel_hover()
    }

    /// Compute breadcrumbs for `path` rooted at `root` (typically the
    /// active editor pane's cwd). Falls back to the path's own
    /// components when `root` doesn't prefix `path`.
    pub fn set_from_path(&mut self, path: &Path, root: Option<&Path>) {
        let segments: Vec<String> = if let Some(root) = root {
            match path.strip_prefix(root) {
                Ok(rel) => rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect(),
                Err(_) => path
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect(),
            }
        } else {
            path.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect()
        };
        self.set_segments(segments);
    }

    pub fn render(
        &self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        theme: &IdeTheme,
    ) {
        self.render_with_options(sugarloaf, x_left, y_top, width, theme, true);
    }

    pub fn render_with_options(
        &self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        theme: &IdeTheme,
        draw_bottom_border: bool,
    ) {
        if !self.visible || width <= 0.0 {
            self.action_hits.borrow_mut().clear();
            *self.kernel_hit.borrow_mut() = None;
            *self.hovered_action.borrow_mut() = None;
            *self.kernel_hovered.borrow_mut() = false;
            return;
        }

        let row_h = self.height();
        let font_size = FONT_SIZE * self.scale;
        let padding_x = PADDING_X * self.scale;
        let segment_gap = SEGMENT_GAP * self.scale;

        sugarloaf.rect(
            None,
            x_left,
            y_top,
            width,
            row_h + 1.0,
            theme.f32(theme.bg),
            DEPTH,
            ORDER_BG,
        );
        if draw_bottom_border {
            sugarloaf.rect(
                None,
                x_left,
                y_top + row_h - 1.0,
                width,
                1.0,
                theme.f32(theme.border),
                DEPTH,
                ORDER_BG,
            );
        }

        if !self.actions.is_empty() {
            self.render_actions(sugarloaf, x_left, y_top, width, theme);
            return;
        }

        if self.segments.is_empty() {
            self.action_hits.borrow_mut().clear();
            *self.kernel_hit.borrow_mut() = None;
            *self.hovered_action.borrow_mut() = None;
            *self.kernel_hovered.borrow_mut() = false;
            return;
        }

        let last_ix = self.segments.len() - 1;
        let body_y = y_top + (row_h - font_size) / 2.0;
        let icon_size = ICON_FONT_SIZE * self.scale;
        let icon_gap = ICON_GAP * self.scale;
        let icon_y = y_top + (row_h - icon_size) / 2.0 - ICON_BASELINE_LIFT * self.scale;

        let mut cursor_x = x_left + padding_x;
        let max_x = x_left + width - padding_x;
        for (ix, seg) in self.segments.iter().enumerate() {
            let is_leaf = ix == last_ix;

            // Folder glyph for non-leaf segments, file-type glyph for the
            // leaf — same source-of-truth as the file_tree row icons so a
            // user reading the breadcrumb sees the exact same visual
            // language they'd see in the tree column.
            let (icon_glyph, icon_rgb) = if is_leaf {
                icon_for_file(seg)
            } else {
                (FOLDER_GLYPH, theme.u8(theme.folder))
            };
            let icon_opts = DrawOpts {
                font_size: icon_size,
                color: icon_rgb,
                ..DrawOpts::default()
            };
            let icon_w = sugarloaf.text_mut().measure(icon_glyph, &icon_opts);

            let opts = DrawOpts {
                font_size,
                color: if is_leaf {
                    theme.u8(theme.fg)
                } else {
                    theme.u8(theme.dim)
                },
                ..DrawOpts::default()
            };
            let seg_w = sugarloaf.text_mut().measure(seg, &opts);

            if cursor_x + icon_w + icon_gap + seg_w > max_x {
                break;
            }
            sugarloaf
                .text_mut()
                .draw(cursor_x, icon_y, icon_glyph, &icon_opts);
            cursor_x += icon_w + icon_gap;
            sugarloaf.text_mut().draw(cursor_x, body_y, seg, &opts);
            cursor_x += seg_w;

            if !is_leaf {
                let sep_opts = DrawOpts {
                    font_size,
                    color: theme.u8(theme.muted),
                    ..DrawOpts::default()
                };
                let sep_w = sugarloaf.text_mut().measure(SEPARATOR, &sep_opts);
                cursor_x += segment_gap;
                if cursor_x + sep_w > max_x {
                    break;
                }
                sugarloaf
                    .text_mut()
                    .draw(cursor_x, body_y, SEPARATOR, &sep_opts);
                cursor_x += sep_w + segment_gap;
            }
        }

        // True-winbar tail: enclosing symbol + Ln/Col, after a chevron
        // separator. Drawn in the muted color so the file leaf still
        // dominates the row visually. Skipped silently when there's no
        // room left in the strip.
        if let Some(tail) = self.tail.as_ref() {
            let sep_opts = DrawOpts {
                font_size,
                color: theme.u8(theme.muted),
                ..DrawOpts::default()
            };
            let tail_opts = DrawOpts {
                font_size,
                color: theme.u8(theme.dim),
                ..DrawOpts::default()
            };
            let sep_w = sugarloaf.text_mut().measure(SEPARATOR, &sep_opts);
            let tail_w = sugarloaf.text_mut().measure(tail, &tail_opts);
            let need = segment_gap + sep_w + segment_gap + tail_w;
            if cursor_x + need <= max_x {
                cursor_x += segment_gap;
                sugarloaf
                    .text_mut()
                    .draw(cursor_x, body_y, SEPARATOR, &sep_opts);
                cursor_x += sep_w + segment_gap;
                sugarloaf
                    .text_mut()
                    .draw(cursor_x, body_y, tail, &tail_opts);
            }
        }

        let _ = ORDER_TEXT;
    }

    fn render_actions(
        &self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        theme: &IdeTheme,
    ) {
        let row_h = self.height();
        let font_size = FONT_SIZE * self.scale;
        let padding_x = PADDING_X * self.scale;
        let button_gap = 6.0 * self.scale;
        let button_pad_x = 7.0 * self.scale;
        let icon_gap = ICON_GAP * self.scale;
        let icon_size = ICON_FONT_SIZE * self.scale;
        let button_h = 18.0 * self.scale;
        let button_y = y_top + (row_h - button_h) * 0.5;
        let label_y = y_top + (row_h - font_size) * 0.5;
        let icon_y = y_top + (row_h - icon_size) * 0.5 - ICON_BASELINE_LIFT * self.scale;
        let radius = 4.0 * self.scale;
        let hovered_action = *self.hovered_action.borrow();
        let kernel_hovered = *self.kernel_hovered.borrow();
        let mut tooltip: Option<([f32; 4], &'static str)> = None;
        let mut right_limit = x_left + width - padding_x;

        if let Some(kernel) = self.kernel_selector.as_ref() {
            let opts = DrawOpts {
                font_size,
                color: theme.u8(theme.fg),
                ..DrawOpts::default()
            };
            let icon_opts = DrawOpts {
                font_size: icon_size,
                color: theme.u8(theme.cyan),
                ..DrawOpts::default()
            };
            let chevron_opts = DrawOpts {
                font_size: icon_size,
                color: theme.u8(theme.muted),
                ..DrawOpts::default()
            };
            let chevron = if kernel.open {
                CHEVRON_UP
            } else {
                CHEVRON_DOWN
            };
            let icon_w = sugarloaf.text_mut().measure(KERNEL_GLYPH, &icon_opts);
            let chevron_w = sugarloaf.text_mut().measure(chevron, &chevron_opts);
            let max_kernel_w = (width * 0.36)
                .min(250.0 * self.scale)
                .max(118.0 * self.scale);
            let label_budget =
                (max_kernel_w - icon_w - chevron_w - icon_gap * 2.0 - button_pad_x * 2.0)
                    .max(20.0 * self.scale);
            let label = truncate_to_fit(&kernel.label, label_budget, sugarloaf, &opts);
            let label_w = sugarloaf.text_mut().measure(&label, &opts);
            let button_w =
                icon_w + icon_gap + label_w + icon_gap + chevron_w + button_pad_x * 2.0;
            let rect = [
                x_left + width - padding_x - button_w,
                button_y,
                button_w,
                button_h,
            ];
            right_limit = rect[0] - button_gap;
            *self.kernel_hit.borrow_mut() = Some(rect);
            sugarloaf.rounded_rect(
                None,
                rect[0],
                rect[1],
                rect[2],
                rect[3],
                if kernel_hovered || kernel.open {
                    theme.f32_alpha(theme.hover, 0.9)
                } else {
                    theme.f32(theme.surface)
                },
                DEPTH,
                radius,
                ORDER_BUTTON,
            );
            let content_x = rect[0] + button_pad_x;
            sugarloaf
                .text_mut()
                .draw(content_x, icon_y, KERNEL_GLYPH, &icon_opts);
            sugarloaf.text_mut().draw(
                content_x + icon_w + icon_gap,
                label_y,
                &label,
                &opts,
            );
            sugarloaf.text_mut().draw(
                content_x + icon_w + icon_gap + label_w + icon_gap,
                icon_y,
                chevron,
                &chevron_opts,
            );
            if kernel_hovered {
                tooltip = Some((rect, "Select Kernel"));
            }
        } else {
            *self.kernel_hit.borrow_mut() = None;
            *self.kernel_hovered.borrow_mut() = false;
        }

        {
            let mut hits = self.action_hits.borrow_mut();
            hits.clear();

            let mut cursor_x = x_left + padding_x;
            let max_x = right_limit.max(cursor_x);
            for item in &self.actions {
                let opts = DrawOpts {
                    font_size,
                    color: theme.u8(theme.fg),
                    ..DrawOpts::default()
                };
                let (icon, icon_color) = notebook_action_icon(item.action, theme);
                let icon_opts = DrawOpts {
                    font_size: icon_size,
                    color: icon_color,
                    ..DrawOpts::default()
                };
                let icon_w = sugarloaf.text_mut().measure(icon, &icon_opts);
                let label_w = sugarloaf.text_mut().measure(&item.label, &opts);
                let button_w = icon_w + icon_gap + label_w + button_pad_x * 2.0;
                if cursor_x + button_w > max_x {
                    break;
                }

                let hovered = hovered_action == Some(item.action);
                let rect = [cursor_x, button_y, button_w, button_h];
                sugarloaf.rounded_rect(
                    None,
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    if hovered {
                        theme.f32_alpha(theme.hover, 0.9)
                    } else {
                        theme.f32(theme.surface)
                    },
                    DEPTH,
                    radius,
                    ORDER_BUTTON,
                );
                sugarloaf.text_mut().draw(
                    cursor_x + button_pad_x,
                    icon_y,
                    icon,
                    &icon_opts,
                );
                sugarloaf.text_mut().draw(
                    cursor_x + button_pad_x + icon_w + icon_gap,
                    label_y,
                    &item.label,
                    &opts,
                );
                hits.push(BreadcrumbActionHit {
                    rect,
                    action: item.action,
                });
                if hovered {
                    tooltip = Some((rect, notebook_action_tooltip(item.action)));
                }
                cursor_x += button_w + button_gap;
            }
        }

        if let Some((rect, label)) = tooltip {
            draw_notebook_action_tooltip(
                sugarloaf,
                rect,
                label,
                [x_left, y_top, width, row_h],
                self.scale,
                theme,
            );
        }
    }
}

fn notebook_action_icon(
    action: BreadcrumbAction,
    theme: &IdeTheme,
) -> (&'static str, [u8; 4]) {
    match action {
        // Font Awesome glyphs from Nerd Font, matching the icon-first style used
        // in the rest of Neoism chrome.
        BreadcrumbAction::RunNotebookCell => ("\u{f04b}", theme.u8(theme.green)),
        BreadcrumbAction::RunNotebookCellAndBelow => ("\u{f063}", theme.u8(theme.blue)),
        BreadcrumbAction::RunAllNotebookCells => ("\u{f144}", theme.u8(theme.green)),
        BreadcrumbAction::ClearNotebookCellOutput => ("\u{f12d}", theme.u8(theme.yellow)),
        BreadcrumbAction::ClearNotebookOutputs => ("\u{f1f8}", theme.u8(theme.red)),
        BreadcrumbAction::InterruptNotebookKernel => ("\u{f04d}", theme.u8(theme.red)),
        BreadcrumbAction::RestartNotebookKernel => ("\u{f021}", theme.u8(theme.cyan)),
    }
}

fn notebook_action_tooltip(action: BreadcrumbAction) -> &'static str {
    match action {
        BreadcrumbAction::RunNotebookCell => "Run Cell",
        BreadcrumbAction::RunNotebookCellAndBelow => "Run Cell And Below",
        BreadcrumbAction::RunAllNotebookCells => "Run All Cells",
        BreadcrumbAction::ClearNotebookCellOutput => "Clear Cell Output",
        BreadcrumbAction::ClearNotebookOutputs => "Clear All Outputs",
        BreadcrumbAction::InterruptNotebookKernel => "Interrupt Kernel",
        BreadcrumbAction::RestartNotebookKernel => "Restart Kernel",
    }
}

fn draw_notebook_action_tooltip(
    sugarloaf: &mut Sugarloaf,
    anchor: [f32; 4],
    label: &str,
    bounds: [f32; 4],
    scale: f32,
    theme: &IdeTheme,
) {
    let font_size = 11.0 * scale;
    let opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        ..DrawOpts::default()
    };
    let pad_x = 7.0 * scale;
    let tooltip_h = 20.0 * scale;
    let tooltip_w = sugarloaf.text_mut().measure(label, &opts) + pad_x * 2.0;
    let margin = 4.0 * scale;
    let min_x = bounds[0] + margin;
    let max_x = (bounds[0] + bounds[2] - tooltip_w - margin).max(min_x);
    let tooltip_x = (anchor[0] + anchor[2] * 0.5 - tooltip_w * 0.5).clamp(min_x, max_x);
    let tooltip_y = bounds[1] + bounds[3] + margin;

    sugarloaf.rounded_rect(
        None,
        tooltip_x,
        tooltip_y,
        tooltip_w,
        tooltip_h,
        theme.f32(theme.surface),
        DEPTH,
        5.0 * scale,
        ORDER_BUTTON + 2,
    );
    sugarloaf.text_mut().draw(
        tooltip_x + pad_x,
        tooltip_y + (tooltip_h - font_size) * 0.5,
        label,
        &opts,
    );
}

fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

impl Default for Breadcrumbs {
    fn default() -> Self {
        Breadcrumbs::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn segments_relative_to_root() {
        let mut b = Breadcrumbs::new();
        let root = PathBuf::from("/proj");
        let file = PathBuf::from("/proj/src/lib.rs");
        b.set_from_path(&file, Some(&root));
        assert_eq!(b.segments(), &["src".to_string(), "lib.rs".to_string()]);
        assert!(b.is_visible());
    }

    #[test]
    fn segments_fall_back_when_root_mismatched() {
        let mut b = Breadcrumbs::new();
        let root = PathBuf::from("/other");
        let file = PathBuf::from("/proj/src/lib.rs");
        b.set_from_path(&file, Some(&root));
        // Falls back to absolute components.
        assert_eq!(
            b.segments(),
            &[
                "/".to_string(),
                "proj".to_string(),
                "src".to_string(),
                "lib.rs".to_string()
            ]
        );
    }

    #[test]
    fn empty_segments_hide_strip() {
        let mut b = Breadcrumbs::new();
        b.set_segments(Vec::new());
        assert!(!b.is_visible());
    }

    #[test]
    fn actions_replace_segments_and_show_strip() {
        let mut b = Breadcrumbs::new();
        b.set_from_path(Path::new("/proj/src/lib.rs"), Some(Path::new("/proj")));
        b.set_actions(vec![BreadcrumbActionItem::new(
            "Run",
            BreadcrumbAction::RunNotebookCell,
        )]);
        assert!(b.is_visible());
        assert!(b.segments().is_empty());
    }
}
