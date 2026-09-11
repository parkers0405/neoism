// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { NativeActivity } from "./nativeActivity";
let root: Root, container: HTMLDivElement;
let reduced = false;
let callbacks: Map<number, FrameRequestCallback>, next: number;
const paint = vi.fn();
beforeEach(() => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    reduced = false; callbacks = new Map(); next = 1;
    vi.stubGlobal("requestAnimationFrame", vi.fn((fn: FrameRequestCallback) => { callbacks.set(next, fn); return next++; }));
    vi.stubGlobal("cancelAnimationFrame", vi.fn((id: number) => callbacks.delete(id)));
    vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: reduced, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => ({
        clearRect: vi.fn(), fillRect: vi.fn(), fillText: paint, setTransform: vi.fn(),
        measureText: (text: string) => ({ width: text.length * 12, actualBoundingBoxAscent: 12 }),
        getImageData: () => ({ data: [232, 232, 232, 255] }),
    }) as unknown as CanvasRenderingContext2D);
    container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(() => { act(() => root.unmount()); container.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); paint.mockClear(); });
it("isolates frames on canvas, changes reasoning status, and cancels on idle/unmount", () => {
    act(() => root.render(<NativeActivity busy activity={{ status: "thinking" }} sessionId="one" />));
    expect(container.textContent).toBe("Pondering");
    expect(container.querySelector("canvas")?.getAttribute("aria-hidden")).toBe("true");
    const dom = container.innerHTML;
    const [id, tick] = [...callbacks][0]; callbacks.delete(id); act(() => tick(performance.now() + 900));
    expect(container.innerHTML).toBe(dom); expect(paint).toHaveBeenCalled();
    act(() => root.render(<NativeActivity busy activity={{ status: "generating" }} sessionId="one" />));
    expect(container.textContent).toBe("Crafting"); expect(callbacks.size).toBe(1);
    act(() => root.render(<NativeActivity busy={false} sessionId="one" />));
    expect(container.textContent).toBe(""); expect(callbacks.size).toBe(0);
});
it("animates Background for idle-with-jobs and switches to the live provider word then clears", () => {
    const runtime = { rootSessionId: "root", revision: 1, branches: [],
        execution: { executionId: "e", rootSessionId: "root", rootMessageId: "u", revision: 1, finished: false, completedMs: 0, activeSegments: {} as Record<string, number> },
        runningBackgroundTasks: [{ sessionId: "root", jobId: "job", startedAt: 1 }] };
    const render = (busy = false) => act(() => root.render(<NativeActivity busy={busy} runtime={{ ...runtime }} sessionId="root" />));
    render();
    expect(container.textContent).toContain("Background");
    expect(container.querySelector("canvas")).not.toBeNull();
    expect(callbacks.size).toBe(1);
    runtime.execution.activeSegments = { provider: 1 }; render(true);
    expect(container.textContent).toContain("Crafting");
    expect(container.textContent).toContain("1 background task running");
    expect(callbacks.size).toBe(1);
    runtime.execution.activeSegments = {}; render();
    expect(container.textContent).toContain("Background");
    runtime.runningBackgroundTasks = []; render();
    expect(container.querySelector('[role="status"]')).toBeNull();
    expect(callbacks.size).toBe(0);
});
it("reduced motion paints resolved text without scheduling animation frames", () => {
    reduced = true;
    act(() => root.render(<NativeActivity busy activity={{ status: "thinking" }} />));
    expect(callbacks.size).toBe(0);
    expect(paint.mock.calls.some(([glyph]) => glyph === "P")).toBe(true);
});
it("preserves exact native child branches and accessible status", () => {
    act(() => root.render(<NativeActivity busy activity={{ status: "waitingSubagents", queuedCount: 2, backgroundCount: 1 }} />));
    expect(container.querySelector('[role="status"]')).not.toBeNull();
    expect(container.querySelector(".native-activity-queue")?.textContent).toBe("├─ queued messages (2)");
    expect(container.querySelector(".native-activity-background")?.textContent).toBe("╰─ 1 background task running");
});
