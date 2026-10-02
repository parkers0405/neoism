use crate::primitives::IdeTheme;

pub fn styled_ide_theme(mut theme: IdeTheme, style: &neoism_lua::StylePatch) -> IdeTheme {
    theme.bg = color_u32(style.background.as_deref(), &theme, theme.bg);
    theme.surface = color_u32(style.background.as_deref(), &theme, theme.surface);
    theme.fg = color_u32(style.foreground.as_deref(), &theme, theme.fg);
    theme.dim = color_u32(style.muted.as_deref(), &theme, theme.dim);
    theme.muted = color_u32(style.muted.as_deref(), &theme, theme.muted);
    theme.accent = color_u32(style.accent.as_deref(), &theme, theme.accent);
    theme.blue = color_u32(style.accent.as_deref(), &theme, theme.blue);
    theme.border = color_u32(style.border_color.as_deref(), &theme, theme.border);
    theme
}

pub fn color_u32(value: Option<&str>, theme: &IdeTheme, fallback: u32) -> u32 {
    let Some(value) = value else { return fallback };
    match value {
        "bg" | "background" => theme.bg,
        "surface" => theme.surface,
        "fg" | "foreground" => theme.fg,
        "muted" | "dim" => theme.dim,
        "accent" => theme.accent,
        "border" => theme.border,
        "hover" | "selection" => theme.hover,
        "red" | "error" => theme.red,
        "green" | "success" => theme.green,
        "yellow" | "warning" => theme.yellow,
        "blue" | "info" => theme.blue,
        value => value
            .strip_prefix('#')
            .filter(|value| value.len() >= 6)
            .and_then(|value| u32::from_str_radix(&value[..6], 16).ok())
            .unwrap_or(fallback),
    }
}

pub fn color_f32(value: Option<&str>, theme: &IdeTheme, fallback: [f32; 4]) -> [f32; 4] {
    let Some(value) = value else { return fallback };
    match value {
        "bg" | "background" => theme.f32(theme.bg),
        "surface" => theme.f32(theme.surface),
        "fg" | "foreground" => theme.f32(theme.fg),
        "muted" | "dim" => theme.f32(theme.dim),
        "accent" => theme.f32(theme.accent),
        "border" => theme.f32(theme.border),
        "hover" => theme.f32(theme.hover),
        "selection" => theme.f32(theme.hover),
        "red" | "error" => theme.f32(theme.red),
        "green" | "success" => theme.f32(theme.green),
        "yellow" | "warning" => theme.f32(theme.yellow),
        "blue" | "info" => theme.f32(theme.blue),
        value => parse_hex(value).unwrap_or(fallback),
    }
}

pub fn color_u8(value: Option<&str>, theme: &IdeTheme, fallback: [u8; 4]) -> [u8; 4] {
    color_f32(
        value,
        theme,
        fallback.map(|channel| channel as f32 / 255.0),
    )
    .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_hex(value: &str) -> Option<[f32; 4]> {
    let value = value.strip_prefix('#')?;
    let (rgb, alpha) = match value.len() {
        6 => (value, 255),
        8 => (&value[..6], u8::from_str_radix(&value[6..], 16).ok()?),
        _ => return None,
    };
    Some([
        u8::from_str_radix(&rgb[..2], 16).ok()? as f32 / 255.0,
        u8::from_str_radix(&rgb[2..4], 16).ok()? as f32 / 255.0,
        u8::from_str_radix(&rgb[4..], 16).ok()? as f32 / 255.0,
        alpha as f32 / 255.0,
    ])
}