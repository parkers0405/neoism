import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";
import { statusInfo } from "../src/chrome/status-info";
import { NativeEditor } from "../src/native-core";

test("the real Rust top bar owns equal icon hit boxes and the original desktop menu", () => {
  const id = coreCall<{ id: number }>("chrome.new", { kind: "topbar", family: "Cascadia Code NF", fonts: ["Symbols Nerd Font Mono"] }).id;
  const params = { id, width: 1100, height: 720, scale: 1, theme: "pastel_dark", sidebar: "files", connection: "connected", agent: true, details: true, menu: false };
  try {
    const frame: any = coreCall("chrome.frame", params);
    const left = frame.regions.filter((r: any) => ["topbar-menu", "toggle-files", "toggle-notes", "toggle-conversations", "open-finder"].includes(r.id));
    expect(left).toHaveLength(5);
    for (let i = 0; i < left.length; i++) { expect(left[i].rect[2]).toBe(22); expect(left[i].rect[3]).toBe(22); if (i) expect(left[i].rect[0] - left[i - 1].rect[0]).toBe(26); }
    const menu: any = coreCall("chrome.click", { ...params, x: 19, y: 15 });
    expect(menu.menuOpen).toBe(true);
    expect(menu.menuRect[1]).toBe(30);
    expect(menu.regions.filter((r: any) => r.menu).map((r: any) => r.label)).toEqual(["About", "Settings", "Start Web Server", "Themes", "Backgrounds", "Extensions"]);
    const about = menu.regions.find((r: any) => r.id === "menu:OpenAbout").rect;
    const chosen: any = coreCall("chrome.click", { ...params, x: about[0] + 20, y: about[1] + 10 });
    expect(chosen.action).toBe("OpenAbout"); expect(chosen.menuOpen).toBe(false);
    expect(coreCall<any>("chrome.frame", params).action).toBeNull();
  } finally { coreCall("chrome.dispose", { id }); }
});

test("status line renders original mode/directory/ruler pills without transport text", async () => {
  const id = coreCall<{ id: number }>("chrome.new", { kind: "status", family: "Cascadia Code NF" }).id;
  const info = { mode: "Normal", cwd_label: "~/projects/neoism", cursor_lines: [3, 80], pending_keys: "2d", diagnostics: { error: 2, warn: 1, info: 0, hint: 0 }, fps: 120 };
  const params = { id, width: 1200, scale: 1, theme: "pastel_dark", info, connection: "connected" };
  try {
    coreCall("chrome.frame", params); await Bun.sleep(350);
    const frame: any = coreCall("chrome.frame", params);
    const text = frame.commands.filter((c: any) => c.kind === "text").map((c: any) => c.text).join("");
    expect(text).toContain("~/projects/neoism"); expect(text).toContain("3/80"); expect(text).toContain("FPS");
    expect(text.toLowerCase()).not.toContain("connected");
    expect(frame.commands.some((c: any) => c.kind === "rect" && c.radii.some((r: number) => r > 0))).toBe(true);
    expect(frame.commands.filter((c: any) => c.kind === "text" && c.text.trim()).every((c: any) => !!c.raster?.id)).toBe(true);
  } finally { coreCall("chrome.dispose", { id }); }
});

test("status metadata follows actual editor input mode and source line count", () => {
  const editor = new NativeEditor("main.ts", "one\ntwo\nthree", false);
  try {
    const surface = { id: "file:one", kind: "file" as const, title: "main.ts", path: "main.ts", workspaceId: "one" };
    const info = statusInfo(surface, "/project", { mode: "Normal", line: 1, col: 0, dirty: false }, editor.frame);
    expect(info.mode).toBe("Insert"); expect(info.cursor_lines).toEqual([2, 3]);
    expect(statusInfo({ ...surface, path: "README.md" }, "/project", { mode: "Normal", line: 0, col: 0, dirty: false }, editor.frame).mode).toBe("Markdown");
  } finally { editor.dispose(); }
});
