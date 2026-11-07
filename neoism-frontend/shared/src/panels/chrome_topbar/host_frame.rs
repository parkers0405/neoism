//! Top-bar and opt-in left activity-rail frames for another native GPU host.
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
    /// Width reserved by the opt-in GPU-host rail in scaled logical pixels.
    pub fn rail_width(&self) -> f32 { 52.0 * self.scale }

    pub(super) fn host_icon_paint_rect(&self, bounds: Rect) -> Rect {
        if !self.painting_rail_host { return bounds; }
        let size = 40.0 * self.scale;
        Rect::new(bounds.x + (bounds.w - size) * 0.5, bounds.y + (bounds.h - size) * 0.5, size, size)
    }

    fn refresh_rail_host_rects(&mut self) {
        let width = self.rail_width();
        let slot = 48.0 * self.scale;
        let mut y = 6.0 * self.scale;
        let mut next = || {
            let bounds = Rect::new(0.0, y, width, slot);
            y += slot;
            bounds
        };
        self.menu_btn_rect = next();
        self.panel_btn_rect = next();
        self.notes_btn_rect = next();
        self.conversations_btn_rect = next();
        self.search_btn_rect = next();
        self.server_btn_rect = next();
        self.right_btn_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        // Neither details control occupies a rail slot.
        self.mobile_agent_panel_btn_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        self.mobile_agent_panel_hit_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        self.peer_rects = (0..self.peers.len().min(CHROME_PEER_CAP)).map(|_| next()).collect();
    }

    /// Paint a left rail with full 52x48 scaled slot regions. `width` and
    /// `viewport_height` describe the viewport used to clamp the right-opening menu.
    /// Icon transforms/animations remain the host's responsibility.
    pub fn paint_rail_host(&mut self, painter: &mut (impl Paint + ?Sized), width: f32, viewport_height: f32, theme: &IdeTheme) -> ChromeHostFrame {
        let strip = Rect::new(0.0, 0.0, self.rail_width(), viewport_height);
        self.refresh_rail_host_rects();
        self.menu_rect = self.menu_overlay_rect_for(self.menu_btn_rect, strip, DockEdge::Left, Rect::new(0.0, 0.0, width, viewport_height));
        self.painting_rail_host = true;
        self.paint(painter, strip, DockEdge::Left, theme, &crate::primitives::surface_background::style("chrome.top"));
        self.painting_rail_host = false;
        self.host_frame()
    }

    pub fn paint_host(&mut self, painter: &mut (impl Paint + ?Sized), width: f32, viewport_height: f32, theme: &IdeTheme) -> ChromeHostFrame {
        let strip = Rect::new(0.0, 0.0, width, self.height());
        self.refresh_rects(strip, painter.scale_factor());
        self.menu_rect = self.menu_overlay_rect_for(self.menu_btn_rect, strip, DockEdge::Top, Rect::new(0.0, 0.0, width, viewport_height));
        self.paint(painter, strip, DockEdge::Top, theme, &crate::primitives::surface_background::style("chrome.top"));
        self.host_frame()
    }

    fn host_frame(&self) -> ChromeHostFrame {
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
        for (index,bounds) in self.peer_rects.iter().enumerate() {
            if let Some(peer)=self.peers.get(index) {
                regions.push(ChromeRegion{id:format!("presence:{}",peer.peer_id),label:peer.display_name.clone(),rect:rect(*bounds),menu:false});
            }
        }
        if self.menu_open {
            for (index, item) in self.menu_items().iter().enumerate() {
                regions.push(ChromeRegion { id: format!("menu:{:?}", item.action()), label: item.label().into(), rect: rect(self.menu_item_rect(index)), menu: true });
            }
        }
        ChromeHostFrame { regions, menu_open: self.menu_open, menu_rect: rect(self.menu_rect), animating: self.presence_animation_active() }
    }
}
