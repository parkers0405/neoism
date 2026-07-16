#[allow(unused_macros)]
macro_rules! font {
    ($font:literal) => {
        include_bytes!($font) as &[u8]
    };
}

pub const DEFAULT_FONT_FAMILY: &str = "cascadiacode";

// Fonts:
// CascadiaCode-Bold.ttf
// CascadiaCode-BoldItalic.ttf
// CascadiaCode-ExtraLight.ttf
// CascadiaCode-ExtraLightItalic.ttf
// CascadiaCode-Italic.ttf
// CascadiaCode-Light.ttf
// CascadiaCode-LightItalic.ttf
// CascadiaCode-Regular.ttf
// CascadiaCode-SemiBold.ttf
// CascadiaCode-SemiBoldItalic.ttf
// CascadiaCode-SemiLight.ttf
// CascadiaCode-SemiLightItalic.ttf

pub const FONT_CASCADIAMONO_BOLD: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-Bold.otf");

pub const FONT_CASCADIAMONO_BOLD_ITALIC: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-BoldItalic.otf");

pub const FONT_CASCADIAMONO_EXTRA_LIGHT: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-ExtraLight.otf");

pub const FONT_CASCADIAMONO_EXTRA_LIGHT_ITALIC: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-ExtraLightItalic.otf");

pub const FONT_CASCADIAMONO_ITALIC: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-Italic.otf");

pub const FONT_CASCADIAMONO_LIGHT: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-Light.otf");

pub const FONT_CASCADIAMONO_LIGHT_ITALIC: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-LightItalic.otf");

pub const FONT_CASCADIAMONO_NF_REGULAR: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCodeNF-Regular.otf");

pub const FONT_CASCADIAMONO_SEMI_BOLD: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-SemiBold.otf");

pub const FONT_CASCADIAMONO_SEMI_BOLD_ITALIC: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-SemiBoldItalic.otf");

pub const FONT_CASCADIAMONO_SEMI_LIGHT: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-SemiLight.otf");

pub const FONT_CASCADIAMONO_SEMI_LIGHT_ITALIC: &[u8] =
    font!("./resources/CascadiaCode/CascadiaCode-SemiLightItalic.otf");

pub const FONT_SYMBOLS_NERD_FONT_MONO: &[u8] =
    font!("./resources/SymbolsNerdFontMono/SymbolsNerdFontMono-Regular.ttf");

/// "Press Start 2P" (Google Fonts, SIL OFL 1.1 — see the OFL.txt beside the
/// TTF) — the arcade pixel face feature UIs opt into by family name via
/// `font_id_for_family("Press Start 2P")` (e.g. the agent side-panel
/// headings). Registered with the other bundled fonts at library load.
pub const FONT_PRESS_START_2P: &[u8] =
    font!("./resources/PressStart2P/PressStart2P-Regular.ttf");
