//! Chat uses the configurable sans slot; code and fixed-width UI use the separate mono slot.

use crate::ui::theme::Type;
use rune::TextStyle;

impl Type {
    pub fn chat_sized(self, points: f32) -> TextStyle {
        TextStyle::new(points * self.scale(), 400.0)
    }

    pub fn chat_body(self) -> TextStyle {
        TextStyle::new(self.size, 400.0).lh(self.size * 1.45)
    }

    pub fn chat_heading(self, level: u8) -> TextStyle {
        let scale = match level { 1 => 1.6, 2 => 1.35, 3 => 1.15, _ => 1.0 };
        TextStyle::new(self.size * scale, 700.0).lh(self.size * scale * 1.35)
    }

    pub fn chat_composer(self) -> TextStyle {
        TextStyle::new(self.size * 1.13, 400.0).lh(self.size * 1.6)
    }
}
