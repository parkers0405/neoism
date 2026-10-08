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

/// Surface opacity affects material fills, never foreground or focus indicators.
pub fn background_opacity(style: &neoism_lua::StylePatch) -> f32 {
    style
        .opacity
        .filter(|value| value.is_finite())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
}

pub fn apply_background_opacity(
    style: &neoism_lua::StylePatch,
    mut color: [f32; 4],
) -> [f32; 4] {
    color[3] *= background_opacity(style);
    color
}

pub fn background_color(
    style: &neoism_lua::StylePatch,
    theme: &IdeTheme,
    fallback: [f32; 4],
) -> [f32; 4] {
    apply_background_opacity(
        style,
        color_f32(style.background.as_deref(), theme, fallback),
    )
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
            .filter(|value| value.is_ascii() && value.len() >= 6)
            .and_then(|value| u32::from_str_radix(&value[..6], 16).ok())
            .unwrap_or(fallback),
    }
}

pub fn color_f32(value: Option<&str>, theme: &IdeTheme, fallback: [f32; 4]) -> [f32; 4] {
    let Some(value) = value else { return fallback };
    match value {
        "transparent" => [0.0; 4],
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
    color_f32(value, theme, fallback.map(|channel| channel as f32 / 255.0))
        .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_hex(value: &str) -> Option<[f32; 4]> {
    let value = value.strip_prefix('#')?;
    if !value.is_ascii() {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_opacity_preserves_defaults_and_rgb() {
        let theme = IdeTheme::default();
        let fallback = [0.1, 0.2, 0.3, 0.6];
        let mut style = neoism_lua::StylePatch::default();
        assert_eq!(background_color(&style, &theme, fallback), fallback);
        style.opacity = Some(1.0);
        assert_eq!(background_color(&style, &theme, fallback), fallback);
        style.opacity = Some(0.25);
        assert_eq!(
            background_color(&style, &theme, fallback),
            [0.1, 0.2, 0.3, 0.15]
        );
        style.opacity = Some(0.0);
        assert_eq!(
            background_color(&style, &theme, fallback),
            [0.1, 0.2, 0.3, 0.0]
        );
        style.background = Some("#00000080".into());
        style.opacity = Some(0.5);
        assert_eq!(
            background_color(&style, &theme, fallback),
            [0.0, 0.0, 0.0, 64.0 / 255.0]
        );
        style.background = Some("transparent".into());
        assert_eq!(background_color(&style, &theme, fallback), [0.0; 4]);
    }

    #[test]
    fn material_opacity_clamps_and_ignores_nonfinite_values() {
        for (value, expected) in [
            (None, 1.0),
            (Some(-1.0), 0.0),
            (Some(2.0), 1.0),
            (Some(f32::NAN), 1.0),
            (Some(f32::INFINITY), 1.0),
        ] {
            let style = neoism_lua::StylePatch {
                opacity: value,
                ..Default::default()
            };
            assert_eq!(background_opacity(&style), expected);
        }
    }

    #[test]
    fn material_opacity_inherits_without_changing_foreground_theme() {
        let mut styles = neoism_lua::StyleSheet::default();
        styles.insert(
            "agent.sidebar",
            neoism_lua::StylePatch {
                opacity: Some(0.25),
                ..Default::default()
            },
        );
        let style = styles.resolve("agent.sidebar.row.selected");
        let theme = IdeTheme::default();
        assert_eq!(background_opacity(&style), 0.25);
        let styled = styled_ide_theme(theme, &style);
        assert_eq!(
            (
                styled.bg,
                styled.surface,
                styled.fg,
                styled.accent,
                styled.border
            ),
            (
                theme.bg,
                theme.surface,
                theme.fg,
                theme.accent,
                theme.border
            ),
        );
        styles.insert(
            "agent.sidebar.row.selected",
            neoism_lua::StylePatch {
                opacity: Some(0.5),
                ..Default::default()
            },
        );
        assert_eq!(
            background_opacity(&styles.resolve("agent.sidebar.row.selected")),
            0.5
        );
    }

    #[test]
    fn background_colors_preserve_alpha_and_defaults() {
        let theme = IdeTheme::default();
        let fallback = theme.f32(theme.surface);
        assert_eq!(color_f32(None, &theme, fallback), fallback);
        assert_eq!(color_f32(Some("invalid"), &theme, fallback), fallback);
        assert_eq!(
            color_f32(Some("#\u{1f5a4}ffff"), &theme, fallback),
            fallback
        );
        assert_eq!(
            color_u32(Some("#\u{1f5a4}ffff"), &theme, theme.bg),
            theme.bg
        );
        assert_eq!(color_f32(Some("surface"), &theme, fallback), fallback);
        assert_eq!(color_f32(Some("transparent"), &theme, fallback), [0.0; 4]);
        assert_eq!(color_f32(Some("#00000000"), &theme, fallback), [0.0; 4]);
        assert_eq!(
            color_f32(Some("#00000080"), &theme, fallback),
            [0.0, 0.0, 0.0, 128.0 / 255.0],
        );
        assert_eq!(color_f32(Some("#ffffff"), &theme, fallback), [1.0; 4],);
    }
}
