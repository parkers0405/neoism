import { afterEach, describe, expect, it, vi } from "vitest";
import { defaultPreferences, loadPreferences } from "./types";
import { fonts } from "./generated/fonts";

afterEach(() => vi.unstubAllGlobals());
describe("separate interface and code font preferences", () => {
    it("defaults to Geist text and the bundled JetBrains Mono code face", () => {
        expect(defaultPreferences.font).toBe("geist");
        expect(defaultPreferences.codeFont).toBe("jetbrains-mono");
        expect(fonts.find(font => font.id === defaultPreferences.font)?.family).toBe("Geist");
        expect(fonts.find(font => font.id === defaultPreferences.codeFont)?.family).toBe("Neoism JetBrains Mono");
    });
    it("adds a code default without losing existing interface choices", () => {
        vi.stubGlobal("localStorage", {getItem: () => JSON.stringify({font:"system-sans"})});
        expect(loadPreferences()).toMatchObject({font:"system-sans",codeFont:"jetbrains-mono"});
    });
    it("restores interface and code choices independently", () => {
        vi.stubGlobal("localStorage", {getItem: () => JSON.stringify({font:"geist",codeFont:"geist-mono"})});
        expect(loadPreferences()).toMatchObject({font:"geist",codeFont:"geist-mono"});
    });
});
