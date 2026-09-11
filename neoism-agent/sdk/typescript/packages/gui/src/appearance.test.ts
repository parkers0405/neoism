import { describe, expect, it } from "vitest";
import { appearanceTokens, themeOptions, DEFAULT_GUI_THEME } from "./appearance";
import { themes } from "./generated/themes";

describe("GUI appearance tokens", () => {
    it("uses the native Pastelbeans palette without a gray override", () => {
        const tokens = appearanceTokens(themeOptions.find(theme => theme.id === DEFAULT_GUI_THEME)!);
        expect(themeOptions.some(theme => theme.id === "neoism")).toBe(false);
        expect(tokens.colorScheme).toBe("dark");
        expect(tokens.variables).toMatchObject({
            bg: "#151515", fg: "#e8e8d3", panel: "#252525",
            accent: "#ff9da4", border: "#2d2d2d",
            "surface-2": "#2e2e2e", "surface-3": "#2e2e2e", muted: "#525252",
        });
    });
    it("preserves every native palette with complete semantic surfaces", () => {
        expect(themeOptions).toHaveLength(themes.length);
        expect(new Set(themeOptions.map((theme) => theme.id)).size).toBe(themeOptions.length);
        for (const theme of themes) {
            const tokens = appearanceTokens(theme);
            expect(tokens.variables.bg).toBe(theme.colors.bg);
            expect(tokens.variables.fg).toBe(theme.colors.fg);
            expect(tokens.variables.accent).toBe(theme.colors.accent);
            expect(tokens.variables["surface-1"]).toBe(theme.colors.surface);
            expect(tokens.variables["surface-3"]).toBe(theme.colors.hover);
            for (const value of Object.values(tokens.variables)) {
                expect(value).toBeTruthy();
                expect(value).not.toContain("undefined");
                expect(value).not.toContain("NaN");
            }
        }
        expect(themes.some((theme) => appearanceTokens(theme).colorScheme === "light")).toBe(true);
    });
});
