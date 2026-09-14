import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const styles = readFileSync(new URL("../style.css", import.meta.url), "utf8");
const avatarStyles = readFileSync(new URL("./user-avatar.css", import.meta.url), "utf8");
const selector = ".message.user:not(.runtime-message)";
const rule = (source: string, selector: string) => source.slice(source.indexOf(selector), source.indexOf("}", source.indexOf(selector)));
describe("small curved iMessage-style user tail", () => {
    it("shares the exact bubble blue and uses a rounded cubic taper, not a triangle or background cutout", () => {
        const bubble = rule(styles, `${selector} {`), tail = rule(styles, `${selector}::before`);
        expect(bubble).toContain("--bubble-blue: #007aff");
        expect(bubble).toContain("background: var(--bubble-blue)");
        expect(bubble).toContain("border-radius: 20px 20px 16px 20px");
        expect(tail).toContain("background: var(--bubble-blue)");
        expect(tail).toContain("right: -5px");
        expect(tail).toContain("width: 16px");
        expect(tail).toContain("height: 18px");
        expect(tail).toContain("C8 11 10 14 15 16Q16 16.5 15 17");
        expect(tail).toContain("-webkit-mask: var(--bubble-tail)");
        expect(tail).toContain("mask: var(--bubble-tail)");
        expect(tail).toContain("pointer-events: none");
        expect(tail).not.toMatch(/polygon|border-(?:left|right|top|bottom):|var\(--bg\)/);
        expect(styles).not.toContain(`${selector}::after`);
    });
    it("does not clip attachments or consume wrapping width, and keeps the right avatar gap", () => {
        const bubble = rule(styles, `${selector} {`);
        expect(bubble).toContain("width: fit-content");
        expect(bubble).toContain("min-width: 0");
        expect(bubble).not.toContain("overflow: hidden");
        expect(rule(styles, `${selector} img`)).toContain("max-width: 100%");
        expect(rule(styles, `${selector}::before`)).toContain("position: absolute");
        expect(avatarStyles).toContain("margin-right: 44px");
        expect(avatarStyles).toContain("max-width: min(calc(100% - 60px), 38rem, 75%)");
        expect(avatarStyles).toContain("right: -44px; bottom: 0; width: 28px; height: 28px");
    });
});
