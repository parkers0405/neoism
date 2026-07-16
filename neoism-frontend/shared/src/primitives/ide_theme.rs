//! Per-theme palette shared across chrome panels and the syntax
//! highlighter. Mirrors `nvim_runtime/lua/rio/theme.lua` so chrome and
//! editor paint with the same colors.
//!
//! Lifted from `frontends/neoism/src/chrome/primitives/theme.rs` so
//! native and web reach the same source of truth.

use neoism_terminal_core::colors::ColorRgb;
use std::sync::RwLock;
use sugarloaf::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdeThemeName {
    PastelDark,
    NvChadOne,
    TokyoNight,
    CatppuccinMocha,
    /// A theme loaded at runtime (from `ide-themes/*.toml` or a Mash Up
    /// Pack). The name is interned once per distinct string so the enum
    /// stays `Copy` and everything downstream of theme selection keeps
    /// passing `IdeTheme` by value.
    Custom(&'static str),
}

impl IdeThemeName {
    /// Every selectable IDE theme, in display order. Single source of
    /// truth for theme pickers (the Cmd+P themes mode and the hamburger
    /// → Themes action) so web and desktop offer the same list without
    /// re-hardcoding it per host.
    pub const ALL: [IdeThemeName; 4] = [
        IdeThemeName::PastelDark,
        IdeThemeName::NvChadOne,
        IdeThemeName::TokyoNight,
        IdeThemeName::CatppuccinMocha,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            IdeThemeName::PastelDark => "pastel_dark",
            IdeThemeName::NvChadOne => "nvchad_one",
            IdeThemeName::TokyoNight => "tokyo_night",
            IdeThemeName::CatppuccinMocha => "catppuccin_mocha",
            IdeThemeName::Custom(name) => name,
        }
    }

    pub fn from_str(name: &str) -> Self {
        match name {
            "nvchad_one" => IdeThemeName::NvChadOne,
            "tokyo_night" => IdeThemeName::TokyoNight,
            "catppuccin_mocha" => IdeThemeName::CatppuccinMocha,
            _ => IdeThemeName::PastelDark,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IdeTheme {
    pub name: IdeThemeName,
    pub bg: u32,
    pub fg: u32,
    pub surface: u32,
    pub hover: u32,
    pub border: u32,
    pub muted: u32,
    pub dim: u32,
    pub accent: u32,
    pub folder: u32,
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
    pub blue: u32,
    pub magenta: u32,
    pub cyan: u32,
    pub white: u32,
    pub black: u32,
    pub syn_comment: u32,
    pub syn_string: u32,
    pub syn_number: u32,
    pub syn_keyword: u32,
    #[allow(dead_code)]
    pub syn_statement: u32,
    pub syn_func: u32,
    pub syn_type: u32,
    #[allow(dead_code)]
    pub syn_property: u32,
    #[allow(dead_code)]
    pub syn_constructor: u32,
    #[allow(dead_code)]
    pub syn_special: u32,
}

impl Default for IdeTheme {
    fn default() -> Self {
        Self::pastel_dark()
    }
}

/// Runtime-registered themes (Mash Up Packs / `ide-themes/*.toml`).
/// Const-init like `ACTIVE_IDE_THEME`; linear scan is fine at this
/// cardinality. Wasm keeps this empty today, so `by_name` degrades to
/// the builtin set there.
struct CustomThemeEntry {
    name: &'static str,
    description: String,
    theme: IdeTheme,
}

static CUSTOM_IDE_THEMES: RwLock<Vec<CustomThemeEntry>> = RwLock::new(Vec::new());

/// Look up a runtime-registered theme by name.
pub fn custom_ide_theme(name: &str) -> Option<IdeTheme> {
    CUSTOM_IDE_THEMES
        .read()
        .ok()?
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.theme)
}

/// Replace the whole custom-theme set (called after a disk scan, so
/// deleted theme files disappear from pickers). Names are interned once
/// per distinct string — re-registering an existing name reuses its
/// `&'static str` so config hot-reload does not leak per reload.
pub fn replace_custom_ide_themes(themes: Vec<(String, String, IdeTheme)>) {
    let Ok(mut registry) = CUSTOM_IDE_THEMES.write() else {
        return;
    };
    let mut next = Vec::with_capacity(themes.len());
    for (name, description, mut theme) in themes {
        // Builtin names always win: silently shadowing `tokyo_night`
        // with a file would make the picker ambiguous.
        if IdeThemeName::ALL.iter().any(|b| b.as_str() == name) {
            continue;
        }
        let interned: &'static str = registry
            .iter()
            .chain(next.iter())
            .find(|entry: &&CustomThemeEntry| entry.name == name)
            .map(|entry| entry.name)
            .unwrap_or_else(|| Box::leak(name.clone().into_boxed_str()));
        theme.name = IdeThemeName::Custom(interned);
        next.push(CustomThemeEntry {
            name: interned,
            description,
            theme,
        });
    }
    *registry = next;
}

/// `(name, description)` for every runtime-registered theme, in
/// registration (scan) order — feeds the theme pickers after the
/// builtin four.
pub fn custom_ide_theme_entries() -> Vec<(String, String)> {
    CUSTOM_IDE_THEMES
        .read()
        .map(|registry| {
            registry
                .iter()
                .map(|entry| (entry.name.to_string(), entry.description.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Builtins first, then customs — the single source of truth for every
/// theme list (palette themes mode, hamburger → Themes, web).
pub fn all_ide_theme_names() -> Vec<String> {
    let mut names: Vec<String> = IdeThemeName::ALL
        .iter()
        .map(|name| name.as_str().to_string())
        .collect();
    names.extend(custom_ide_theme_entries().into_iter().map(|(name, _)| name));
    names
}

/// Parse `#RGB` / `#RRGGBB` (leading `#` optional) into the packed
/// `0xRRGGBB` form `IdeTheme` stores.
pub fn parse_theme_hex(value: &str) -> Option<u32> {
    let value = value.trim().trim_start_matches('#');
    match value.len() {
        3 => {
            let packed = u32::from_str_radix(value, 16).ok()?;
            let r = (packed >> 8) & 0xf;
            let g = (packed >> 4) & 0xf;
            let b = packed & 0xf;
            Some((r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11))
        }
        6 => u32::from_str_radix(value, 16).ok(),
        _ => None,
    }
}

impl IdeTheme {
    pub fn by_name(name: &str) -> Self {
        match name {
            "pastel_dark" => Self::pastel_dark(),
            "nvchad_one" => Self::nvchad_one(),
            "tokyo_night" => Self::tokyo_night(),
            "catppuccin_mocha" => Self::catppuccin_mocha(),
            other => custom_ide_theme(other).unwrap_or_else(Self::pastel_dark),
        }
    }

    /// Build a theme from a base plus `key = "#hex"` overrides — the
    /// deserialized body of an `ide-themes/*.toml` / pack `theme.toml`.
    /// Unknown keys and unparseable colors are reported, not fatal, so
    /// a typo'd theme file still loads with the rest of its palette.
    pub fn from_overrides(
        extends: &str,
        overrides: &[(String, String)],
    ) -> (Self, Vec<String>) {
        let mut theme = Self::by_name(extends);
        let mut warnings = Vec::new();
        for (key, value) in overrides {
            let Some(color) = parse_theme_hex(value) else {
                warnings.push(format!("bad color for `{key}`: {value:?}"));
                continue;
            };
            let slot = match key.as_str() {
                "bg" => &mut theme.bg,
                "fg" => &mut theme.fg,
                "surface" => &mut theme.surface,
                "hover" => &mut theme.hover,
                "border" => &mut theme.border,
                "muted" => &mut theme.muted,
                "dim" => &mut theme.dim,
                "accent" => &mut theme.accent,
                "folder" => &mut theme.folder,
                "red" => &mut theme.red,
                "green" => &mut theme.green,
                "yellow" => &mut theme.yellow,
                "blue" => &mut theme.blue,
                "magenta" => &mut theme.magenta,
                "cyan" => &mut theme.cyan,
                "white" => &mut theme.white,
                "black" => &mut theme.black,
                "comment" | "syn_comment" => &mut theme.syn_comment,
                "string" | "syn_string" => &mut theme.syn_string,
                "number" | "syn_number" => &mut theme.syn_number,
                "keyword" | "syn_keyword" => &mut theme.syn_keyword,
                "statement" | "syn_statement" => &mut theme.syn_statement,
                "func" | "syn_func" => &mut theme.syn_func,
                "type" | "syn_type" => &mut theme.syn_type,
                "property" | "syn_property" => &mut theme.syn_property,
                "constructor" | "syn_constructor" => &mut theme.syn_constructor,
                "special" | "syn_special" => &mut theme.syn_special,
                other => {
                    warnings.push(format!("unknown theme key `{other}`"));
                    continue;
                }
            };
            *slot = color;
        }
        (theme, warnings)
    }

    /// The palette table the managed nvim colorscheme consumes
    /// (`rio.theme` in the lua runtime). Keys mirror the builtin lua
    /// palettes; `line`/`surface` map to the closest chrome shades.
    pub fn lua_palette_pairs(&self) -> Vec<(&'static str, String)> {
        let hex = |c: u32| format!("#{c:06x}");
        vec![
            ("bg", hex(self.bg)),
            ("fg", hex(self.fg)),
            ("line", hex(self.surface)),
            ("surface", hex(self.hover)),
            ("muted", hex(self.muted)),
            ("comment", hex(self.syn_comment)),
            ("string", hex(self.syn_string)),
            ("number", hex(self.syn_number)),
            ("keyword", hex(self.syn_keyword)),
            ("statement", hex(self.syn_statement)),
            ("func", hex(self.syn_func)),
            ("type", hex(self.syn_type)),
            ("property", hex(self.syn_property)),
            ("constructor", hex(self.syn_constructor)),
            ("special", hex(self.syn_special)),
            ("error", hex(self.red)),
            ("warn", hex(self.yellow)),
            ("info", hex(self.blue)),
        ]
    }

    pub fn pastel_dark() -> Self {
        Self {
            name: IdeThemeName::PastelDark,
            bg: 0x000000,
            fg: 0xe8e8e8,
            surface: 0x1a1a1a,
            hover: 0x1f1f1f,
            border: 0x1c1c1c,
            muted: 0x5a5a5a,
            dim: 0xb0b0b0,
            accent: 0xe8e8e8,
            folder: 0x7ebae4,
            red: 0xef8891,
            green: 0x9fe8c3,
            yellow: 0xfbdf90,
            blue: 0x99aee5,
            magenta: 0xc2a2e3,
            cyan: 0xb5c3ea,
            white: 0xb5bcc9,
            black: 0x000000,
            syn_comment: 0x7a7a7a,
            syn_string: 0x9fe8c3,
            syn_number: 0xeda685,
            syn_keyword: 0xc2a2e3,
            syn_statement: 0xc2a2e3,
            syn_func: 0x99aee5,
            syn_type: 0xfbdf90,
            syn_property: 0x99aee5,
            syn_constructor: 0xb5c3ea,
            syn_special: 0xef8891,
        }
    }

    pub fn nvchad_one() -> Self {
        Self {
            name: IdeThemeName::NvChadOne,
            bg: 0x1e222a,
            fg: 0xabb2bf,
            surface: 0x282c34,
            hover: 0x353b45,
            border: 0x31353d,
            muted: 0x565c64,
            dim: 0x6f737b,
            accent: 0x61afef,
            folder: 0x61afef,
            red: 0xe06c75,
            green: 0x98c379,
            yellow: 0xe7c787,
            blue: 0x61afef,
            magenta: 0xc678dd,
            cyan: 0x56b6c2,
            white: 0xabb2bf,
            black: 0x1e222a,
            syn_comment: 0x565c64,
            syn_string: 0x98c379,
            syn_number: 0xd19a66,
            syn_keyword: 0xc678dd,
            syn_statement: 0xe06c75,
            syn_func: 0x61afef,
            syn_type: 0xe5c07b,
            syn_property: 0xe06c75,
            syn_constructor: 0x56b6c2,
            syn_special: 0xbe5046,
        }
    }

    pub fn tokyo_night() -> Self {
        Self {
            name: IdeThemeName::TokyoNight,
            bg: 0x1a1b26,
            fg: 0xc0caf5,
            surface: 0x24283b,
            hover: 0x292e42,
            border: 0x3b4261,
            muted: 0x565f89,
            dim: 0xa9b1d6,
            accent: 0x7aa2f7,
            folder: 0x7aa2f7,
            red: 0xf7768e,
            green: 0x9ece6a,
            yellow: 0xe0af68,
            blue: 0x7aa2f7,
            magenta: 0xbb9af7,
            cyan: 0x7dcfff,
            white: 0xc0caf5,
            black: 0x11121d,
            syn_comment: 0x565f89,
            syn_string: 0x9ece6a,
            syn_number: 0xff9e64,
            syn_keyword: 0x7aa2f7,
            syn_statement: 0xbb9af7,
            syn_func: 0x7aa2f7,
            syn_type: 0x2ac3de,
            syn_property: 0x73daca,
            syn_constructor: 0x7dcfff,
            syn_special: 0xe0af68,
        }
    }

    pub fn catppuccin_mocha() -> Self {
        Self {
            name: IdeThemeName::CatppuccinMocha,
            bg: 0x1e1e2e,
            fg: 0xcdd6f4,
            surface: 0x313244,
            hover: 0x45475a,
            border: 0x585b70,
            muted: 0x6c7086,
            dim: 0xa6adc8,
            accent: 0xcba6f7,
            folder: 0x89b4fa,
            red: 0xf38ba8,
            green: 0xa6e3a1,
            yellow: 0xf9e2af,
            blue: 0x89b4fa,
            magenta: 0xcba6f7,
            cyan: 0x89dceb,
            white: 0xcdd6f4,
            black: 0x11111b,
            syn_comment: 0x6c7086,
            syn_string: 0xa6e3a1,
            syn_number: 0xfab387,
            syn_keyword: 0x89b4fa,
            syn_statement: 0xcba6f7,
            syn_func: 0xf9e2af,
            syn_type: 0x94e2d5,
            syn_property: 0x89dceb,
            syn_constructor: 0x89dceb,
            syn_special: 0xf5c2e7,
        }
    }

    /// Whether this palette is dark (background luminance below half).
    /// Drives derived roles like [`Self::panel_bg`] so light themes
    /// (e.g. a Mash Up Pack's retro look) don't inherit pitch-black
    /// panels tuned for dark themes.
    pub fn is_dark(&self) -> bool {
        let r = ((self.bg >> 16) & 0xff) as f32;
        let g = ((self.bg >> 8) & 0xff) as f32;
        let b = (self.bg & 0xff) as f32;
        // Rec. 601 luma — cheap and good enough for a binary split.
        (0.299 * r + 0.587 * g + 0.114 * b) < 128.0
    }

    /// Deep elevated-panel background (floating pickers, hover docs,
    /// code blocks, reasoning cards). Dark themes keep the historical
    /// `black` token (darker than `bg` — the "deep panel" look); light
    /// themes use `white` (paper panel) since near-black cards on a
    /// light bg read as unthemed holes.
    pub fn panel_bg(&self) -> u32 {
        if self.is_dark() {
            self.black
        } else {
            self.white
        }
    }

    pub fn f32(self, color: u32) -> [f32; 4] {
        let r = ((color >> 16) & 0xff) as f32 / 255.0;
        let g = ((color >> 8) & 0xff) as f32 / 255.0;
        let b = (color & 0xff) as f32 / 255.0;
        [r, g, b, 1.0]
    }

    pub fn f32_alpha(self, color: u32, alpha: f32) -> [f32; 4] {
        let mut out = self.f32(color);
        out[3] = alpha;
        out
    }

    pub fn u8(self, color: u32) -> [u8; 4] {
        [
            ((color >> 16) & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            (color & 0xff) as u8,
            255,
        ]
    }

    pub fn u8_alpha(self, color: u32, alpha: f32) -> [u8; 4] {
        let mut out = self.u8(color);
        out[3] = (255.0 * alpha.clamp(0.0, 1.0)) as u8;
        out
    }

    pub fn rgb(self, color: u32) -> ColorRgb {
        ColorRgb {
            r: ((color >> 16) & 0xff) as u8,
            g: ((color >> 8) & 0xff) as u8,
            b: (color & 0xff) as u8,
        }
    }

    pub fn sugar(self, color: u32) -> Color {
        Color {
            r: ((color >> 16) & 0xff) as f64 / 255.0,
            g: ((color >> 8) & 0xff) as f64 / 255.0,
            b: (color & 0xff) as f64 / 255.0,
            a: 1.0,
        }
    }

    pub fn sugar_alpha(self, color: u32, alpha: f64) -> Color {
        Color {
            r: ((color >> 16) & 0xff) as f64 / 255.0,
            g: ((color >> 8) & 0xff) as f64 / 255.0,
            b: (color & 0xff) as f64 / 255.0,
            a: alpha.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod custom_theme_tests {
    use super::*;

    fn overrides(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parse_theme_hex_forms() {
        assert_eq!(parse_theme_hex("#33ff66"), Some(0x33ff66));
        assert_eq!(parse_theme_hex("33ff66"), Some(0x33ff66));
        assert_eq!(parse_theme_hex("#3f6"), Some(0x33ff66));
        assert_eq!(parse_theme_hex(" #000080 "), Some(0x000080));
        assert_eq!(parse_theme_hex("#33ff6"), None);
        assert_eq!(parse_theme_hex("nope"), None);
    }

    #[test]
    fn from_overrides_applies_and_warns() {
        let (theme, warnings) = IdeTheme::from_overrides(
            "tokyo_night",
            &overrides(&[
                ("bg", "#030f06"),
                ("comment", "#1f7a3c"),
                ("syn_string", "#7dffa8"),
                ("bogus_key", "#ffffff"),
                ("accent", "not-a-color"),
            ]),
        );
        // Overridden slots take, untouched slots keep the base.
        assert_eq!(theme.bg, 0x030f06);
        assert_eq!(theme.syn_comment, 0x1f7a3c);
        assert_eq!(theme.syn_string, 0x7dffa8);
        assert_eq!(theme.fg, IdeTheme::tokyo_night().fg);
        assert_eq!(theme.accent, IdeTheme::tokyo_night().accent);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn registry_roundtrip_shadow_and_intern_reuse() {
        let (phosphor, _) =
            IdeTheme::from_overrides("pastel_dark", &overrides(&[("bg", "#030f06")]));
        replace_custom_ide_themes(vec![
            ("phosphor".to_string(), "Green CRT".to_string(), phosphor),
            // Builtin names must not be shadowable.
            ("tokyo_night".to_string(), String::new(), phosphor),
        ]);

        assert_eq!(IdeTheme::by_name("phosphor").bg, 0x030f06);
        assert_eq!(
            IdeTheme::by_name("phosphor").name.as_str(),
            "phosphor",
            "custom theme keeps its own name"
        );
        assert_eq!(
            IdeTheme::by_name("tokyo_night").bg,
            IdeTheme::tokyo_night().bg,
            "builtin wins over a shadowing file"
        );
        let names = all_ide_theme_names();
        assert!(names.contains(&"phosphor".to_string()));
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "tokyo_night").count(),
            1
        );

        // Re-registering the same name must reuse the interned str.
        let first = match IdeTheme::by_name("phosphor").name {
            IdeThemeName::Custom(s) => s.as_ptr(),
            _ => panic!("expected custom"),
        };
        replace_custom_ide_themes(vec![(
            "phosphor".to_string(),
            String::new(),
            phosphor,
        )]);
        let second = match IdeTheme::by_name("phosphor").name {
            IdeThemeName::Custom(s) => s.as_ptr(),
            _ => panic!("expected custom"),
        };
        assert_eq!(first, second, "name interning must not re-leak");

        // Unknown names still fall back to pastel_dark.
        replace_custom_ide_themes(Vec::new());
        assert_eq!(IdeTheme::by_name("phosphor").bg, IdeTheme::pastel_dark().bg);
    }

    #[test]
    fn lua_palette_pairs_cover_the_lua_contract() {
        let pairs = IdeTheme::pastel_dark().lua_palette_pairs();
        for key in [
            "bg",
            "fg",
            "line",
            "surface",
            "muted",
            "comment",
            "string",
            "number",
            "keyword",
            "statement",
            "func",
            "type",
            "property",
            "constructor",
            "special",
            "error",
            "warn",
            "info",
        ] {
            assert!(
                pairs.iter().any(|(k, _)| *k == key),
                "missing lua palette key {key}"
            );
        }
        assert!(pairs
            .iter()
            .all(|(_, v)| v.starts_with('#') && v.len() == 7));
    }
}
