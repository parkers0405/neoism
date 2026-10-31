import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadConfig } from "../src/config";
import { easeOutCubic, revealProgress } from "../src/design/animation";
const dirs: string[] = [];
afterEach(() => dirs.splice(0).forEach((path) => rmSync(path, { recursive: true, force: true })));
test("reads actual Neoism JSONC appearance and ui.window paths without writing config", () => {
  const dir = mkdtempSync(join(tmpdir(), "neoism-gpuix-config-")); dirs.push(dir);
  const path = join(dir, "config.json");
  writeFileSync(path, `{
    // same JSONC format as desktop
    "appearance": {"theme":"tokyo_night","fonts":{"family":"Geist Mono","size":20,"weight":700,"symbol-map":[{"font-family":"JetBrainsMono Nerd Font"}]}},
    "look":{"markdown":{"font-family":"Monocraft"}},
    "terminal":{"scroll":{"multiplier":4,"divider":2}},
    "ui":{"window":{"background-image":{"path":${JSON.stringify(join(dir,"image.png"))},"opacity":0.35}}},
  }`);
  const config = loadConfig(["--config", path], {});
  expect(config.fontSize).toBe(20); expect(config.scale).toBeCloseTo(20 / 14);
  expect(config.fontFamily).toBe("Geist Mono"); expect(config.fontWeight).toBe(700);
  expect(config.iconFontFamily).toBe("JetBrainsMono Nerd Font"); expect(config.markdownFontFamily).toBe("Monocraft");
  expect(config.theme.bg).toBe("#1a1b26"); expect(config.background?.opacity).toBe(0.35);
  expect(config.terminalScrollMultiplier).toBe(4); expect(config.terminalScrollDivider).toBe(2);
});
test("does not automatically send a local secret to an explicit remote endpoint", () => {
  const config = loadConfig(["--config", "/nonexistent/neoism-gpuix-test", "--daemon", "wss://example.test/session"], {});
  expect(config.authToken).toBeUndefined(); expect(config.pairingToken).toBeUndefined();
});
test("cursor blink reads the original local half-period and desktop bounds", () => {
  expect(loadConfig([], {}, {}).cursorBlinkInterval).toBe(530);
  const cursor = (value: number) => loadConfig([], {}, {terminal:{cursor:{"blinking-interval":value}}}).cursorBlinkInterval;
  expect(cursor(900)).toBe(900); expect(cursor(1)).toBe(350); expect(cursor(5000)).toBe(1200);
  expect(loadConfig([], {}, {terminal:{cursor:{blinking:false}}}).cursorBlink).toBe(false);
});
test("wordmark autoplay follows the original opt-in preference without changing reduced motion", () => {
  expect(loadConfig([], {}, {}).splashAutoAnimate).toBe(false);
  expect(loadConfig([], {}, { appearance: { "splash-auto-animate": true } }).splashAutoAnimate).toBe(true);
  expect(loadConfig([], {}, { appearance: { "splash-auto-animate": "on" } }).splashAutoAnimate).toBe(false);
  const reduced = loadConfig(["--reduced-motion"], {}, { appearance: { "splash-auto-animate": true } });
  expect(reduced.splashAutoAnimate).toBe(true);
  expect(reduced.reducedMotion).toBe(true);
});
test("tree reveal matches the production ease-out cubic and per-row delay", () => {
  expect(easeOutCubic(0)).toBe(0); expect(easeOutCubic(0.5)).toBe(0.875); expect(easeOutCubic(1)).toBe(1);
  expect(revealProgress(12, 1, 320, 12)).toBe(0);
  expect(revealProgress(172, 1, 320, 12)).toBe(0.875);
  expect(revealProgress(332, 1, 320, 12)).toBe(1);
});

test("composer material retains configured selector opacity without fading text or changing wallpaper", () => {
  const config = (styles: Record<string, unknown>) => loadConfig([], {}, {ui: {styles}});
  expect(config({}).composerOpacity).toBe(0.85);
  expect(config({composer: {opacity: 0.4}}).composerOpacity).toBe(0.4);
  expect(config({composer: {opacity: 0.4}, "composer.agent": {opacity: 0.7}}).composerOpacity).toBe(0.7);
  expect(config({"composer.agent": {opacity: 0}}).composerOpacity).toBe(0);
  expect(config({"composer.agent": {opacity: 1}}).composerOpacity).toBe(1);
  expect(config({"composer.agent": {opacity: 5}}).composerOpacity).toBe(1);
  expect(config({"composer.agent": {opacity: -2}}).composerOpacity).toBe(0);
  expect(config({"composer.agent": {opacity: NaN}}).composerOpacity).toBe(0.85);
});

test("custom configuration home matches the daemon and an explicit file takes precedence", () => {
  const dir = mkdtempSync(join(tmpdir(), "neoism-config-home-")); dirs.push(dir);
  writeFileSync(join(dir, "config.json"), JSON.stringify({appearance:{fonts:{size:19}}}));
  const explicit = join(dir, "override.json");
  writeFileSync(explicit, JSON.stringify({appearance:{fonts:{size:22}}}));
  expect(loadConfig([], {NEOISM_CONFIG_HOME:dir, XDG_CONFIG_HOME:"/nonexistent"}).fontSize).toBe(19);
  expect(loadConfig(["--config", explicit], {NEOISM_CONFIG_HOME:dir}).fontSize).toBe(22);
});

test("equals-form launch flags retain explicit endpoint isolation and preferences", () => {
  const config = loadConfig(["--daemon=wss://example.test/session", "--workspace=work", "--font-size=19", "--theme=tokyo_night"], {}, {});
  expect(config.endpoint).toBe("wss://example.test/session");
  expect(config.workspaceId).toBe("work");
  expect(config.fontSize).toBe(19);
  expect(config.themeName).toBe("tokyo_night");
  expect(config.authToken).toBeUndefined();
});
