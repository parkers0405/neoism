//! The `.neodraw` document model.
//!
//! A [`Scene`] is a flat, z-ordered list of [`Shape`]s. It is the
//! single source of truth a `.neodraw` file (de)serializes to/from,
//! and the same model the markdown ```draw embed renders read-only.
//!
//! The on-disk format is JSON via serde — human-readable and
//! git-diff-friendly. Geometry lives in *world* coordinates (logical
//! pixels, top-left origin); the [`Camera`](super::Camera) maps world
//! space onto the screen at draw time.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Current on-disk schema version. Bump when the layout changes in a
/// way old readers can't tolerate; readers should migrate forward.
pub const SCENE_VERSION: u32 = 1;

/// A point/offset in world space (logical pixels, top-left origin).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// An 8-bit RGBA colour. Serializes as a `#rrggbb` / `#rrggbbaa` hex
/// string so `.neodraw` files read cleanly (and look like Excalidraw).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Linear 0..=1 `[r, g, b, a]` for sugarloaf draw calls.
    pub fn rgba_f32(&self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }

    /// Same as [`rgba_f32`](Self::rgba_f32) but with `a` multiplied by
    /// `opacity` (the per-shape style alpha).
    pub fn rgba_f32_with_opacity(&self, opacity: f32) -> [f32; 4] {
        let mut c = self.rgba_f32();
        c[3] *= opacity.clamp(0.0, 1.0);
        c
    }

    fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    fn from_hex(s: &str) -> Result<Self, String> {
        let h = s.strip_prefix('#').unwrap_or(s);
        let byte = |i: usize| -> Result<u8, String> {
            u8::from_str_radix(&h[i..i + 2], 16)
                .map_err(|_| format!("invalid hex colour: {s:?}"))
        };
        match h.len() {
            6 => Ok(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            3 => {
                // #rgb shorthand → expand each nibble.
                let nib = |i: usize| -> Result<u8, String> {
                    let v = u8::from_str_radix(&h[i..i + 1], 16)
                        .map_err(|_| format!("invalid hex colour: {s:?}"))?;
                    Ok(v << 4 | v)
                };
                Ok(Self::rgb(nib(0)?, nib(1)?, nib(2)?))
            }
            _ => Err(format!("invalid hex colour: {s:?}")),
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct HexVisitor;
        impl Visitor<'_> for HexVisitor {
            type Value = Color;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a hex colour string like \"#rrggbb\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Color, E> {
                Color::from_hex(v).map_err(E::custom)
            }
        }
        d.deserialize_str(HexVisitor)
    }
}

/// Stable identifier for a shape, unique within a [`Scene`]. Used by
/// selection, undo/redo, and hit-testing so edits survive reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShapeId(pub u64);

/// How an arrow's endpoint is decorated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowHead {
    None,
    #[default]
    Triangle,
}

/// Visual style applied to a shape's stroke and fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Color,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Color>,
    pub width: f32,
    /// Hand-drawn wobble amount (0 = clean). Drives the rough pass.
    #[serde(default)]
    pub roughness: f32,
    /// Per-shape PRNG seed so the rough jitter is *stable* across
    /// frames (otherwise hand-drawn strokes shimmer on every redraw).
    #[serde(default)]
    pub seed: u32,
    #[serde(default = "one")]
    pub opacity: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for Style {
    fn default() -> Self {
        Self {
            stroke: Color::WHITE,
            fill: None,
            width: 2.0,
            roughness: 1.0,
            seed: 0,
            opacity: 1.0,
        }
    }
}

/// The geometry of a shape. Tagged in JSON by a `"type"` field, e.g.
/// `{ "type": "rect", "x": 0, "y": 0, "w": 100, "h": 40 }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(default)]
        corner: f32,
    },
    Ellipse {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    /// A polyline. Two points = a straight segment.
    Line {
        points: Vec<Vec2>,
    },
    /// A closed polygon. The renderer closes the path automatically.
    Polygon {
        points: Vec<Vec2>,
    },
    Arrow {
        points: Vec<Vec2>,
        #[serde(default)]
        head: ArrowHead,
    },
    /// A captured pen stroke.
    Freehand {
        points: Vec<Vec2>,
    },
    Text {
        x: f32,
        y: f32,
        content: String,
        size: f32,
        /// None keeps legacy auto-sized labels; resizing sets a world-space wrap width.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
    },
}

/// A single drawable element: geometry plus style plus a stable id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: ShapeId,
    #[serde(flatten)]
    pub kind: ShapeKind,
    #[serde(default)]
    pub style: Style,
}

/// A `.neodraw` document: a z-ordered list of shapes (vec order is
/// back-to-front paint order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub version: u32,
    #[serde(default)]
    pub shapes: Vec<Shape>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            version: SCENE_VERSION,
            shapes: Vec::new(),
        }
    }
}

impl Scene {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse a `.neodraw` document from JSON.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    /// Serialize to pretty JSON (the on-disk form).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\":{:?}}}", e.to_string()))
    }

    /// An id one past the current maximum, for allocating new shapes.
    pub fn next_id(&self) -> ShapeId {
        let max = self.shapes.iter().map(|s| s.id.0).max().unwrap_or(0);
        ShapeId(max + 1)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_text_loads_and_resized_width_round_trips() {
        let source = r#"{"version":1,"shapes":[{"id":1,"type":"text","x":10,"y":20,"content":"old drawing","size":28}]}"#;
        let mut scene = super::Scene::from_json(source).unwrap();
        match &mut scene.shapes[0].kind {
            super::ShapeKind::Text { width, .. } => {
                assert_eq!(*width, None);
                *width = Some(160.0);
            }
            _ => panic!(),
        }
        let loaded = super::Scene::from_json(&scene.to_json()).unwrap();
        assert_eq!(loaded, scene);
    }

    use super::*;

    #[test]
    fn color_hex_round_trips() {
        assert_eq!(Color::rgb(255, 0, 0).to_hex(), "#ff0000");
        assert_eq!(Color::rgba(0, 16, 255, 128).to_hex(), "#0010ff80");
        assert_eq!(Color::from_hex("#fff").unwrap(), Color::rgb(255, 255, 255));
        assert_eq!(Color::from_hex("00ff00").unwrap(), Color::rgb(0, 255, 0));
        assert!(Color::from_hex("#xyz").is_err());
    }

    #[test]
    fn scene_json_round_trips() {
        let scene = Scene {
            version: SCENE_VERSION,
            shapes: vec![
                Shape {
                    id: ShapeId(1),
                    kind: ShapeKind::Rect {
                        x: 10.0,
                        y: 20.0,
                        w: 100.0,
                        h: 40.0,
                        corner: 6.0,
                    },
                    style: Style {
                        stroke: Color::WHITE,
                        fill: Some(Color::rgba(255, 255, 255, 20)),
                        ..Style::default()
                    },
                },
                Shape {
                    id: ShapeId(2),
                    kind: ShapeKind::Arrow {
                        points: vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 80.0)],
                        head: ArrowHead::Triangle,
                    },
                    style: Style::default(),
                },
                Shape {
                    id: ShapeId(3),
                    kind: ShapeKind::Text {
                        width: None,
                        x: 5.0,
                        y: 5.0,
                        content: "Ambiguity".into(),
                        size: 28.0,
                    },
                    style: Style::default(),
                },
            ],
        };

        let json = scene.to_json();
        let parsed = Scene::from_json(&json).expect("round-trip parse");
        assert_eq!(parsed, scene);
        assert_eq!(parsed.next_id(), ShapeId(4));
    }

    #[test]
    fn tagged_kind_reads_cleanly() {
        let json = r##"{
            "version": 1,
            "shapes": [
                { "id": 7, "type": "ellipse", "x": 0, "y": 0, "w": 30, "h": 30,
                  "style": { "stroke": "#41b8ff", "width": 4 } }
            ]
        }"##;
        let scene = Scene::from_json(json).expect("parse");
        assert_eq!(scene.shapes.len(), 1);
        let shape = &scene.shapes[0];
        assert_eq!(shape.id, ShapeId(7));
        assert!(matches!(shape.kind, ShapeKind::Ellipse { w, .. } if w == 30.0));
        assert_eq!(shape.style.stroke, Color::rgb(0x41, 0xb8, 0xff));
        // Unspecified style fields fall back to defaults.
        assert_eq!(shape.style.opacity, 1.0);
    }

    #[test]
    fn empty_and_missing_shapes_default() {
        let scene = Scene::from_json(r#"{ "version": 1 }"#).expect("parse");
        assert!(scene.shapes.is_empty());
    }
}
