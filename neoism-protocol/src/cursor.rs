//! Cursor-overlay wire messages.
//!
//! The desktop frontend drives the trail cursor, custom mouse-sprite,
//! cursorline-overlay, and yank-flash overlays entirely off its local
//! editor + mouse loops. The web frontend has no host-side equivalent
//! to derive these from, so the daemon translates the nvim grid +
//! mouse-route state into the same four push-style messages.
//!
//! Wire payloads carry **logical (grid-cell) coordinates** wherever
//! possible. The daemon has no notion of physical pixel cell metrics
//! (those depend on font size + DPR, both client-owned) so it ships
//! row/col indices and lets the wasm-side bridge multiply by its own
//! [`Chrome::cell_metrics`] when calling
//! [`ChromeBridge::set_trail_cursor`] et al. The web dispatcher in
//! `frontends/web/src/terminal/TerminalPanel.ts` does this final
//! translation before handing the JSON to the bridge.
//!
//! The exception is [`CursorOverlayServerMessage::CustomCursor`]:
//! mouse pointer coordinates are already in physical pixels by the
//! time they reach the daemon (web clients send them through
//! `EditorClientMessage::MouseInput`).

use serde::{Deserialize, Serialize};

/// Logical cursor shape — matches the terminal snapshot cursor shape
/// but kept palette-free so the wire stays free of crate-private enums.
/// Wasm-side mapping is case-insensitive (`"block"`, `"beam"`,
/// `"underline"`, `"hidden"`) — anything unknown falls back to `Block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Beam,
    Underline,
    Hidden,
}

impl CursorShape {
    /// Render to the lower-case string the bridge setter expects.
    pub fn as_str(self) -> &'static str {
        match self {
            CursorShape::Block => "block",
            CursorShape::Beam => "beam",
            CursorShape::Underline => "underline",
            CursorShape::Hidden => "hidden",
        }
    }

    /// Map nvim's `mode_change` short name (`"normal"`, `"insert"`,
    /// `"visual"`, `"replace"`, etc.) to the cursor shape the chrome
    /// should paint. Defaults to `Block` for unknown modes.
    pub fn from_mode(mode: &str) -> Self {
        match mode {
            "insert" => CursorShape::Beam,
            "replace" => CursorShape::Underline,
            // Command-line and prompt modes hide the editor cursor
            // because focus has shifted to the cmdline UI.
            "cmdline" | "cmdline_normal" | "prompt" => CursorShape::Hidden,
            _ => CursorShape::Block,
        }
    }
}

/// One yank-flash region (inclusive screen-row range, grid cell rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct YankFlashRegion {
    /// 0-based screen row relative to the editor pane top.
    pub row_top: u32,
    /// 0-based screen row relative to the editor pane top. Inclusive.
    pub row_bot: u32,
    /// Optional 0-based inclusive start column. Missing means full row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_left: Option<u32>,
    /// Optional 0-based inclusive end column. Missing means full row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_right: Option<u32>,
}

/// Client-originated cursor-overlay updates. This is intentionally
/// narrow: nvim still owns editor cursor/yank producers, while the
/// browser/desktop pointer loop can feed the daemon physical mouse
/// coordinates for the custom cursor sprite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CursorOverlayClientMessage {
    /// Custom mouse-sprite position + visibility, in physical pixels.
    CustomCursor {
        x: f32,
        y: f32,
        #[serde(default = "default_true")]
        visible: bool,
    },
}

/// Server-originated cursor-overlay pushes. Externally-tagged like the
/// rest of the protocol crate. Each variant mirrors a single
/// `ChromeBridge::set_*` setter on the wasm side; the dispatcher in
/// `frontends/web/src/terminal/TerminalPanel.ts` does the cell→pixel
/// translation before invoking the bridge setter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CursorOverlayServerMessage {
    /// Trail-cursor destination + shape, in grid cells.
    ///
    /// Maps to `ChromeBridge::set_trail_cursor`. The web dispatcher
    /// multiplies `(col, row)` by `cell_metrics()` to get physical
    /// pixels before forwarding.
    TrailCursor {
        /// 0-based cell column.
        col: u32,
        /// 0-based cell row.
        row: u32,
        /// Cursor shape. `None` defaults to `Block` on the bridge.
        shape: Option<CursorShape>,
        /// When true, advances the trail's destination without ranking
        /// a new logical-cursor jump — used during scroll-spring follow.
        #[serde(default)]
        no_jump: bool,
        /// When true, clears the spring + last-destination cache before
        /// applying the new destination. Use this on pane switch.
        #[serde(default)]
        reset: bool,
        /// When true, teleports the trail to the destination (no glide).
        #[serde(default)]
        snap: bool,
    },
    /// Custom mouse-sprite position + visibility, in physical pixels.
    ///
    /// The pointer position is naturally in pixels (web clients send
    /// it via `EditorClientMessage::MouseInput` after their own
    /// translation), so the daemon forwards as-is. Maps to
    /// `ChromeBridge::set_custom_cursor`.
    ///
    /// `x` / `y` are `Option<f32>` so the daemon can emit
    /// visibility-only updates (e.g. nvim `mouse_off` / `mouse_on`
    /// from a `:terminal` buffer) without clobbering the last
    /// pointer position cached on the chrome. When omitted, the
    /// bridge preserves the last-known coordinates and only toggles
    /// the `visible` flag.
    CustomCursor {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f32>,
        /// `false` hides the sprite without forgetting its last-known
        /// position (call on pointer-leave). Defaults to `true`.
        #[serde(default = "default_true")]
        visible: bool,
    },
    /// Cursorline-overlay target for one editor pane, in grid cell row.
    ///
    /// Maps to `ChromeBridge::set_cursorline_overlay`. The web
    /// dispatcher multiplies `target_row` by `cell_h` to get
    /// `target_y` before forwarding.
    CursorlineOverlay {
        /// 0-based pane / rich-text id.
        rich_text_id: u32,
        /// 0-based row of the highlighted line (cell coordinates).
        target_row: u32,
        /// When true, pins the highlight without glide animation.
        #[serde(default)]
        snap: bool,
        /// When true, drops the cached state for this pane id (call
        /// when the pane is closed/destroyed).
        #[serde(default)]
        forget: bool,
    },
    /// Yank-flash regions (one or more inclusive screen-row spans).
    /// Cell-row coordinates. Maps to `ChromeBridge::set_yank_flash`.
    YankFlash { regions: Vec<YankFlashRegion> },
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: &CursorOverlayServerMessage) {
        let json = serde_json::to_string(msg).expect("encode");
        let decoded: CursorOverlayServerMessage =
            serde_json::from_str(&json).expect("decode");
        assert_eq!(&decoded, msg);
    }

    fn roundtrip_client(msg: &CursorOverlayClientMessage) {
        let json = serde_json::to_string(msg).expect("encode");
        let decoded: CursorOverlayClientMessage =
            serde_json::from_str(&json).expect("decode");
        assert_eq!(&decoded, msg);
    }

    #[test]
    fn trail_cursor_roundtrip() {
        roundtrip(&CursorOverlayServerMessage::TrailCursor {
            col: 12,
            row: 4,
            shape: Some(CursorShape::Beam),
            no_jump: true,
            reset: false,
            snap: false,
        });
    }

    #[test]
    fn custom_cursor_roundtrip() {
        roundtrip(&CursorOverlayServerMessage::CustomCursor {
            x: Some(100.0),
            y: Some(200.0),
            visible: false,
        });
        // Visibility-only frame (daemon emits when nvim toggles
        // `mouse_on` / `mouse_off`): omits x/y so the bridge keeps
        // the last cached position.
        roundtrip(&CursorOverlayServerMessage::CustomCursor {
            x: None,
            y: None,
            visible: false,
        });
    }

    #[test]
    fn custom_cursor_client_roundtrip() {
        roundtrip_client(&CursorOverlayClientMessage::CustomCursor {
            x: 12.5,
            y: 42.0,
            visible: true,
        });
    }

    #[test]
    fn custom_cursor_client_defaults_visible() {
        let decoded: CursorOverlayClientMessage =
            serde_json::from_str(r#"{"CustomCursor":{"x":1.0,"y":2.0}}"#).unwrap();
        assert_eq!(
            decoded,
            CursorOverlayClientMessage::CustomCursor {
                x: 1.0,
                y: 2.0,
                visible: true,
            }
        );
    }

    #[test]
    fn cursorline_overlay_roundtrip() {
        roundtrip(&CursorOverlayServerMessage::CursorlineOverlay {
            rich_text_id: 3,
            target_row: 6,
            snap: true,
            forget: false,
        });
        roundtrip(&CursorOverlayServerMessage::CursorlineOverlay {
            rich_text_id: 3,
            target_row: 0,
            snap: false,
            forget: true,
        });
    }

    #[test]
    fn yank_flash_roundtrip() {
        roundtrip(&CursorOverlayServerMessage::YankFlash {
            regions: vec![
                YankFlashRegion {
                    row_top: 4,
                    row_bot: 4,
                    col_left: Some(2),
                    col_right: Some(8),
                },
                YankFlashRegion {
                    row_top: 7,
                    row_bot: 9,
                    col_left: None,
                    col_right: None,
                },
            ],
        });
    }

    #[test]
    fn cursor_shape_str() {
        assert_eq!(CursorShape::Block.as_str(), "block");
        assert_eq!(CursorShape::Beam.as_str(), "beam");
        assert_eq!(CursorShape::Underline.as_str(), "underline");
        assert_eq!(CursorShape::Hidden.as_str(), "hidden");
    }

    #[test]
    fn cursor_shape_from_mode() {
        assert_eq!(CursorShape::from_mode("insert"), CursorShape::Beam);
        assert_eq!(CursorShape::from_mode("replace"), CursorShape::Underline);
        assert_eq!(CursorShape::from_mode("cmdline"), CursorShape::Hidden);
        assert_eq!(CursorShape::from_mode("normal"), CursorShape::Block);
        assert_eq!(CursorShape::from_mode("visual"), CursorShape::Block);
        assert_eq!(CursorShape::from_mode(""), CursorShape::Block);
    }
}
