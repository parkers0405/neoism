//! CPU text shaping, glyph rasters, image data, and retained document layout.
//! GPUIX owns GPU rendering and presentation. This crate creates no GPU device or window.
mod atlas;
pub mod font;
pub mod glyph;
pub mod graphics;
pub mod image;
pub mod layout;
mod occlusion;
pub mod primitives;
pub mod text;
pub mod virtual_surface;
pub use graphics::*;
pub use layout::{
    Content, RichTextConfig, SpanStyle, SpanStyleDecoration, TextDimensions,
    UnderlineInfo, UnderlineShape,
};
pub use primitives::*;
pub use swash;
pub use swash::{Attributes, Stretch, Style, Weight};
pub use virtual_surface::*;
#[derive(Debug)]
pub struct FontLoadErrors {
    pub fonts_not_found: Vec<font::fonts::FontSpec>,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}
impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 0.,
    };
    pub const BLACK: Self = Self {
        r: 0.,
        g: 0.,
        b: 0.,
        a: 1.,
    };
    pub const WHITE: Self = Self {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 1.,
    };
    pub const RED: Self = Self {
        r: 1.,
        g: 0.,
        b: 0.,
        a: 1.,
    };
    pub const BLUE: Self = Self {
        r: 0.,
        g: 0.,
        b: 1.,
        a: 1.,
    };
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Colorspace {
    #[default]
    Srgb,
    DisplayP3,
    Rec2020,
}
