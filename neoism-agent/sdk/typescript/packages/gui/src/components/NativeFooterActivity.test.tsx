// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { Composer, ComposerFooter } from "./Composer";
import { footerScannerFrame, NativeFooterActivity } from "./NativeFooterActivity";
(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

it("ports native 40ms, 8-forward/9-hold/7-back/30-hold phases and six-cell trail", () => {
    const at = (frame: number) => footerScannerFrame(frame * .04);
    const active = (frame: number) => at(frame).flatMap((c, i) => c.active ? [i] : []);
    expect(active(0)).toEqual([0]);
    expect(active(7)).toEqual([2, 3, 4, 5, 6, 7]);
    expect(active(8)).toEqual(active(7));
    expect(active(14)).toEqual([]);
    expect(active(17)).toEqual([6, 7]);
    expect(active(23)).toEqual([0, 1, 2, 3, 4, 5]);
    expect(active(24)).toEqual(active(23));
    expect(active(30)).toEqual([]);
    expect(at(54)).toEqual(at(0));
    expect(at(7)[6].bloom).toBeCloseTo(.19);
    expect(at(7)[5].alpha).toBe(Math.round(.65 * 255) / 255);
    expect(footerScannerFrame(NaN)).toEqual(at(0));
});

it("keeps home hints and places busy squares at the right of the chat controls", () => {
    const host = document.createElement("div"), root = createRoot(host);
    act(() => root.render(<ComposerFooter directory="/repo" />));
    expect(host.querySelector(".project-pill")?.textContent).toContain("repo");
    expect(host.querySelector(".native-composer-hints")?.textContent).toContain("commands");
    expect(host.querySelector(".native-footer-activity")).toBeNull();
    const props = {sessionId:"chat",directory:"/repo",busy:true,commands:[],send:async () => {},abort:() => {},model:"provider/model",agent:"Build",thinking:"medium",openPicker:() => {}};
    act(() => root.render(<Composer {...props} />));
    expect(host.querySelectorAll(".native-composer-chips .native-footer-cell")).toHaveLength(8);
    expect(host.querySelector(".native-composer-chips")?.lastElementChild?.className).toBe("native-footer-activity");
    expect(host.querySelector(".native-composer-footer")).toBeNull();
    act(() => root.render(<Composer {...props} busy={false} />));
    expect(host.querySelector(".native-footer-activity")).toBeNull();
    act(() => root.unmount());
});

it("never renders below-input hints in a session-backed footer", () => {
    const host = document.createElement("div"), root = createRoot(host);
    act(() => root.render(<ComposerFooter sessionId="chat" directory="/repo" busy showProject />));
    expect(host.textContent).toBe("");
    expect(host.querySelector(".native-composer-footer")).toBeNull();
    act(() => root.unmount());
});

it("stops its clock for reduced motion, resumes on preference changes, and cleans up", () => {
    vi.useFakeTimers();
    const motion = Object.assign(new EventTarget(), { matches: true });
    vi.stubGlobal("matchMedia", () => motion);
    const host = document.createElement("div"), root = createRoot(host);
    act(() => root.render(<NativeFooterActivity busy />));
    expect(vi.getTimerCount()).toBe(0);
    expect((host.querySelector(".native-footer-cell") as HTMLElement).style.getPropertyValue("--scanner-size")).toBe("6.5px");
    motion.matches = false;
    act(() => motion.dispatchEvent(new Event("change")));
    expect(vi.getTimerCount()).toBe(1);
    motion.matches = true;
    act(() => motion.dispatchEvent(new Event("change")));
    expect(vi.getTimerCount()).toBe(0);
    act(() => root.unmount());
    expect(vi.getTimerCount()).toBe(0);
});

it("uses native square geometry, depth offsets and live magenta/fg roles without gray remapping", () => {
    const css = readFileSync("src/components/nativeFooterActivity.css", "utf8");
    for (const rule of ["68px", "8.5px", "12.5px", "border-radius: 0", "translate(3px, 3px)", "translate(1.5px, 1.5px)", "--theme-magenta", "--theme-fg", "--theme-bg", "--theme-dim"]) expect(css).toContain(rule);
    expect(css).not.toMatch(/--muted|--text-faint|--accent/);
});
