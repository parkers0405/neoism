import { existsSync, readFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve, extname } from "node:path";
import { parse, type ParseError } from "jsonc-parser";
import { palettes, type Palette } from "./design/generated";
import { cursorBlinkInterval, DEFAULT_CURSOR_BLINK_INTERVAL_MS } from "./ui/cursor-blink";

export interface AppConfig {
  endpoint: string;
  authToken?: string;
  pairingToken?: string;
  workspaceId?: string;
  theme: Palette;
  themeName: string;
  fontFamily: string;
  fontSize: number;
  fontWeight: number;
  markdownFontFamily: string;
  iconFontFamily: string;
  vim: boolean;
  statusFps?: boolean;
  cursorBlink: boolean;
  cursorBlinkInterval?: number;
  cursorStyle: string;
  cursorColor?: string;
  presenceDisplayName?: string;
  terminalScrollMultiplier?: number;
  terminalScrollDivider?: number;
  scale: number;
  background?: { path: string; opacity: number; blur: number };
  reducedMotion: boolean;
  splashAutoAnimate?: boolean;
  warnings: string[];
}
function object(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
const number = (value: unknown, fallback: number) => typeof value === "number" && Number.isFinite(value) ? value : fallback;
const string = (value: unknown, fallback: string) => typeof value === "string" && value.trim() ? value : fallback;
const readOptional = (path: string) => { try { return readFileSync(path, "utf8").trim() || undefined; } catch { return undefined; } };

export function loadConfig(args = process.argv.slice(2), env = process.env, values?: Record<string, unknown>): AppConfig {
  const value = (flag: string) => { const i = args.indexOf(flag); return i >= 0 ? args[i + 1] : undefined; };
  const configRoot = join(env.XDG_CONFIG_HOME || join(homedir(), ".config"), "neoism");
  const configPath = value("--config") || join(configRoot, "config.json");
  const warnings: string[] = [];
  let raw: Record<string, unknown> = {};
  if (values) raw = object(values);
  else if (existsSync(configPath)) {
    const errors: ParseError[] = [];
    raw = object(parse(readFileSync(configPath, "utf8"), errors, { allowTrailingComma: true }));
    if (errors.length) { warnings.push("Neoism config contains invalid JSONC; using appearance defaults."); raw = {}; }
  }
  const appearance = object(raw.appearance);
  const fonts = object(appearance.fonts);
  const windowConfig = object(object(raw.ui).window ?? raw.window);
  const image = object(windowConfig["background-image"]);
  const themeName = value("--theme") || string(appearance.theme, "pastel_dark");
  const theme = { ...(palettes[themeName as keyof typeof palettes] || palettes.pastel_dark) } as Palette;
  const fontSize = Math.max(7, Math.min(42, number(Number(value("--font-size") || fonts.size || 14), 14)));
  const runtimeDir = env.XDG_RUNTIME_DIR || join(tmpdir(), `neoism-${process.getuid?.() ?? "user"}`);
  const socketPath = join(runtimeDir, "neoism.sock");
  const explicit = value("--daemon") || env.NEOISM_GPUIX_DAEMON_URL;
  const endpoint = explicit || (process.platform !== "win32" && existsSync(socketPath)
    ? `ws+unix://${socketPath}:/session` : "ws://127.0.0.1:7878/session");
  // Only discover local upgrade credentials for our own default endpoint.
  const authToken = env.NEOISM_DAEMON_TOKEN || (!explicit ? readOptional(join(runtimeDir, "neoism", "daemon-token")) : undefined);
  if (!palettes[themeName as keyof typeof palettes]) warnings.push(`Theme '${themeName}' is not bundled; using pastel_dark.`);
  let backgroundPath = typeof image.path === "string" ? resolve(image.path.replace(/^~(?=\/)/, homedir())) : undefined;
  const blur = Math.max(0, number(image.blur, 0));
  // Sugarloaf already caches blurred backgrounds beside the original. Reuse
  // those exact pixels when available without modifying any personal asset.
  if (backgroundPath && blur > 0) {
    const cached = backgroundPath.slice(0, -extname(backgroundPath).length) + `-blur-${blur}.png`;
    if (existsSync(cached)) backgroundPath = cached;
    else warnings.push("Background blur has no cached image; showing the original image for now.");
  }
  return {
    endpoint, authToken, pairingToken: env.NEOISM_PAIRING_TOKEN,
    workspaceId: value("--workspace"), theme, themeName,
    fontFamily: string(fonts.family, "Cascadia Code NF"), fontSize,
    markdownFontFamily: string(object(object(raw.look).markdown)["font-family"], string(fonts.family, "Cascadia Code NF")),
    iconFontFamily: Array.isArray(fonts["symbol-map"]) ? string(object(fonts["symbol-map"][0])["font-family"], "Cascadia Code NF") : "Cascadia Code NF",
    vim: object(raw.editor)["vim-mode"] === true,
    statusFps: object(raw.ui)["status-fps"] !== false,
    cursorBlink: object(object(raw.terminal).cursor).blinking !== false,
    cursorBlinkInterval: cursorBlinkInterval(number(object(object(raw.terminal).cursor)["blinking-interval"], DEFAULT_CURSOR_BLINK_INTERVAL_MS)),
    cursorStyle: string(object(raw.presence)["cursor-style"], "solid"),
    cursorColor: typeof object(raw.presence)["cursor-color"] === "string" ? object(raw.presence)["cursor-color"] as string : undefined,
    presenceDisplayName: typeof object(raw.presence)["display-name"] === "string" ? object(raw.presence)["display-name"] as string : undefined,
    terminalScrollMultiplier: Math.max(0, number(object(object(raw.terminal).scroll).multiplier, 3)),
    terminalScrollDivider: Math.max(0.01, number(object(object(raw.terminal).scroll).divider, 1)),
    fontWeight: number(fonts.weight, 400), scale: Math.max(0.5, Math.min(3, fontSize / 14)),
    background: backgroundPath && !windowConfig["background-image-disabled"]
      ? { path: backgroundPath, opacity: Math.max(0, Math.min(1, number(image.opacity, 0.35))), blur } : undefined,
    reducedMotion: args.includes("--reduced-motion") || env.NEOISM_REDUCED_MOTION === "1",
    splashAutoAnimate: appearance["splash-auto-animate"] === true,
    warnings,
  };
}

export function publicEndpoint(endpoint: string): string {
  try {
    const url = new URL(endpoint);
    url.username = ""; url.password = ""; url.search = ""; url.hash = "";
    return url.toString();
  } catch { return "daemon"; }
}
