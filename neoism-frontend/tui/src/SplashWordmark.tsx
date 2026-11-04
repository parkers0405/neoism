import { CliRenderEvents } from "@opentui/core";
import { useRenderer } from "@opentui/solid";
import { readFileSync } from "node:fs";
import { PNG } from "pngjs";
import { For, createMemo, createSignal, onCleanup } from "solid-js";

export const splashWordmarkSource = new URL("../../shared/assets/splash/neoism-wordmark.png", import.meta.url);
// The shared GUI PNG is already trimmed to its alpha bounding box (tested).
export const splashWordmarkCrop = { left: 0, top: 0, width: 1196, height: 193 };

let artwork: PNG | undefined;

/** Area-sample the PNG's alpha, not a font or a hand-drawn approximation.
 * The source is white on transparent; fg supplies its theme-aware ink.
 * Quantizing coverage keeps empty halves transparent to the parent's actual
 * background (no assumed black bg and no alpha-bearing terminal colors). */
function rasterize(columns: number, rows: number): string[] {
  const png = artwork ??= PNG.sync.read(readFileSync(splashWordmarkSource));
  const samples: boolean[][] = [];
  for (let y = 0; y < rows * 2; y++) {
    const top = y * png.height / (rows * 2), bottom = (y + 1) * png.height / (rows * 2);
    const line: boolean[] = [];
    for (let x = 0; x < columns; x++) {
      const left = x * png.width / columns, right = (x + 1) * png.width / columns;
      let alpha = 0;
      for (let py = Math.floor(top); py < Math.ceil(bottom); py++) {
        const dy = Math.min(bottom, py + 1) - Math.max(top, py);
        for (let px = Math.floor(left); px < Math.ceil(right); px++) {
          const dx = Math.min(right, px + 1) - Math.max(left, px);
          alpha += png.data[(py * png.width + px) * 4 + 3]! / 255 * dx * dy;
        }
      }
      line.push(alpha / ((right - left) * (bottom - top)) >= 0.35);
    }
    samples.push(line);
  }
  return Array.from({ length: rows }, (_, y) => Array.from({ length: columns }, (_, x) =>
    samples[y * 2]![x] ? samples[y * 2 + 1]![x] ? "█" : "▀" : samples[y * 2 + 1]![x] ? "▄" : " "
  ).join(""));
}

/** Always use ordinary, explicitly colored ANSI cells. Native block glyphs
 * alone do not prove paint (transparent PNG background emits blocks too). Auto
 * trusts graphics claims, not successful presentation: Neoism's embedded
 * Ghostty answers the Kitty query with OK but its frontend paints only cells,
 * not image placements. Neither load-ready nor a successful Kitty query
 * proves visible paint, so this splash deliberately never upgrades transports,
 * even on late capability replies. No global capability flags are falsified. */
export function SplashWordmark(props: { columns: number; fg: string }) {
  const renderer = useRenderer();
  const readAspect = () => {
    const pixels = renderer.resolution;
    if (!pixels || pixels.width <= 0 || pixels.height <= 0 || renderer.terminalWidth <= 0 || renderer.terminalHeight <= 0) return 2;
    return (pixels.height / renderer.terminalHeight) / (pixels.width / renderer.terminalWidth);
  };
  const [aspect, setAspect] = createSignal(readAspect());
  const refresh = () => setAspect(readAspect());
  // Resolution replies request a frame but do not emit RESIZE in OpenTUI 0.8.2.
  // FRAME catches those late replies without polling or starting an animation.
  const events = [CliRenderEvents.RESIZE, CliRenderEvents.CAPABILITIES, CliRenderEvents.FRAME];
  for (const event of events) renderer.on(event, refresh);
  onCleanup(() => { for (const event of events) renderer.off(event, refresh); });
  const width = createMemo(() => Math.max(1, Math.min(64, Math.floor(Number.isFinite(props.columns) ? props.columns : 1))));
  const height = createMemo(() => Math.max(1, Math.round(width() * splashWordmarkCrop.height / splashWordmarkCrop.width / aspect())));
  const lines = createMemo(() => rasterize(width(), height()));

  return <box id="splash-wordmark" width={width()} height={height()} flexShrink={0} flexDirection="column">
    <For each={lines()}>{line => <text width={width()} height={1} flexShrink={0}
      fg={props.fg} selectable={false} wrapMode="none">{line}</text>}</For>
  </box>;
}