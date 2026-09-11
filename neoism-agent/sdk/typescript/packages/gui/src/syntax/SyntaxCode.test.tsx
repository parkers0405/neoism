// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import { SyntaxCode } from "./SyntaxCode";
import { Markdown, copyCode } from "../components/Markdown";
const { highlight } = vi.hoisted(() => ({ highlight: vi.fn() }));
vi.mock("./client", () => ({ highlight }));
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
afterEach(() => { vi.useRealTimers(); vi.clearAllMocks(); vi.unstubAllGlobals(); });
it("renders escaped source, ignores stale streaming results, and never reparses for theme changes", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("IntersectionObserver", undefined);
    const host = document.createElement("div"); document.body.append(host);
    const root = createRoot(host);
    let finish!: (spans: { start: number; end: number; token: string }[]) => void;
    highlight.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    highlight.mockResolvedValue([{ start: 0, end: 6, token: "string" }]);
    try {
        await act(async () => root.render(<SyntaxCode source="old" language="rust" />));
        await act(async () => { await vi.advanceTimersByTimeAsync(100); });
        await act(async () => root.render(<SyntaxCode source="<img/>" language="rust" />));
        await act(async () => { finish([{ start: 0, end: 3, token: "keyword" }]); await vi.advanceTimersByTimeAsync(100); });
        expect(host.textContent).toBe("<img/>"); expect(host.querySelector("img")).toBeNull();
        expect(host.querySelector(".neo-syn-string")?.textContent).toBe("<img/>");
        const count = highlight.mock.calls.length;
        host.style.setProperty("--theme-syn_string", "red");
        await act(async () => root.render(<SyntaxCode source="<img/>" language="rust" />));
        expect(highlight).toHaveBeenCalledTimes(count);
        expect(highlight.mock.calls[0][2].aborted).toBe(true);
    } finally { await act(async () => root.unmount()); host.remove(); }
});
it("unsupported fences retain language header, whitespace and copy error contract", async () => {
    const host = document.createElement("div"), root = createRoot(host);
    try {
        await act(async () => root.render(<Markdown text={'```unknown\n  <unsafe>\n\n```'} />));
        expect(host.querySelector(".neo-code-header")?.textContent).toBe("unknown");
        expect(host.querySelector("pre code")?.textContent).toBe("  <unsafe>\n\n");
        expect(host.querySelector("unsafe")).toBeNull();
        expect(host.querySelector('button[aria-label="Copy code"]')).not.toBeNull();
        expect(highlight).not.toHaveBeenCalled();
        const write = vi.fn().mockResolvedValue(undefined);
        expect(await copyCode("  <unsafe>\n\n", write)).toBeUndefined();
        expect(write).toHaveBeenCalledWith("  <unsafe>\n\n");
        expect(await copyCode("x", async () => { throw Error(); })).toBe("Could not copy code. Check clipboard permissions and try again.");
    } finally { await act(async () => root.unmount()); }
});
