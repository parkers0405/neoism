use serde::{Deserialize, Serialize};

#[inline]
fn default_true() -> bool {
    true
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Effects {
    #[serde(default = "bool::default", rename = "custom-mouse-cursor")]
    pub custom_mouse_cursor: bool,
    /// Neovide-style critically-damped cursor trail.
    /// Default ON for the IDE fork — set `effects.trail-cursor = false` to disable.
    #[serde(default = "default_true", rename = "trail-cursor")]
    pub trail_cursor: bool,
    /// Render the cursor with a liquid-glass effect (opt-in).
    #[serde(default, rename = "liquid-glass-cursor")]
    pub liquid_glass_cursor: bool,
    /// Active rail selection material, independent of the keyboard cursor.
    #[serde(default, rename = "liquid-glass-selection")]
    pub liquid_glass_selection: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            custom_mouse_cursor: false,
            trail_cursor: true,
            liquid_glass_cursor: false,
            liquid_glass_selection: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liquid_glass_selection_defaults_off_and_is_independent() {
        assert!(!Effects::default().liquid_glass_selection);
        let cursor: Effects = serde_json::from_str(r#"{"liquid-glass-cursor":true}"#).unwrap();
        assert!(!cursor.liquid_glass_selection);
        for enabled in [true, false] {
            let effect: Effects = serde_json::from_value(serde_json::json!({"liquid-glass-selection": enabled})).unwrap();
            assert_eq!(effect.liquid_glass_selection, enabled);
            assert!(!effect.liquid_glass_cursor);
            assert_eq!(serde_json::to_value(effect).unwrap()["liquid-glass-selection"], enabled);
        }
    }

    #[test]
    fn liquid_glass_cursor_defaults_off_and_round_trips() {
        assert!(!Effects::default().liquid_glass_cursor);
        assert!(!serde_json::from_str::<Effects>("{}").unwrap().liquid_glass_cursor);
        for enabled in [false, true] {
            let effects: Effects = serde_json::from_value(serde_json::json!({"liquid-glass-cursor": enabled})).unwrap();
            assert_eq!(effects.liquid_glass_cursor, enabled);
            assert_eq!(serde_json::to_value(effects).unwrap()["liquid-glass-cursor"], enabled);
        }
    }
}
