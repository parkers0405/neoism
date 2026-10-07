//! Data-only native background effects. No script runs in a paint callback.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackgroundEffect {
    Stars(EffectOptions),
    Scanlines(EffectOptions),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawOptions")]
pub struct EffectOptions {
    pub color: [f32; 4],
    pub seed: u32,
    pub density: f32,
    pub speed: f32,
    pub opacity: f32,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawOptions {
    color: [f32; 4],
    seed: u32,
    density: f32,
    speed: f32,
    opacity: f32,
}

impl Default for RawOptions {
    fn default() -> Self {
        Self {
            color: [1.0; 4],
            seed: 0,
            density: 1.0,
            speed: 1.0,
            opacity: 1.0,
        }
    }
}

impl Default for EffectOptions {
    fn default() -> Self {
        RawOptions::default().try_into().unwrap()
    }
}

impl TryFrom<RawOptions> for EffectOptions {
    type Error = String;

    fn try_from(v: RawOptions) -> Result<Self, String> {
        let result = Self {
            color: v.color,
            seed: v.seed,
            density: v.density,
            speed: v.speed,
            opacity: v.opacity,
        };
        result.validate()?;
        Ok(result)
    }
}

impl EffectOptions {
    pub fn validate(&self) -> Result<(), String> {
        fn bounded(v: f32, max: f32) -> bool {
            v.is_finite() && v >= 0.0 && v <= max
        }
        if !self.color.iter().all(|v| bounded(*v, 1.0))
            || !bounded(self.density, 4.0)
            || !bounded(self.speed, 4.0)
            || !bounded(self.opacity, 1.0)
        {
            return Err("background effect: color/opacity must be finite 0..1; density/speed finite 0..4".into());
        }
        Ok(())
    }
}

impl BackgroundEffect {
    pub fn options(&self) -> &EffectOptions {
        match self {
            Self::Stars(v) | Self::Scanlines(v) => v,
        }
    }
}

pub(crate) fn deserialize_effects<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Vec<BackgroundEffect>>, D::Error> {
    let effects = Option::<Vec<BackgroundEffect>>::deserialize(d)?;
    if effects.as_ref().is_some_and(|v| v.len() > 4) {
        return Err(serde::de::Error::custom(
            "at most four background effects per style",
        ));
    }
    Ok(effects)
}

/// Only these native surfaces currently consume background_effects. This is
/// deliberately separate from the chrome.actions placement registry.
pub const BACKGROUND_EFFECT_SURFACES: &[&str] = &[
    "composer.agent",
    "chrome.top",
    "status",
    "editor.code",
    "file-tree",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_overlay_inherits_and_empty_effects_clear() {
        use crate::{StylePatch, StyleSheet};
        let mut pack = StyleSheet::default();
        pack.insert(
            "composer",
            StylePatch {
                background: Some("#000000".into()),
                background_effects: Some(vec![BackgroundEffect::Stars(
                    Default::default(),
                )]),
                ..Default::default()
            },
        );
        let mut plugin = StyleSheet::default();
        plugin.insert(
            "composer",
            StylePatch {
                foreground: Some("#ffffff".into()),
                ..Default::default()
            },
        );
        pack.overlay(&plugin);
        let mut user = StyleSheet::default();
        user.insert(
            "composer.agent",
            StylePatch {
                background_effects: Some(vec![]),
                ..Default::default()
            },
        );
        pack.overlay(&user);
        let mut personal_parent = StyleSheet::default();
        personal_parent.insert(
            "composer",
            StylePatch {
                foreground: Some("#eeeeee".into()),
                ..Default::default()
            },
        );
        pack.overlay_layer(&personal_parent);
        let resolved = pack.resolve("composer.agent.input");
        assert_eq!(resolved.background.as_deref(), Some("#000000"));
        assert_eq!(resolved.foreground.as_deref(), Some("#eeeeee"));
        assert_eq!(resolved.background_effects, Some(vec![]));
        let invalid = serde_json::json!({"background_effects": vec![serde_json::json!({"kind":"stars"}); 5]});
        assert!(serde_json::from_value::<StylePatch>(invalid).is_err());
    }

    #[test]
    fn strict_effect_schema() {
        for json in [
            r#"{"kind":"unknown"}"#,
            r#"{"kind":"stars","speed":5}"#,
            r#"{"kind":"stars","color":[1,1,1,2]}"#,
            r#"{"kind":"stars","extra":1}"#,
            r#"{"kind":"scanlines","seed":-1}"#,
        ] {
            assert!(
                serde_json::from_str::<BackgroundEffect>(json).is_err(),
                "{json}"
            );
        }
        assert!(serde_json::from_str::<BackgroundEffect>(r#"{"kind":"stars"}"#).is_ok());
        assert!(EffectOptions {
            speed: f32::NAN,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
