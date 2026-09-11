import { themes, type Theme } from "./generated/themes";

export const DEFAULT_GUI_THEME = "pastelbeans";
export const themeOptions: Theme[] = themes;
export const resolveTheme = (id: string): Theme => themeOptions.find(theme => theme.id === id)
    || themeOptions.find(theme => theme.id === DEFAULT_GUI_THEME)!;

export function appearanceTokens(theme: Theme) {
    const c = theme.colors;
    const bg = c.bg || c.background;
    const fg = c.fg || c.foreground;
    const rgb = bg.replace("#", "");
    const light = [0.299, 0.587, 0.114].reduce((sum, weight, index) =>
        sum + parseInt(rgb.slice(index * 2, index * 2 + 2), 16) * weight, 0) > 150;
    const surface = c.surface || `color-mix(in srgb, ${bg} 93%, ${fg})`;
    return {
        colorScheme: light ? "light" : "dark",
        variables: {
            bg, fg, panel: surface,
            "surface-1": surface,
            "surface-2": c.hover || surface,
            "surface-3": c.hover || surface,
            muted: c.dim || c.muted || fg,
            "text-faint": c.muted || c.dim || fg,
            accent: c.accent || fg,
            border: c.border || `color-mix(in srgb, ${fg} 10%, transparent)`,
            "neutral-button": `color-mix(in srgb, ${fg} 6%, transparent)`,
            "overlay-shadow": light
                ? "0 16px 32px #0002, 0 8px 16px #0001, 0 0 0 .5px #0002"
                : "0 16px 32px #0005, 0 8px 16px #0005, 0 0 0 .5px #ffffff29, inset 0 1px 0 #ffffff0a",
        },
    };
}
