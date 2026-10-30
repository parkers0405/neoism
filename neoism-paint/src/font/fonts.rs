use crate::font::DEFAULT_FONT_FAMILY;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Clone)]
pub enum FontStyle {
    #[default]
    #[serde(alias = "normal")]
    Normal,
    #[serde(alias = "italic")]
    Italic,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Clone)]
pub enum FontWidth {
    UltraCondensed,
    ExtraCondensed,
    Condensed,
    SemiCondensed,
    #[default]
    Normal,
    SemiExpanded,
    Expanded,
    ExtraExpanded,
    UltraExpanded,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct FontSpec {
    #[serde(default = "default_font_family")]
    pub family: String,
    #[serde(default = "Option::default")]
    pub weight: Option<u16>,
    #[serde(default = "FontStyle::default")]
    pub style: FontStyle,
    #[serde(default = "Option::default")]
    pub width: Option<FontWidth>,
}

impl Default for FontSpec {
    fn default() -> Self {
        Self {
            family: default_font_family(),
            weight: None,
            style: FontStyle::Normal,
            width: None,
        }
    }
}

impl FontSpec {
    #[inline]
    pub fn is_default_family(&self) -> bool {
        let current = self.family.replace(' ', "").trim().to_lowercase();
        current == default_font_family()
    }
}

#[inline]
pub fn default_font_size() -> f32 {
    14.
}

#[inline]
pub fn default_bool_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolMap {
    pub start: String,
    pub end: String,
    #[serde(rename = "font-family")]
    pub font_family: String,
}

fn default_font_family() -> String {
    DEFAULT_FONT_FAMILY.to_string()
}

pub fn default_font_regular() -> FontSpec {
    FontSpec {
        family: default_font_family(),
        weight: Some(400),
        style: FontStyle::Normal,
        width: None,
    }
}

pub fn default_font_bold() -> FontSpec {
    FontSpec {
        family: default_font_family(),
        weight: Some(800),
        style: FontStyle::Normal,
        width: None,
    }
}

pub fn default_font_italic() -> FontSpec {
    FontSpec {
        family: default_font_family(),
        weight: Some(300),
        style: FontStyle::Italic,
        width: None,
    }
}

pub fn default_font_bold_italic() -> FontSpec {
    FontSpec {
        family: default_font_family(),
        weight: Some(800),
        style: FontStyle::Italic,
        width: None,
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct FontConfig {
    #[serde(default = "default_font_size")]
    pub size: f32,
    #[serde(default = "default_bool_true")]
    pub hinting: bool,
    #[serde(default = "Option::default")]
    pub features: Option<Vec<String>>,
    #[serde(default = "Option::default")]
    pub family: Option<String>,
    /// Global base weight (100–900). When set, overrides the regular +
    /// italic face weight so body text can be made thicker/thinner
    /// without editing each variant. Bold keeps its own weight.
    #[serde(default = "Option::default")]
    pub weight: Option<u16>,
    #[serde(default = "default_font_regular")]
    pub regular: FontSpec,
    #[serde(default = "default_font_bold")]
    pub bold: FontSpec,
    #[serde(default = "default_font_bold_italic", rename = "bold-italic")]
    pub bold_italic: FontSpec,
    #[serde(default = "default_font_italic")]
    pub italic: FontSpec,
    #[serde(default = "default_bool_true", rename = "use-drawable-chars")]
    pub use_drawable_chars: bool,
    #[serde(default = "Option::default", rename = "symbol-map")]
    pub symbol_map: Option<Vec<SymbolMap>>,
    #[serde(default = "bool::default", rename = "disable-warnings-not-found")]
    pub disable_warnings_not_found: bool,
    #[serde(default = "Option::default", rename = "additional-dirs")]
    pub additional_dirs: Option<Vec<String>>,
    /// Extra system families to load without changing the primary
    /// terminal/code cascade. Markdown look (`appearance.look.markdown.font-family`)
    /// uses this so a proportional face can sit beside a mono primary font.
    #[serde(default = "Option::default", rename = "extra-families")]
    pub extra_families: Option<Vec<String>>,
}

pub fn parse_unicode(input: &str) -> Option<char> {
    if let Ok(unicode) = u32::from_str_radix(input, 16) {
        if let Some(result) = char::from_u32(unicode) {
            return Some(result);
        }
    }

    None
}

impl Default for FontConfig {
    fn default() -> FontConfig {
        FontConfig {
            features: None,
            hinting: true,
            size: default_font_size(),
            family: None,
            weight: None,
            regular: default_font_regular(),
            bold: default_font_bold(),
            bold_italic: default_font_bold_italic(),
            italic: default_font_italic(),
            use_drawable_chars: true,
            symbol_map: None,
            disable_warnings_not_found: false,
            additional_dirs: None,
            extra_families: None,
        }
    }
}
