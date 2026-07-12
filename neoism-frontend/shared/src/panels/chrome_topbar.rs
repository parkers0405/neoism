//! Window-top chrome bar.
//!
//! Sits above the buffer-tabs strip (which itself sits above the
//! optional workspace tabs row), giving the chrome a permanent home
//! for actions that don't belong to any individual buffer — analogous
//! to the macOS traffic-light row in Warp / VSCode title bars but
//! purely application-owned. Hosted in shared Rust so desktop and
//! web get identical paint + hit-testing.
//!
//! Current contents:
//!
//! - Left: panel toggle button. Uses the same codicon glyph (\u{eb56},
//!   "split-horizontal") that the bottom-right status pill uses for
//!   the split toggle, so the user reads "this opens / closes a side
//!   panel" without learning a new symbol. Clicking it dispatches
//!   [`TopBarAction::TogglePanel`], which the host wires to
//!   [`crate::chrome::Chrome::toggle_file_tree`].
//! - Right: hamburger menu (\u{f0c9}). Click toggles a dropdown that
//!   lists `Settings`, `Themes`, `Extensions`. Each item dispatches the
//!   matching `TopBarAction::Open*` variant; the host owns the actual
//!   destination screens (none yet — those land in a follow-up).
//!
//! The strip is render-only on the shared crate: it doesn't reach into
//! `Chrome` state. Chrome drains [`ChromeTopBar::take_action`] each
//! frame and applies the side effect.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::event::{PointerButton, UiEvent};
use crate::layout::{PanelLayout, Rect};
use crate::panels::{Panel, PanelContext};
use crate::primitives::IdeTheme;

pub const CHROME_TOPBAR_HEIGHT: f32 = 30.0;

const PANEL_BTN_GLYPH: &str = "\u{eb56}"; // codicon split-horizontal — same as status-line split pill
const HAMBURGER_GLYPH: &str = "\u{f0c9}"; // FA bars

/// Which half of the split-pane toggle glyph paints in the accent
/// color while its panel is open — the left half for the left (file
/// tree) button, the right half for the right (agent panel) button.
#[derive(Clone, Copy)]
enum ActiveHalf {
    Left,
    Right,
}

const ICON_FONT_SIZE: f32 = 13.0;
const MENU_FONT_SIZE: f32 = 12.5;
const EDGE_PAD_X: f32 = 8.0;
const BTN_GAP: f32 = 4.0;
const BTN_SIZE: f32 = 22.0;
const MENU_ITEM_HEIGHT: f32 = 26.0;
const MENU_WIDTH: f32 = 168.0;
const MENU_PAD_Y: f32 = 4.0;
const MENU_ITEM_PAD_X: f32 = 12.0;
/// Corner radius for the dropdown card.
const MENU_RADIUS: f32 = 8.0;

const DEPTH: f32 = 0.0;
const ORDER_BG: u8 = 4;
const ORDER_HOVER: u8 = 5;
const ORDER_ICON: u8 = 7;
const ORDER_MENU_BG: u8 = 30;
const ORDER_MENU_HOVER: u8 = 31;
const ORDER_MENU_TEXT: u8 = 32;
const ORDER_MENU_BORDER: u8 = 33;

/// Side effects the top bar can request. Chrome drains these via
/// [`ChromeTopBar::take_action`] each frame and applies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopBarAction {
    TogglePanel,
    /// Right-side button — toggles the agent side panel. Only fires
    /// when the host has enabled the right button via
    /// `set_right_button_visible(true)`.
    ToggleRightPanel,
    OpenSettings,
    OpenWorkspaces,
    StartWebServer,
    OpenThemes,
    OpenExtensions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    Settings,
    Workspaces,
    StartWebServer,
    Themes,
    Extensions,
}

impl MenuItem {
    const ALL: [MenuItem; 5] = [
        MenuItem::Settings,
        MenuItem::Workspaces,
        MenuItem::StartWebServer,
        MenuItem::Themes,
        MenuItem::Extensions,
    ];

    fn label(self) -> &'static str {
        match self {
            MenuItem::Settings => "Settings",
            MenuItem::Workspaces => "Workspaces",
            MenuItem::StartWebServer => "Start Web Server",
            MenuItem::Themes => "Themes",
            MenuItem::Extensions => "Extensions",
        }
    }

    fn action(self) -> TopBarAction {
        match self {
            MenuItem::Settings => TopBarAction::OpenSettings,
            MenuItem::Workspaces => TopBarAction::OpenWorkspaces,
            MenuItem::StartWebServer => TopBarAction::StartWebServer,
            MenuItem::Themes => TopBarAction::OpenThemes,
            MenuItem::Extensions => TopBarAction::OpenExtensions,
        }
    }
}

/// Per-instance state for the top bar.
pub struct ChromeTopBar {
    visible: bool,
    scale: f32,
    menu_open: bool,
    /// True when the host has an agent side panel to toggle. Drives
    /// whether the right-edge button is painted + hit-tested.
    right_button_visible: bool,
    /// Open/closed state of the panels the two toggle buttons drive
    /// (left = file tree, right = agent side panel). When `true` the
    /// button paints in the active accent style so the user can see at
    /// a glance which panels are open. Hosts push these every frame.
    panel_open: bool,
    right_panel_open: bool,
    left_safe_inset: f32,
    /// Last hit rects (in window-global coords) refreshed every paint.
    /// Hit-testing reads these, so layout drift between frames can't
    /// activate the wrong region.
    panel_btn_rect: Rect,
    menu_btn_rect: Rect,
    right_btn_rect: Rect,
    menu_rect: Rect,
    hover_panel_btn: bool,
    hover_menu_btn: bool,
    hover_right_btn: bool,
    hover_menu_item: Option<usize>,
    pending_action: Option<TopBarAction>,
}

impl ChromeTopBar {
    pub fn new() -> Self {
        Self {
            visible: true,
            scale: 1.0,
            menu_open: false,
            right_button_visible: false,
            panel_open: false,
            right_panel_open: false,
            left_safe_inset: 0.0,
            panel_btn_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            menu_btn_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            right_btn_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            menu_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            hover_panel_btn: false,
            hover_menu_btn: false,
            hover_right_btn: false,
            hover_menu_item: None,
            pending_action: None,
        }
    }

    /// Show / hide the right-side panel toggle button. Hosts call
    /// this with `true` when an agent side panel exists.
    pub fn set_right_button_visible(&mut self, v: bool) {
        self.right_button_visible = v;
        if !v {
            self.hover_right_btn = false;
        }
    }

    pub fn is_right_button_visible(&self) -> bool {
        self.right_button_visible
    }

    /// Push the open/closed state of the left panel-toggle target (the
    /// file tree) so the button can paint in its active accent style.
    pub fn set_panel_open(&mut self, open: bool) {
        self.panel_open = open;
    }

    /// Push the open/closed state of the right toggle target (the agent
    /// side panel) so the button can paint in its active accent style.
    pub fn set_right_panel_open(&mut self, open: bool) {
        self.right_panel_open = open;
    }

    pub fn set_left_safe_inset(&mut self, inset: f32) {
        self.left_safe_inset = inset.max(0.0);
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn height(&self) -> f32 {
        CHROME_TOPBAR_HEIGHT * self.scale
    }

    /// Vertical space the bar should reserve in the chrome layout.
    /// Always the bar's strip height — the dropdown overlays the
    /// chrome below without pushing it down.
    pub fn layout_reservation(&self) -> f32 {
        self.height()
    }

    /// Height of the dropdown card itself.
    pub fn menu_height(&self) -> f32 {
        let item_h = MENU_ITEM_HEIGHT * self.scale;
        let pad_y = MENU_PAD_Y * self.scale;
        item_h * MenuItem::ALL.len() as f32 + pad_y * 2.0
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
        if !v {
            self.menu_open = false;
        }
    }

    pub fn is_menu_open(&self) -> bool {
        self.menu_open
    }

    pub fn close_menu(&mut self) {
        self.menu_open = false;
    }

    /// Window-rect (in global coords) the dropdown occupies while open.
    /// `None` when the menu is closed. Chrome reads this to extend the
    /// pointer-event hit area beyond the strip itself.
    pub fn menu_overlay_rect(&self) -> Option<Rect> {
        if self.menu_open {
            Some(self.menu_rect)
        } else {
            None
        }
    }

    /// Drain the single pending action (if any). Chrome calls this
    /// each frame after `handle_event` and dispatches the side effect.
    pub fn take_action(&mut self) -> Option<TopBarAction> {
        self.pending_action.take()
    }

    fn menu_overlay_rect_for(&self, menu_btn: Rect, strip: Rect) -> Rect {
        let scale = self.scale;
        let menu_w = MENU_WIDTH * scale;
        let item_h = MENU_ITEM_HEIGHT * scale;
        let pad_y = MENU_PAD_Y * scale;
        let menu_h = item_h * MenuItem::ALL.len() as f32 + pad_y * 2.0;
        // Anchor below the hamburger, clamped inside the viewport so a
        // narrow window never pushes the card off-screen.
        let menu_x = menu_btn.x.min(strip.x + strip.w - menu_w).max(strip.x);
        let menu_y = strip.y + strip.h;
        Rect::new(menu_x, menu_y, menu_w, menu_h)
    }

    fn refresh_rects(&mut self, strip: Rect) {
        let scale = self.scale;
        let btn = BTN_SIZE * scale;
        let edge = EDGE_PAD_X * scale;
        let gap = BTN_GAP * scale;
        let cy = strip.y + (strip.h - btn) * 0.5;
        let left_x = strip.x + edge + self.left_safe_inset * scale;
        self.panel_btn_rect = Rect::new(left_x, cy, btn, btn);
        self.menu_btn_rect = Rect::new(left_x + btn + gap, cy, btn, btn);
        self.right_btn_rect = if self.right_button_visible {
            Rect::new(strip.x + strip.w - edge - btn, cy, btn, btn)
        } else {
            Rect::new(0.0, 0.0, 0.0, 0.0)
        };
        self.menu_rect = self.menu_overlay_rect_for(self.menu_btn_rect, strip);
    }

    fn menu_item_rect(&self, idx: usize) -> Rect {
        let scale = self.scale;
        let item_h = MENU_ITEM_HEIGHT * scale;
        let pad_y = MENU_PAD_Y * scale;
        Rect::new(
            self.menu_rect.x,
            self.menu_rect.y + pad_y + item_h * idx as f32,
            self.menu_rect.w,
            item_h,
        )
    }

    /// Public pointer-move entry point — desktop callers feed mouse
    /// state directly without going through `Panel::handle_event` /
    /// `PanelContext`. Web routes through `handle_event` already.
    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.handle_pointer_move(x, y);
    }

    /// Public left-button-down entry point. Returns `true` when the
    /// click landed on the strip or on the open dropdown (and the
    /// caller should consider it consumed). The resulting
    /// [`TopBarAction`] — if any — is queued for `take_action`.
    pub fn pointer_down(&mut self, x: f32, y: f32) -> bool {
        let had_open_menu = self.menu_open;
        let handled = self.handle_pointer_down(x, y);
        let inside_strip = self.panel_btn_rect.contains(x, y)
            || self.menu_btn_rect.contains(x, y)
            || (self.right_button_visible && self.right_btn_rect.contains(x, y))
            || self
                .menu_overlay_rect()
                .map(|r| r.contains(x, y))
                .unwrap_or(false);
        handled || inside_strip || had_open_menu
    }

    fn handle_pointer_move(&mut self, x: f32, y: f32) {
        self.hover_panel_btn = self.panel_btn_rect.contains(x, y);
        self.hover_menu_btn = self.menu_btn_rect.contains(x, y);
        self.hover_right_btn =
            self.right_button_visible && self.right_btn_rect.contains(x, y);
        self.hover_menu_item = if self.menu_open {
            (0..MenuItem::ALL.len()).find(|i| self.menu_item_rect(*i).contains(x, y))
        } else {
            None
        };
    }

    fn handle_pointer_down(&mut self, x: f32, y: f32) -> bool {
        if self.panel_btn_rect.contains(x, y) {
            self.pending_action = Some(TopBarAction::TogglePanel);
            self.menu_open = false;
            return true;
        }
        if self.menu_btn_rect.contains(x, y) {
            self.menu_open = !self.menu_open;
            return true;
        }
        if self.right_button_visible && self.right_btn_rect.contains(x, y) {
            self.pending_action = Some(TopBarAction::ToggleRightPanel);
            self.menu_open = false;
            return true;
        }
        if self.menu_open {
            if let Some(idx) =
                (0..MenuItem::ALL.len()).find(|i| self.menu_item_rect(*i).contains(x, y))
            {
                self.pending_action = Some(MenuItem::ALL[idx].action());
                self.menu_open = false;
                return true;
            }
            // Click anywhere else (still routed to us via the menu
            // overlay rect) closes the menu without firing an action.
            self.menu_open = false;
        }
        false
    }

    /// Paint the strip + (optionally) the open dropdown. Mirrors the
    /// breadcrumbs/status-line shape: free-function render so the
    /// chrome can call it without going through a `PanelContext`.
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        theme: &IdeTheme,
    ) {
        if !self.visible || width <= 0.0 {
            return;
        }
        let strip = Rect::new(x_left, y_top, width, self.height());
        self.refresh_rects(strip);

        let row_h = strip.h;

        // Background + hairline border at the bottom edge — matches the
        // breadcrumbs strip so the two rows read as one stacked unit
        // when both are visible.
        sugarloaf.rect(
            None,
            strip.x,
            strip.y,
            strip.w,
            row_h - 1.0,
            theme.f32(theme.bg),
            DEPTH,
            ORDER_BG,
        );
        sugarloaf.rect(
            None,
            strip.x,
            strip.y + row_h - 1.0,
            strip.w,
            1.0,
            theme.f32(theme.border),
            DEPTH,
            ORDER_BG,
        );

        // Panel-toggle button (left) — its left pane fills with the
        // accent color while the file tree is open.
        self.draw_icon_button(
            sugarloaf,
            self.panel_btn_rect,
            PANEL_BTN_GLYPH,
            self.hover_panel_btn,
            self.panel_open.then_some(ActiveHalf::Left),
            theme,
        );
        // Hamburger button (left, right of the panel toggle).
        self.draw_icon_button(
            sugarloaf,
            self.menu_btn_rect,
            HAMBURGER_GLYPH,
            self.hover_menu_btn || self.menu_open,
            None,
            theme,
        );
        // Right-side agent panel toggle. Same glyph as the left panel
        // toggle so the user reads "open / close a side panel" without
        // a second symbol. Only rendered when the host has flagged an
        // agent side panel as present; its right pane fills with the
        // accent color while that panel is open.
        if self.right_button_visible {
            self.draw_icon_button(
                sugarloaf,
                self.right_btn_rect,
                PANEL_BTN_GLYPH,
                self.hover_right_btn,
                self.right_panel_open.then_some(ActiveHalf::Right),
                theme,
            );
        }

        if self.menu_open {
            self.draw_menu(sugarloaf, theme);
        }
    }

    fn draw_icon_button(
        &self,
        sugarloaf: &mut Sugarloaf,
        rect: Rect,
        glyph: &str,
        hovered: bool,
        active_half: Option<ActiveHalf>,
        theme: &IdeTheme,
    ) {
        if hovered {
            sugarloaf.rect(
                None,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                theme.f32_alpha(theme.hover, 0.85),
                DEPTH,
                ORDER_HOVER,
            );
        }
        let icon_size = ICON_FONT_SIZE * self.scale;
        let opts = DrawOpts {
            font_size: icon_size,
            color: if hovered {
                theme.u8(theme.fg)
            } else {
                theme.u8(theme.dim)
            },
            ..DrawOpts::default()
        };
        let glyph_w = sugarloaf.text_mut().measure(glyph, &opts);
        let gx = rect.x + (rect.w - glyph_w) * 0.5;
        let gy = rect.y + (rect.h - icon_size) * 0.5;
        // Base glyph in the neutral color.
        sugarloaf.text_mut().draw(gx, gy, glyph, &opts);
        // When the panel is open, repaint the matching half of the
        // glyph itself in the accent color (clipped down the vertical
        // centre) — the icon's own left/right pane fills in, no extra
        // chrome behind it.
        if let Some(half) = active_half {
            let half_w = glyph_w * 0.5;
            let clip_x = match half {
                ActiveHalf::Left => gx,
                ActiveHalf::Right => gx + half_w,
            };
            let accent_opts = DrawOpts {
                font_size: icon_size,
                color: theme.u8(theme.accent),
                clip_rect: Some([clip_x, rect.y, half_w, rect.h]),
                ..DrawOpts::default()
            };
            sugarloaf.text_mut().draw(gx, gy, glyph, &accent_opts);
        }
        let _ = ORDER_ICON;
    }

    fn draw_menu(&self, sugarloaf: &mut Sugarloaf, theme: &IdeTheme) {
        let menu = self.menu_rect;
        let radius = MENU_RADIUS * self.scale;

        sugarloaf.overlay_rounded_rect(
            menu.x,
            menu.y,
            menu.w,
            menu.h,
            theme.f32(theme.panel_bg()),
            DEPTH,
            radius,
            ORDER_MENU_BG,
        );
        let border = theme.f32(theme.border);
        let bw = self.scale.max(1.0);
        sugarloaf.overlay_rect(
            menu.x,
            menu.y,
            menu.w,
            bw,
            border,
            DEPTH,
            ORDER_MENU_BORDER,
        );
        sugarloaf.overlay_rect(
            menu.x,
            menu.y + menu.h - bw,
            menu.w,
            bw,
            border,
            DEPTH,
            ORDER_MENU_BORDER,
        );
        sugarloaf.overlay_rect(
            menu.x,
            menu.y,
            bw,
            menu.h,
            border,
            DEPTH,
            ORDER_MENU_BORDER,
        );
        sugarloaf.overlay_rect(
            menu.x + menu.w - bw,
            menu.y,
            bw,
            menu.h,
            border,
            DEPTH,
            ORDER_MENU_BORDER,
        );

        let font = MENU_FONT_SIZE * self.scale;
        let last_ix = MenuItem::ALL.len() - 1;
        for (i, item) in MenuItem::ALL.iter().enumerate() {
            let row = self.menu_item_rect(i);
            let hovered = self.hover_menu_item == Some(i);
            if hovered {
                // First / last rows trace the card's rounded corners;
                // middle rows are square. Inset slightly so the hover
                // fill never visually extends past the card edge.
                let row_radius = if i == 0 || i == last_ix {
                    (radius * 0.6).max(0.0)
                } else {
                    0.0
                };
                let inset_x = 4.0 * self.scale;
                let hover_rect = Rect::new(
                    row.x + inset_x,
                    row.y,
                    (row.w - inset_x * 2.0).max(0.0),
                    row.h,
                );
                sugarloaf.overlay_rounded_rect(
                    hover_rect.x,
                    hover_rect.y,
                    hover_rect.w,
                    hover_rect.h,
                    theme.f32(theme.hover),
                    DEPTH,
                    row_radius,
                    ORDER_MENU_HOVER,
                );
            }
            let opts = DrawOpts {
                font_size: font,
                color: theme.u8(theme.fg),
                ..DrawOpts::default()
            };
            let tx = row.x + MENU_ITEM_PAD_X * self.scale;
            let ty = row.y + (row.h - font) * 0.5;
            sugarloaf
                .overlay_text_mut()
                .draw(tx, ty, item.label(), &opts);
        }
        let _ = ORDER_MENU_TEXT;
    }
}

impl Default for ChromeTopBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel for ChromeTopBar {
    fn handle_event(&mut self, event: &UiEvent, _ctx: &mut PanelContext) {
        match event {
            UiEvent::PointerMove { x, y, .. } => {
                self.handle_pointer_move(*x, *y);
            }
            UiEvent::PointerDown {
                button: PointerButton::Left,
                x,
                y,
                ..
            } => {
                let _ = self.handle_pointer_down(*x, *y);
            }
            UiEvent::PointerLeave => {
                self.hover_panel_btn = false;
                self.hover_menu_btn = false;
                self.hover_menu_item = None;
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        _sugarloaf: &mut Sugarloaf,
        _layout: &PanelLayout,
        _ctx: &PanelContext,
    ) {
        // Painted via the inherent `render` from `Chrome::draw` so the
        // active IdeTheme is in scope. The trait impl is here only so
        // the panel can sit in event-dispatch infrastructure that
        // expects `dyn Panel`.
    }

    fn name(&self) -> &str {
        "chrome_topbar"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_strip(bar: &mut ChromeTopBar, strip: Rect) {
        // Skip sugarloaf — only refresh hit rects, which is the part
        // the tests exercise.
        bar.refresh_rects(strip);
    }

    #[test]
    fn panel_button_click_queues_toggle() {
        let mut bar = ChromeTopBar::new();
        let strip = Rect::new(0.0, 0.0, 800.0, CHROME_TOPBAR_HEIGHT);
        paint_strip(&mut bar, strip);
        let btn = bar.panel_btn_rect;
        bar.handle_pointer_down(btn.x + btn.w * 0.5, btn.y + btn.h * 0.5);
        assert_eq!(bar.take_action(), Some(TopBarAction::TogglePanel));
        assert!(!bar.is_menu_open());
    }

    #[test]
    fn left_safe_inset_offsets_left_icons_only() {
        let mut bar = ChromeTopBar::new();
        bar.set_left_safe_inset(76.0);
        let strip = Rect::new(0.0, 0.0, 800.0, CHROME_TOPBAR_HEIGHT);
        paint_strip(&mut bar, strip);

        assert!(bar.menu_btn_rect.x > bar.panel_btn_rect.x);
        assert_eq!(bar.panel_btn_rect.x, EDGE_PAD_X + 76.0);
    }

    #[test]
    fn hamburger_toggles_menu_and_items_dispatch() {
        let mut bar = ChromeTopBar::new();
        let strip = Rect::new(0.0, 0.0, 800.0, CHROME_TOPBAR_HEIGHT);
        paint_strip(&mut bar, strip);
        let btn = bar.menu_btn_rect;
        bar.handle_pointer_down(btn.x + btn.w * 0.5, btn.y + btn.h * 0.5);
        assert!(bar.is_menu_open());
        assert_eq!(bar.take_action(), None);

        // Click the "Themes" item. `MenuItem::ALL` order is
        // [Settings, Workspaces, StartWebServer, Themes, Extensions],
        // so Themes is index 3 (the comment + index here were stale from
        // before Workspaces / Start Web Server were inserted above it).
        let themes_ix = MenuItem::ALL
            .iter()
            .position(|item| item.action() == TopBarAction::OpenThemes)
            .expect("Themes menu item present");
        let item = bar.menu_item_rect(themes_ix);
        bar.handle_pointer_down(item.x + 4.0, item.y + 4.0);
        assert_eq!(bar.take_action(), Some(TopBarAction::OpenThemes));
        assert!(!bar.is_menu_open());
    }

    #[test]
    fn click_outside_open_menu_closes_it() {
        let mut bar = ChromeTopBar::new();
        let strip = Rect::new(0.0, 0.0, 800.0, CHROME_TOPBAR_HEIGHT);
        paint_strip(&mut bar, strip);
        let btn = bar.menu_btn_rect;
        bar.handle_pointer_down(btn.x + btn.w * 0.5, btn.y + btn.h * 0.5);
        assert!(bar.is_menu_open());

        // Click somewhere outside both icons and outside menu items.
        bar.handle_pointer_down(strip.x + strip.w * 0.5, strip.y + strip.h * 0.5);
        assert!(!bar.is_menu_open());
        assert_eq!(bar.take_action(), None);
    }
}
