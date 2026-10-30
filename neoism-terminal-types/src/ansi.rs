use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    /// Cursor is a block like `▒`.
    #[default]
    #[serde(alias = "block")]
    Block,
    /// Cursor is an underscore like `_`.
    #[serde(alias = "underline")]
    Underline,
    /// Cursor is a vertical bar `⎸`.
    #[serde(alias = "beam")]
    Beam,
    /// Cursor is hidden.
    #[serde(alias = "hidden")]
    Hidden,
}

impl CursorShape {
    pub fn from_char(c: char) -> CursorShape {
        match c {
            '_' => CursorShape::Underline,
            '|' => CursorShape::Beam,
            _ => CursorShape::Block,
        }
    }
}

impl From<CursorShape> for char {
    fn from(value: CursorShape) -> Self {
        match value {
            CursorShape::Underline => '_',
            CursorShape::Beam => '|',
            _ => '▇',
        }
    }
}
