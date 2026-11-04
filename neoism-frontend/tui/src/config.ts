import { existsSync, readFileSync, mkdirSync, writeFileSync, renameSync, unlinkSync, statSync, watchFile, unwatchFile } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { parse, modify, applyEdits, type ParseError } from "jsonc-parser";
import { palettes, type Palette } from "../../gpuix/src/design/generated";

export interface TuiConfig {
  endpoint: string;
  authToken?: string;
  pairingToken?: string;
  workspaceId?: string;
  directory?: string;
  theme: Palette;
  themeName: string;
  offline: boolean;
  vim: boolean;
  configPath: string;
  keybinds: unknown;
  warnings: string[];
}

export const help = `Neoism terminal frontend (SolidJS + OpenTUI)

  bun start [DIRECTORY] [--daemon URL] [--workspace ID] [--theme NAME] [--offline]

Attaches to an existing workspace daemon; does not start or modify one.
Uses Neoism's native Rust editor. NEOISM_GPUIX_CORE can override its library path.

  --daemon URL     Explicit ws://, wss://, or ws+unix:// endpoint
  --workspace ID   Select an existing workspace by ID
  DIRECTORY        Open/create a workspace at this daemon-host directory
  --config PATH    Read appearance from a Neoism JSONC config
  --theme NAME     Use a bundled Neoism theme
  --offline        Show the splash without connecting

Credentials: NEOISM_DAEMON_TOKEN and NEOISM_PAIRING_TOKEN.
Explicit remote endpoints never inherit a discovered local token.
`;

export function loadTuiConfig(args = process.argv.slice(2), env = process.env): TuiConfig {
  const flags = new Map<string, string>();
  let offline = false;
  let directory: string | undefined;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (arg === "--offline") { offline = true; continue; }
    if (!arg.startsWith("-")) { if (directory) throw new Error("Only one workspace directory may be supplied."); directory = arg; continue; }
    const [name, ...rest] = arg.split("=");
    if (!["--daemon", "--workspace", "--config", "--theme"].includes(name!)) throw new Error(`Unknown argument: ${arg}`);
    const value = rest.length ? rest.join("=") : args[++i];
    if (!value || value.startsWith("--")) throw new Error(`Missing value for ${name}`);
    flags.set(name!, value);
  }
  const warnings: string[] = [];
  const root = env.NEOISM_CONFIG_HOME || join(env.XDG_CONFIG_HOME || join(homedir(), ".config"), "neoism");
  const configPath = flags.get("--config") || join(root, "config.json");
  let configuredTheme: unknown;
  let vim = true;
  let keybinds: unknown = [];
  if (existsSync(configPath)) {
    const errors: ParseError[] = [];
    const raw = parse(readFileSync(configPath, "utf8"), errors, { allowTrailingComma: true });
    if (errors.length) warnings.push("Invalid JSONC config; using appearance defaults.");
    else { configuredTheme = raw?.appearance?.theme; vim = raw?.editor?.["vim-mode"] !== false; keybinds = raw?.keybinds?.keys || []; }
  }
  const themeName = flags.get("--theme") || (typeof configuredTheme === "string" ? configuredTheme : "pastel_dark");
  const theme = Object.hasOwn(palettes, themeName) ? palettes[themeName as keyof typeof palettes] : palettes.pastel_dark;
  if (!Object.hasOwn(palettes, themeName)) warnings.push(`Theme '${themeName}' is not bundled; using pastel_dark.`);
  const runtime = env.XDG_RUNTIME_DIR || join(tmpdir(), `neoism-${process.getuid?.() ?? "user"}`);
  const socket = env.NEOISM_DAEMON_SOCKET || (env.XDG_RUNTIME_DIR ? join(runtime, "neoism.sock") : join(tmpdir(), `neoism-${process.getuid?.() ?? "user"}.sock`));
  const explicit = flags.get("--daemon") || env.NEOISM_TUI_DAEMON_URL;
  const endpoint = explicit || (process.platform !== "win32" && existsSync(socket)
    ? `ws+unix://${socket}:/session` : `ws://${env.NEOISM_DAEMON_ADDR || "127.0.0.1:7878"}/session`);
  if (!/^(wss?:\/\/|ws\+unix:\/\/)/.test(endpoint)) throw new Error("Daemon endpoint must use ws://, wss://, or ws+unix://");
  if (directory && flags.has("--workspace")) throw new Error("Choose a directory or --workspace ID, not both.");
  if (directory && !/^(\/|[A-Za-z]:[\\/]|\\\\)/.test(directory)) {
    const local = endpoint.startsWith("ws+unix:") || ["localhost", "127.0.0.1", "[::1]", "::1"].includes(new URL(endpoint).hostname);
    if (!local) throw new Error("Remote workspace directories must be absolute paths on the daemon host.");
    directory = resolve(env.NEOISM_LAUNCH_DIRECTORY || process.cwd(), directory);
  }
  let authToken = env.NEOISM_DAEMON_TOKEN;
  if (!authToken && !explicit) {
    try { authToken = readFileSync(env.XDG_RUNTIME_DIR ? join(runtime, "neoism", "daemon-token") : join(runtime, "daemon-token"), "utf8").trim() || undefined; }
    catch { /* A daemon without an upgrade token is valid. */ }
  }
  return { endpoint, authToken, pairingToken: env.NEOISM_PAIRING_TOKEN, workspaceId: flags.get("--workspace"), directory, theme, themeName, offline, vim, configPath, keybinds, warnings };
}

export function persistTheme(config: Pick<TuiConfig, "configPath">, name: string) {
  if (!Object.hasOwn(palettes, name)) throw new Error(`Unknown theme: ${name}`);
  persistConfigValue(config, ["appearance", "theme"], name);
}

export function persistConfigValue(config: Pick<TuiConfig, "configPath">, path: string[], value: unknown) {
  const source = existsSync(config.configPath) ? readFileSync(config.configPath, "utf8") : "{}\n";
  const errors: ParseError[] = [];
  const parsed = parse(source, errors, { allowTrailingComma: true });
  if (errors.length || !parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("Cannot update an invalid JSONC config.");
  const result = applyEdits(source, modify(source, path, value, { formattingOptions: { insertSpaces: true, tabSize: 2 } }));
  mkdirSync(dirname(config.configPath), { recursive: true });
  const temporary = `${config.configPath}.tui-${crypto.randomUUID()}.tmp`;
  try {
    writeFileSync(temporary, result, { flag: "wx", mode: existsSync(config.configPath) ? statSync(config.configPath).mode & 0o777 : 0o600 });
    renameSync(temporary, config.configPath);
  } finally { try { unlinkSync(temporary); } catch {} }
}

export function watchSharedConfig(config: TuiConfig, changed: (next: TuiConfig) => void, error: (error: unknown) => void) {
  const listener = () => {
    try {
      const next = loadTuiConfig(["--config", config.configPath, "--daemon", config.endpoint]);
      if (next.warnings.length) throw new Error(next.warnings.join(" "));
      changed(next);
    } catch (cause) { error(cause); }
  };
  watchFile(config.configPath, { interval: 500, persistent: false }, listener);
  return () => unwatchFile(config.configPath, listener);
}