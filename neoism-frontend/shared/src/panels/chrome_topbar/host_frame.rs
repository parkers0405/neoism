//! The original top-bar renderer and hit regions for another native GPU host.
use super::*;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct ChromeRegion {
    pub id: String,
    pub label: String,
    pub rect: [f32; 4],
    pub menu: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ChromeHostFrame {
    pub regions: Vec<ChromeRegion>,
    pub menu_open: bool,
    pub menu_rect: [f32; 4],
    pub animating: bool,
}
fn rect(r: Rect) -> [f32; 4] { [r.x, r.y, r.w, r.h] }
impl ChromeTopBar {
    pub fn paint_host(&mut self, painter: &mut (impl Paint + ?Sized), width: f32, viewport_height: f32, theme: &IdeTheme) -> ChromeHostFrame {
        let strip = Rect::new(0.0, 0.0, width, self.height());
        self.refresh_rects(strip, painter.scale_factor());
        self.menu_rect = self.menu_overlay_rect_for(self.menu_btn_rect, strip, DockEdge::Top, Rect::new(0.0, 0.0, width, viewport_height));
        self.paint(painter, strip, DockEdge::Top, theme, &crate::primitives::surface_background::style("chrome.top"));
        let mut regions = Vec::new();
        for (id, label, bounds) in [
            ("topbar-menu", "Menu", self.menu_btn_rect),
            ("toggle-files", "Files", self.panel_btn_rect),
            ("toggle-notes", "Notes", self.notes_btn_rect),
            ("toggle-conversations", "Conversations", self.conversations_btn_rect),
            ("open-finder", "Find", self.search_btn_rect),
            ("open-servers", "Servers", self.server_btn_rect),
            ("toggle-details", "Agent details", self.right_btn_rect),
        ] {
            if bounds.w > 0.0 && bounds.h > 0.0 {
                regions.push(ChromeRegion { id: id.into(), label: label.into(), rect: rect(bounds), menu: false });
            }
        }
        if self.menu_open {
            for (index, item) in MenuItem::visible(self.share_with_phone_enabled).iter().enumerate() {
                regions.push(ChromeRegion { id: format!("menu:{:?}", item.action()), label: item.label().into(), rect: rect(self.menu_item_rect(index)), menu: true });
            }
        }
        ChromeHostFrame { regions, menu_open: self.menu_open, menu_rect: rect(self.menu_rect), animating: self.presence_animation_active() }
    }
}
