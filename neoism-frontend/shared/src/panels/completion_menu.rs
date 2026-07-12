//! Completion popup (LSP `pum_show`). Lifted verbatim from
//! `frontends/neoism/src/chrome/panels/completion_menu.rs` — same
//! animations, same layout math, same wheel-to-step accounting.
//!
//! The native version reads its anchor data through
//! `ContextManager<T>` (current grid → tab item → dimension &
//! layout_rect). The shared crate can't pull in `ContextManager` —
//! that type is winit/native-only — so the host packs the same data
//! into an `EditorAnchor` POD and hands the popup a reference. The
//! anchor row/col live on the snapshot's `PopupMenu` itself.
//!
//! TODO(wave-cutover): `wheel_steps` takes a winit-style scroll
//! delta. The native version accepts `neoism_window::event::MouseScrollDelta`;
//! the shared crate uses [`ScrollDelta`] which carries the same axis
//! data with a host-neutral name. Native callers translate at the
//! boundary — one mechanical match arm.

use web_time::Instant;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::animation::CriticallyDampedSpring;
use crate::editor_snapshot::PopupMenu;
use crate::panels::status_line::STATUS_LINE_HEIGHT;
use crate::primitives::IdeTheme;

const FONT_SIZE: f32 = 13.0;
const ROW_HEIGHT: f32 = 26.0;
const PADDING_X: f32 = 10.0;
const KIND_WIDTH: f32 = 58.0;
const MENU_WIDTH: f32 = 84.0;
const MIN_WIDTH: f32 = 280.0;
const MAX_WIDTH: f32 = 560.0;
const MAX_VISIBLE_ROWS: usize = 12;
const EDGE_GAP: f32 = 8.0;
const RADIUS: f32 = 8.0;
const DEPTH: f32 = 0.0;
const ORDER: u8 = 13;
const LIST_SCROLL_ANIMATION_LENGTH: f32 = 0.14;
const MAX_WHEEL_SELECTION_STEPS: i32 = 6;

/// Where the popup should anchor itself. Bundles the cell geometry and
/// panel rect the popup needs to translate `(anchor_row, anchor_col)`
/// into window-space pixels.
///
/// Built by the native shim per frame from `ContextManager`; the web
/// frontend builds it from its own buffer layout. The popup itself
/// only reads the POD fields below — it doesn't know about either.
#[derive(Clone, Copy, Debug)]
pub struct EditorAnchor {
    /// Logical-px width of one cell in the editor grid.
    pub cell_w: f32,
    /// Logical-px height of one cell in the editor grid.
    pub cell_h: f32,
    /// Physical-px left edge of the editor panel content area
    /// (i.e. after scaled padding is subtracted).
    pub panel_left_phys: f32,
    /// Physical-px top edge of the editor panel content area.
    pub panel_top_phys: f32,
    /// Visible line count of the editor panel — bounds the popup's
    /// "below" anchor.
    pub panel_lines: u32,
    /// Whether the focused buffer is an editor (not a terminal). The
    /// popup never renders for a terminal pane.
    pub editor_focused: bool,
}

/// Host-neutral mouse-wheel delta. Mirrors the two variants of
/// `winit::event::MouseScrollDelta` so the native shim can forward
/// scroll events without translation.
#[derive(Clone, Copy, Debug)]
pub enum ScrollDelta {
    /// Line-based delta (typically integer; sign matches OS convention).
    Lines { x: f32, y: f32 },
    /// Pixel-based delta from high-resolution trackpads.
    Pixels { x: f32, y: f32 },
}

pub struct CompletionMenu {
    scale: f32,
    list_scroll_spring: CriticallyDampedSpring,
    last_list_scroll_frame: Instant,
    last_first_visible: Option<usize>,
    last_menu_signature: Option<(u64, u64, u64, usize)>,
    wheel_accumulator: f32,
    /// Latest popup snapshot pushed by the host. `None` hides the menu.
    /// The chrome_shim_more `draw` shim reads this so the panel becomes
    /// data-driven instead of always painting an empty `PopupMenu`.
    stored_popup: Option<PopupMenu>,
    /// Latest editor anchor pushed by the host. The shim falls back to
    /// a synthesized anchor (panel-bounds based) when `None`.
    stored_anchor: Option<EditorAnchor>,
}

#[derive(Clone, Copy)]
struct CompletionMenuLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    row_h: f32,
    font_size: f32,
    pad_x: f32,
    kind_w: f32,
    menu_w: f32,
    visible_rows: usize,
}

impl CompletionMenu {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            list_scroll_spring: CriticallyDampedSpring::new(),
            last_list_scroll_frame: Instant::now(),
            last_first_visible: None,
            last_menu_signature: None,
            wheel_accumulator: 0.0,
            stored_popup: None,
            stored_anchor: None,
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
        self.reset_motion();
    }

    /// Replace the popup snapshot. `None` clears the menu (the
    /// `chrome_shim_more::draw` shim will paint nothing on the next
    /// frame).
    pub fn set_popup(&mut self, popup: Option<PopupMenu>) {
        self.stored_popup = popup;
    }

    /// Replace the editor anchor. The shim reads this when laying the
    /// popup out; missing anchor falls back to a panel-bounds anchor.
    pub fn set_anchor(&mut self, anchor: EditorAnchor) {
        self.stored_anchor = Some(anchor);
    }

    /// Read-only accessor used by the chrome shim.
    pub fn stored_popup(&self) -> Option<&PopupMenu> {
        self.stored_popup.as_ref()
    }

    /// Read-only accessor used by the chrome shim.
    pub fn stored_anchor(&self) -> Option<&EditorAnchor> {
        self.stored_anchor.as_ref()
    }

    /// Clear both popup and anchor — same effect as
    /// `set_popup(None)` plus forgetting the last anchor so the next
    /// `set_popup(Some(...))` re-installs both.
    pub fn dismiss(&mut self) {
        self.stored_popup = None;
        self.stored_anchor = None;
        self.reset_motion();
    }

    pub fn row_height(&self) -> f32 {
        ROW_HEIGHT * self.scale
    }

    pub fn contains_point(
        &self,
        menu: Option<&PopupMenu>,
        anchor: &EditorAnchor,
        dimensions: (f32, f32, f32),
        overlay_active: bool,
        mouse_x: f32,
        mouse_y: f32,
    ) -> bool {
        let Some((_menu, layout)) = self.layout(menu, anchor, dimensions, overlay_active)
        else {
            return false;
        };
        mouse_x >= layout.x
            && mouse_x <= layout.x + layout.width
            && mouse_y >= layout.y
            && mouse_y <= layout.y + layout.height
    }

    pub fn is_animating(&self) -> bool {
        self.list_scroll_spring.position.abs() > 0.5
    }

    /// Translate mouse wheel input into popup selection steps. Positive
    /// means next item (`<C-n>`), negative means previous (`<C-p>`).
    pub fn wheel_steps(&mut self, delta: &ScrollDelta) -> i32 {
        match delta {
            ScrollDelta::Lines { x: _, y } => (-*y).round() as i32,
            ScrollDelta::Pixels { x: _, y } => {
                let row_h = self.row_height().max(1.0);
                self.wheel_accumulator += -(*y);
                let mut steps = 0i32;
                while self.wheel_accumulator.abs() >= row_h {
                    let sign = self.wheel_accumulator.signum();
                    self.wheel_accumulator -= sign * row_h;
                    steps += if sign > 0.0 { 1 } else { -1 };
                }
                steps
            }
        }
        .clamp(-MAX_WHEEL_SELECTION_STEPS, MAX_WHEEL_SELECTION_STEPS)
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        menu: Option<&PopupMenu>,
        anchor: &EditorAnchor,
        dimensions: (f32, f32, f32),
        overlay_active: bool,
        theme: &IdeTheme,
    ) {
        let Some((menu, layout)) = self.layout(menu, anchor, dimensions, overlay_active)
        else {
            self.reset_motion();
            return;
        };

        let x = layout.x;
        let y = layout.y;
        let menu_w_total = layout.width;
        let menu_h = layout.height;
        let row_h = layout.row_h;
        let font_size = layout.font_size;
        let pad_x = layout.pad_x;
        let kind_w = layout.kind_w;
        let menu_w = layout.menu_w;
        let visible_rows = layout.visible_rows;

        sugarloaf.overlay_rounded_rect(
            x,
            y,
            menu_w_total,
            menu_h,
            theme.f32(theme.surface),
            DEPTH,
            RADIUS,
            ORDER,
        );
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            menu_w_total,
            1.0_f32.max(self.scale),
            theme.f32(theme.border),
            DEPTH,
            RADIUS,
            ORDER + 1,
        );

        let selected = menu.selected_index();
        let first = first_visible(selected, menu.items.len(), visible_rows);
        let list_scroll_offset =
            snap_to_device_px(self.tick_list_scroll(menu, first, row_h), dimensions.2);
        let overscan =
            ((list_scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
        let start = first.saturating_sub(overscan);
        let end = (first + visible_rows + overscan).min(menu.items.len());
        let menu_clip = [x, y, menu_w_total, menu_h];

        for absolute_ix in start..end {
            let item = &menu.items[absolute_ix];
            let visible_ix = absolute_ix as isize - first as isize;
            let row_y = y + visible_ix as f32 * row_h + list_scroll_offset;
            let row_bottom = row_y + row_h;
            if row_bottom <= y || row_y >= y + menu_h {
                continue;
            }
            let visible_y = row_y.max(y);
            let visible_h = row_bottom.min(y + menu_h) - visible_y;
            let selected_row = selected == Some(absolute_ix);
            if selected_row {
                sugarloaf.overlay_rect(
                    x,
                    visible_y,
                    menu_w_total,
                    visible_h,
                    theme.f32(theme.hover),
                    DEPTH,
                    ORDER + 1,
                );
                sugarloaf.overlay_rect(
                    x,
                    visible_y,
                    (3.0 * self.scale).max(2.0),
                    visible_h,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER + 2,
                );
            }

            let text_y = row_y + (row_h - font_size) / 2.0;
            let word_x = x + pad_x;
            let kind_x = x + menu_w_total - pad_x - kind_w - menu_w;
            let menu_x = x + menu_w_total - pad_x - menu_w;
            let word_budget = (kind_x - word_x - pad_x).max(font_size * 4.0);

            let word = truncate(&item.word, word_budget, font_size);
            // Prefer a colored nerd-font icon for the kind (VS Code / Zed
            // style); fall back to the short text tag for kinds we don't map.
            let icon = kind_icon(&item.kind);
            let kind = if icon.is_empty() {
                compact_kind(&item.kind)
            } else {
                icon
            };
            let menu_text = truncate(&item.menu, menu_w - pad_x, font_size);

            let word_opts = DrawOpts {
                font_size,
                color: if selected_row {
                    theme.u8(theme.fg)
                } else {
                    theme.u8(theme.dim)
                },
                clip_rect: Some(menu_clip),
                ..DrawOpts::default()
            };
            let kind_opts = DrawOpts {
                font_size,
                color: kind_color(&item.kind, theme),
                clip_rect: Some(menu_clip),
                ..DrawOpts::default()
            };
            let menu_opts = DrawOpts {
                font_size,
                color: if item.menu.is_empty() {
                    theme.u8(theme.muted)
                } else {
                    theme.u8(theme.dim)
                },
                clip_rect: Some(menu_clip),
                ..DrawOpts::default()
            };

            let ui = sugarloaf.overlay_text_mut();
            ui.draw(word_x, text_y, &word, &word_opts);
            if !kind.is_empty() {
                ui.draw(kind_x, text_y, kind, &kind_opts);
            }
            if !menu_text.is_empty() {
                ui.draw(menu_x, text_y, &menu_text, &menu_opts);
            }
        }

        if menu.items.len() > visible_rows {
            let style = crate::primitives::look::scrollbar_style();
            let ratio = visible_rows as f32 / menu.items.len() as f32;
            // Site's minimum thumb length is one row, not the shared
            // 20 px default, so only a pack override replaces it.
            let min_thumb = style.min_thumb.map_or(row_h, |m| m * self.scale);
            let thumb_h = (menu_h * ratio).max(min_thumb);
            let max_scroll = (menu.items.len() - visible_rows).max(1) as f32;
            let top_ratio = first as f32 / max_scroll;
            let thumb_y = y + (menu_h - thumb_h) * top_ratio;
            let thumb_w = style.width_or(2.0) * self.scale;
            let bar_x = x + menu_w_total - thumb_w - 1.0 * self.scale;
            let radius = style.radius(thumb_w, 0.0);
            if let Some(track) = style.track_or(None) {
                crate::widgets::scrollbar::draw_bar(
                    sugarloaf,
                    true,
                    bar_x,
                    y,
                    thumb_w,
                    menu_h,
                    track,
                    radius,
                    DEPTH,
                    ORDER + 2,
                );
            }
            let thumb_color = style.thumb_or(theme.f32_alpha(theme.muted, 0.85));
            crate::widgets::scrollbar::draw_bar(
                sugarloaf,
                true,
                bar_x,
                thumb_y,
                thumb_w,
                thumb_h,
                thumb_color,
                radius,
                DEPTH,
                ORDER + 2,
            );
        }
    }

    fn layout<'a>(
        &self,
        menu: Option<&'a PopupMenu>,
        anchor: &EditorAnchor,
        dimensions: (f32, f32, f32),
        overlay_active: bool,
    ) -> Option<(&'a PopupMenu, CompletionMenuLayout)> {
        if overlay_active {
            return None;
        }

        if !anchor.editor_focused {
            return None;
        }
        let menu = menu?;
        if menu.items.is_empty() {
            return None;
        }

        let (_window_w_phys, window_h_phys, scale_factor) = dimensions;
        let window_w = dimensions.0 / scale_factor;
        let window_h = window_h_phys / scale_factor;
        let usable_bottom = (window_h - STATUS_LINE_HEIGHT - EDGE_GAP).max(EDGE_GAP);
        let cell_w = anchor.cell_w.round().max(1.0);
        let cell_h = anchor.cell_h.round().max(1.0);
        let panel_left_phys = anchor.panel_left_phys.round();
        let panel_top_phys = anchor.panel_top_phys.round();
        let panel_bottom =
            (panel_top_phys + anchor.panel_lines.max(1) as f32 * cell_h) / scale_factor;

        let anchor_x = (panel_left_phys + menu.anchor_col as f32 * cell_w) / scale_factor;
        let anchor_below =
            (panel_top_phys + (menu.anchor_row as f32 + 1.0) * cell_h) / scale_factor;
        let anchor_above =
            (panel_top_phys + menu.anchor_row as f32 * cell_h) / scale_factor;

        let row_h = self.row_height();
        let font_size = FONT_SIZE * self.scale;
        let pad_x = PADDING_X * self.scale;
        let kind_w = KIND_WIDTH * self.scale;
        let menu_w = MENU_WIDTH * self.scale;
        let visible_rows = menu.items.len().min(MAX_VISIBLE_ROWS);
        let menu_h = visible_rows as f32 * row_h;
        let menu_w_total = self.menu_width(menu, font_size, kind_w, menu_w, pad_x);

        let mut x = anchor_x;
        if x + menu_w_total > window_w - EDGE_GAP {
            x = (window_w - menu_w_total - EDGE_GAP).max(EDGE_GAP);
        }
        x = x.max(EDGE_GAP);

        let room_below = panel_bottom - anchor_below;
        let mut y = if room_below < menu_h + EDGE_GAP && anchor_above > menu_h + EDGE_GAP
        {
            anchor_above - menu_h
        } else {
            anchor_below
        };
        if y + menu_h > usable_bottom {
            y = (usable_bottom - menu_h).max(EDGE_GAP);
        }
        y = y.max(EDGE_GAP);

        Some((
            menu,
            CompletionMenuLayout {
                x,
                y,
                width: menu_w_total,
                height: menu_h,
                row_h,
                font_size,
                pad_x,
                kind_w,
                menu_w,
                visible_rows,
            },
        ))
    }

    fn tick_list_scroll(&mut self, menu: &PopupMenu, first: usize, row_h: f32) -> f32 {
        let signature = menu_signature(menu);
        if self.last_menu_signature != Some(signature) {
            self.reset_motion();
            self.last_menu_signature = Some(signature);
            self.last_first_visible = Some(first);
            return 0.0;
        }

        if self.last_first_visible != Some(first) {
            if let Some(old_first) = self.last_first_visible {
                let was_idle = self.list_scroll_spring.position == 0.0;
                let rows = first as i32 - old_first as i32;
                self.list_scroll_spring.position += rows as f32 * row_h;
                if was_idle {
                    self.last_list_scroll_frame = Instant::now();
                }
            }
            self.last_first_visible = Some(first);
        }

        if self.list_scroll_spring.position == 0.0 {
            self.last_list_scroll_frame = Instant::now();
            return 0.0;
        }

        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_list_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_list_scroll_frame = now;
        self.list_scroll_spring
            .update(dt, LIST_SCROLL_ANIMATION_LENGTH);
        self.list_scroll_spring.position
    }

    fn reset_motion(&mut self) {
        self.list_scroll_spring.reset();
        self.last_list_scroll_frame = Instant::now();
        self.last_first_visible = None;
        self.last_menu_signature = None;
        self.wheel_accumulator = 0.0;
    }

    fn menu_width(
        &self,
        menu: &PopupMenu,
        font_size: f32,
        kind_w: f32,
        menu_w: f32,
        pad_x: f32,
    ) -> f32 {
        let max_word_chars = menu.max_word_chars.min(48) as f32;
        let word_w = max_word_chars * font_size * 0.58;
        (pad_x * 3.0 + word_w + kind_w + menu_w)
            .max(MIN_WIDTH * self.scale)
            .min(MAX_WIDTH * self.scale)
    }
}

impl Default for CompletionMenu {
    fn default() -> Self {
        Self::new()
    }
}

fn first_visible(selected: Option<usize>, len: usize, visible: usize) -> usize {
    if len <= visible {
        return 0;
    }
    let Some(selected) = selected else {
        return 0;
    };
    selected
        .saturating_sub(visible / 2)
        .min(len.saturating_sub(visible))
}

fn truncate(text: &str, budget_px: f32, font_size: f32) -> String {
    if text.is_empty() {
        return String::new();
    }
    let max_chars = (budget_px / (font_size * 0.58)).floor().max(1.0) as usize;
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('~');
    out
}

/// Nerd-font (Font Awesome slice) glyph for a completion kind — the same
/// range the rest of the UI draws from. Empty string ⇒ no icon, caller shows
/// the text tag instead. Accepts both the engine's lowercase LSP kind words
/// (`function`, `struct`, …) and nvim's capitalized variants.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "Function" | "function" | "Method" | "method" | "Constructor" | "constructor" => {
            "\u{f121}" // </>  code
        }
        "Struct" | "struct" | "Class" | "class" | "Object" | "object" => "\u{f1b2}", // cube
        "Interface" | "interface" | "TypeParameter" | "typeParameter"
        | "type_parameter" => {
            "\u{f0e8}" // sitemap
        }
        "Module" | "module" | "Namespace" | "namespace" | "Package" | "package" => {
            "\u{f1b3}" // cubes
        }
        "Field" | "field" | "Property" | "property" => "\u{f0c8}", // filled square
        "Variable" | "variable" | "Value" | "value" => "\u{f192}", // dot-circle
        "Enum" | "enum" => "\u{f0ca}",                             // list-ul
        "EnumMember" | "enumMember" | "enum_member" | "Constant" | "constant" => {
            "\u{f0a3}" // certificate
        }
        "Keyword" | "keyword" | "Operator" | "operator" | "Unit" | "unit" => "\u{f02b}", // tag
        "Snippet" | "snippet" => "\u{f0eb}", // lightbulb
        "Text" | "text" | "Reference" | "reference" => "\u{f02d}", // book
        "File" | "file" => "\u{f15b}",       // file
        "Folder" | "folder" => "\u{f07b}",   // folder
        "Color" | "color" => "\u{f1fc}",     // paint-brush
        "Event" | "event" => "\u{f0e7}",     // bolt
        _ => "",
    }
}

fn compact_kind(kind: &str) -> &str {
    match kind {
        "Function" | "function" => "Fn",
        "Method" | "method" => "Meth",
        "Variable" | "variable" => "Var",
        "Field" | "field" => "Field",
        "Property" | "property" => "Prop",
        "Class" | "class" => "Class",
        "Struct" | "struct" => "Struct",
        "Interface" | "interface" => "Iface",
        "Module" | "module" => "Mod",
        "Snippet" | "snippet" => "Snip",
        "Keyword" | "keyword" => "Key",
        "Text" | "text" => "Text",
        "Enum" | "enum" => "Enum",
        "EnumMember" | "enumMember" => "Member",
        "Constant" | "constant" => "Const",
        "TypeParameter" | "typeParameter" => "Type",
        other => other,
    }
}

fn menu_signature(menu: &PopupMenu) -> (u64, u64, u64, usize) {
    (
        menu.grid,
        menu.anchor_row as u64,
        menu.anchor_col as u64,
        menu.items.len(),
    )
}

#[inline]
fn snap_to_device_px(value: f32, scale_factor: f32) -> f32 {
    if scale_factor <= 0.0 {
        value
    } else {
        (value * scale_factor).round() / scale_factor
    }
}

fn kind_color(kind: &str, theme: &IdeTheme) -> [u8; 4] {
    match kind {
        "Function" | "function" | "Method" | "method" | "Constructor" | "constructor" => {
            theme.u8(theme.blue)
        }
        "Class" | "class" | "Struct" | "struct" | "Interface" | "interface" | "Enum"
        | "enum" | "TypeParameter" | "typeParameter" | "type_parameter" => {
            theme.u8(theme.yellow)
        }
        "Variable" | "variable" | "Field" | "field" | "Property" | "property"
        | "Value" | "value" => theme.u8(theme.red),
        "Keyword" | "keyword" | "Snippet" | "snippet" | "Operator" | "operator"
        | "Unit" | "unit" => theme.u8(theme.magenta),
        "Constant" | "constant" | "EnumMember" | "enumMember" | "enum_member" => {
            theme.u8(theme.yellow)
        }
        "Module" | "module" | "Namespace" | "namespace" | "Package" | "package" => {
            theme.u8(theme.blue)
        }
        "Text" | "text" | "Reference" | "reference" | "File" | "file" | "Folder"
        | "folder" => theme.u8(theme.green),
        "Event" | "event" | "Color" | "color" => theme.u8(theme.cyan),
        _ => theme.u8(theme.dim),
    }
}
