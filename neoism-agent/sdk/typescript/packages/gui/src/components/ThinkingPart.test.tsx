// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ThinkingPart, type ThinkingPartProps } from "./ThinkingPart";
const part = (text = "Consider **constraints** first.", time: unknown = { start: 1000 }): ThinkingPartProps["part"] => ({ type: "reasoning", id: "r", messageId: "m", sessionId: "s", text, time } as ThinkingPartProps["part"]);
let root: Root, el: HTMLDivElement;
beforeEach(() => { (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true; el = document.createElement("div"); document.body.append(el); root = createRoot(el); });
afterEach(() => { act(() => root.unmount()); el.remove(); vi.restoreAllMocks(); });
it("matches native Thinking label and expanded nonempty default without a first-line summary", () => {
    act(() => root.render(<ThinkingPart part={part()} />));
    expect(el.querySelector("button")?.textContent).toBe("Thinking"); expect(el.querySelector("button")?.getAttribute("aria-expanded")).toBe("true"); expect(el.querySelector("strong")?.textContent).toBe("constraints");
    expect(el.querySelector(".status-dot")).toBeNull();
});
it("keeps manual collapse across token updates and completion and resets for a different block", () => {
    act(() => root.render(<ThinkingPart part={part()} />)); act(() => el.querySelector("button")!.click());
    for (const p of [part("more tokens"), part("finished", { start: 1000, end: 9123 })]) {
        act(() => root.render(<ThinkingPart part={p} />)); expect(el.querySelector("button")?.getAttribute("aria-expanded")).toBe("false"); expect(el.querySelector(".markdown")).toBeNull();
    }
    act(() => el.querySelector("button")!.click()); expect(el.textContent).toContain("finished");
    act(() => root.render(<ThinkingPart part={{ ...part(), id: "other" }} />)); expect(el.querySelector("button")?.getAttribute("aria-expanded")).toBe("true");
});
it("does not fabricate elapsed wording or a live timer, even with exact known timestamps", () => {
    const interval = vi.spyOn(globalThis, "setInterval");
    for (const time of [undefined, {}, { start: 1000 }, { start: 1000, end: 9123 }]) {
        act(() => root.render(<ThinkingPart part={part("same", time)} />));
        expect(el.querySelector("button")?.textContent).toBe("Thinking"); expect(el.querySelector("time")).toBeNull(); expect(el.textContent).not.toMatch(/Thought|9s|8s|NaN/);
    }
    expect(interval).not.toHaveBeenCalled();
});
it("hides empty reasoning and preserves safe Markdown with terminal controls removed", () => {
    act(() => root.render(<ThinkingPart part={part("  \n")} />)); expect(el.innerHTML).toBe("");
    act(() => root.render(<ThinkingPart part={part('\x1b[31mVisible\x1b[0m\n\n<script>alert(1)</script>\n\n[bad](javascript:alert)')} />));
    expect(el.textContent).toContain("Visible"); expect(el.innerHTML).not.toContain("\x1b"); expect(el.querySelector("script")).toBeNull(); expect(el.querySelector('a[href^="javascript:"]')).toBeNull();
});
