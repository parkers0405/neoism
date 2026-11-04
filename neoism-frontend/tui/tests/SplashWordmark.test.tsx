import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { Writable } from "node:stream";
import { PNG } from "pngjs";
import { CliRenderEvents, NativeImage, RGBA, createCliRenderer } from "@opentui/core";
import { setRendererCapabilities } from "@opentui/core/testing";
import { render, testRender, useTerminalDimensions } from "@opentui/solid";
import { createSignal } from "solid-js";
import { SplashWordmark, splashWordmarkCrop, splashWordmarkSource } from "../src/SplashWordmark";

const disposals: Array<() => void | Promise<void>> = [];
afterEach(async () => { for (const dispose of disposals.splice(0).reverse()) await dispose(); });

async function setup(fg = "#ffffff", bg = "#182030") {
  const [ink, setInk] = createSignal(fg);
  const screen = await testRender(() => {
    const dimensions = useTerminalDimensions();
    return <box width="100%" height="100%" backgroundColor={bg}>
      <SplashWordmark columns={dimensions().width} fg={ink()} />
    </box>;
  }, { width: 64, height: 20 });
  disposals.push(() => screen.dispose());
  await screen.flush();
  const wordmark = screen.renderer.root.findDescendantById("splash-wordmark")!;
  return { ...screen, wordmark, setInk };
}

test("uses the exact white/transparent GUI PNG; native decoding is not the black-hole cause", () => {
  expect(splashWordmarkSource.pathname).toEndWith("/shared/assets/splash/neoism-wordmark.png");
  const bytes = readFileSync(splashWordmarkSource);
  const png = PNG.sync.read(bytes);
  let left = png.width, top = png.height, right = -1, bottom = -1, opaque = 0, transparent = 0, colored = 0;
  for (let y = 0; y < png.height; y++) for (let x = 0; x < png.width; x++) {
    const i = (y * png.width + x) * 4;
    if (png.data[i + 3]) {
      if (png.data[i] !== 255 || png.data[i + 1] !== 255 || png.data[i + 2] !== 255) colored++;
      opaque++;
      left = Math.min(left, x); top = Math.min(top, y);
      right = Math.max(right, x); bottom = Math.max(bottom, y);
    } else transparent++;
  }
  expect(opaque).toBeGreaterThan(100000);
  expect(colored).toBe(0);
  expect(transparent).toBeGreaterThan(50000);
  expect(splashWordmarkCrop).toEqual({ left, top, width: right - left + 1, height: bottom - top + 1 });
  const native = NativeImage.decode(bytes);
  try { expect(Buffer.compare(Buffer.from(native.raw().data), png.data)).toBe(0); }
  finally { native.dispose(); }
});

// Expand native spans into individual cells. Glyph-only checks accepted black or
// fully transparent blocks before; every ink sample must also have visible fg.
function cells(screen: Awaited<ReturnType<typeof setup>>) {
  return screen.captureSpans().lines.slice(0, screen.wordmark.height).map(line =>
    line.spans.flatMap(span => [...span.text].map(char => ({ char, fg: span.fg, bg: span.bg }))));
}

test("paints the PNG silhouette as visible colored cells, leaving its holes on the parent background", async () => {
  const screen = await setup();
  const frame = cells(screen);
  // Fixed asset locations, including the detached dot, inter-letter gaps, and
  // lower stems. An all-black or all-filled rectangle cannot satisfy this.
  expect(screen.captureCharFrame().split("\n").slice(0, 5)).toEqual([
    "                                    ▄██                         ",
    "▄▄▄▄▄▄▄▄▄   ▄▄▄▄▄▄▄    ▄▄▄▄▄▄▄     ▄▄▄▄▄    ▄▄▄▄▄▄▄▄  ▄▄▄▄▄▄▄▄  ",
    "███▀▀▀▀███▄██████████▄███▀▀▀███▄   ▀████   ████████  ███████████",
    "███    █████████████▀████   ███▀    ████    ████████▄███████████",
    "███    ███ ▀███████  ▀▀███████▀  ▄████████ ████████▀ ██▀███▀███▀",
  ]);
  let ink = 0, holes = 0;
  for (const row of frame) for (const cell of row) {
    expect(cell.bg.equals(RGBA.fromHex("#182030"))).toBe(true);
    if (cell.char !== " ") {
      expect(cell.char).toMatch(/^[▀▄█]$/);
      expect(cell.fg.equals(RGBA.fromHex("#ffffff"))).toBe(true);
      ink++;
    } else holes++;
  }
  expect(ink).toBeGreaterThan(150);
  expect(holes).toBeGreaterThan(30);
});

test("ignores false-positive and late graphics claims without modifying capabilities", async () => {
  const screen = await setup();
  const before = screen.captureSpans();
  for (const terminal of ["ghostty", "neoism", "kitty", "unknown"]) {
    setRendererCapabilities(screen.renderer, { kitty_graphics: true, sixel: true,
      terminal: { name: terminal, version: "", from_xtversion: false } });
    await screen.flush();
    expect(screen.renderer.capabilities?.kitty_graphics).toBe(true);
    expect(screen.captureSpans()).toEqual(before);
  }
  setRendererCapabilities(screen.renderer, { kitty_graphics: false, sixel: false, multiplexer: "tmux" });
  await screen.flush();
  expect(screen.captureSpans()).toEqual(before);
});

test("fg is live and visible on light themes too, without a black image background", async () => {
  const screen = await setup("#102030", "#fafafa");
  for (const fg of ["#102030", "#804020"]) {
    screen.setInk(fg); await screen.flush();
    const ink = cells(screen).flat().filter(cell => cell.char !== " ");
    expect(ink.length).toBeGreaterThan(150);
    expect(ink.every(cell => cell.fg.equals(RGBA.fromHex(fg)) && cell.bg.equals(RGBA.fromHex("#fafafa")))).toBe(true);
  }
});

test("fits the artwork aspect, reflows on resize and late pixel reports, and cleans up listeners", async () => {
  const screen = await setup();
  expect([screen.wordmark.width, screen.wordmark.height]).toEqual([64, 5]);
  screen.resize(32, 20); await screen.flush();
  expect([screen.wordmark.width, screen.wordmark.height]).toEqual([32, 3]);
  Object.defineProperty(screen.renderer, "resolution", { configurable: true, get: () => ({ width: 320, height: 200 }) });
  screen.renderer.emit(CliRenderEvents.FRAME);
  await screen.flush();
  expect([screen.wordmark.width, screen.wordmark.height]).toEqual([32, 5]);
  expect(cells(screen).flat().some(cell => cell.char !== " " && cell.fg.equals(RGBA.fromHex("#ffffff")))).toBe(true);
  Object.defineProperty(screen.renderer, "resolution", { configurable: true, get: () => null });
  screen.resize(1, 20); await screen.flush();
  expect([screen.wordmark.width, screen.wordmark.height]).toEqual([1, 1]);
  screen.resize(120, 20); await screen.flush();
  expect(screen.wordmark.width).toBe(64);
  const count = screen.renderer.listenerCount(CliRenderEvents.FRAME);
  await screen.dispose(); disposals.pop();
  expect(screen.renderer.listenerCount(CliRenderEvents.FRAME)).toBeLessThan(count);
});

test("real native serializer sends colored wordmark cells, not just load-ready or graphics uploads", async () => {
  let output = "";
  const sink = new Writable({ write(chunk, _encoding, done) { output += chunk.toString(); done(); } });
  const renderer = await createCliRenderer({ width: 64, height: 10,
    stdout: sink as unknown as NodeJS.WriteStream, consoleMode: "disabled", useMouse: false,
    useKittyKeyboard: null, exitSignals: [], environment: { TERM: "xterm-256color" }, backgroundColor: "#182030" });
  disposals.push(async () => { renderer.destroy(); await renderer.closed; });
  await render(() => <SplashWordmark columns={64} fg="#ffffff" />, renderer);
  // Start after the ordinary terminal queries; this assertion concerns paint.
  output = "";
  renderer.start();
  for (let i = 0; i < 100 && !/[▀▄█]/.test(output); i++) await Bun.sleep(10);
  expect(output).toMatch(/[▀▄█]/);
  expect(output).toMatch(/\x1b\[38;(?:5;15|2;255;255;255)m/);
  expect(output).not.toContain("\x1b_G");
  expect(output).not.toContain("\x1bPq");
});