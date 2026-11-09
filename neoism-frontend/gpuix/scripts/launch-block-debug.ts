import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { applyEdits, modify, parse, type ParseError } from "jsonc-parser";
import { frontendRoot, verifiedDisplayBinding } from "../src/display-binding";

// A real app and real local services, with separate recovery/runtime paths.
// Only this window's copied appearance config enables the alternate terminal.
const production = process.argv.includes("--production");
const keepTerminalLayout = process.argv.includes("--keep-terminal-layout");
if (!production && process.env.NEOISM_GPUIX_DISPLAY_DEBUG !== "1") throw new Error("Set NEOISM_GPUIX_DISPLAY_DEBUG=1 before starting Bun.");
if (production && process.env.NEOISM_GPUIX_DISPLAY_DEBUG === "1") throw new Error("Unset NEOISM_GPUIX_DISPLAY_DEBUG before launching production.");
const display = verifiedDisplayBinding(frontendRoot, !production);
if ((display.stamp.profile || "release") !== (production ? "release" : "debug")) throw new Error("The display artifact has the wrong build profile.");
const repository = resolve(frontendRoot, "../.."), workspace = resolve(process.argv.slice(2).find(arg => arg !== "--production" && arg !== "--keep-terminal-layout") || repository);
const suffix = process.platform === "win32" ? ".exe" : "";
const executable = production ? join(frontendRoot, "dist", `neoism${suffix}`) : process.execPath;
const daemon = resolve(process.env.NEOISM_GPUIX_WORKSPACE_DAEMON || (production ? join(frontendRoot, "dist", `neoism-workspace-daemon${suffix}`) : join(repository, `target/debug/neoism-workspace-daemon${suffix}`)));
const core = resolve(process.env.NEOISM_GPUIX_CORE || join(frontendRoot, production ? "dist/web/native" : "native/target/debug", process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : process.platform === "win32" ? "neoism_gpuix_core.dll" : "libneoism_gpuix_core.so"));
if (!existsSync(executable) || !existsSync(daemon) || !existsSync(core)) throw new Error(`Build the ${production ? "production bundle" : "debug workspace daemon and native core"} before launching.`);
if (production && !existsSync(join(frontendRoot, "dist", `neoism-agent${suffix}`))) throw new Error("Stage the production services before launching.");
const env = { ...process.env };
const profile = !production && env.NEOISM_GPUIX_DEBUG_PROFILE ? resolve(env.NEOISM_GPUIX_DEBUG_PROFILE) : mkdtempSync(join(tmpdir(), production ? "neoism-local-production-" : "neoism-block-debug-"));
mkdirSync(profile, { recursive: true, mode: 0o700 });
const configRoot = env.NEOISM_CONFIG_HOME || join(env.XDG_CONFIG_HOME || join(homedir(), ".config"), "neoism");
const original = join(configRoot, "config.json"), configPath = join(profile, "config.json");
let config = existsSync(configPath) ? readFileSync(configPath, "utf8") : existsSync(original) ? readFileSync(original, "utf8") : "{}";
const errors: ParseError[] = [], current = parse(config, errors, { allowTrailingComma: true });
if (errors.length) throw new Error("The normal config has JSONC errors; it was left untouched.");
const position = current?.terminal?.["input-position"];
for (const [key, value] of keepTerminalLayout ? [] : [["block-ui", true], ["input-position", position === "top" || position === "flow" ? position : "bottom"]] as const) {
  config = applyEdits(config, modify(config, ["terminal", key], value, { formattingOptions: { insertSpaces: true, tabSize: 2 } }));
}
writeFileSync(configPath, config, { mode: 0o600 });
if (process.platform === "linux" && !env.WAYLAND_DISPLAY && !env.DISPLAY) {
  const result = spawnSync("hyprctl", ["instances", "-j"], { encoding: "utf8", timeout: 2000 });
  if (result.status === 0) {
    const instances = JSON.parse(result.stdout);
    if (instances.length === 1) { env.WAYLAND_DISPLAY = join(env.XDG_RUNTIME_DIR || `/run/user/${process.getuid?.()}`, instances[0].wl_socket); env.HYPRLAND_INSTANCE_SIGNATURE = instances[0].instance; }
  }
  if (!env.WAYLAND_DISPLAY) throw new Error("No desktop display is available.");
}
if (env.WAYLAND_DISPLAY && !isAbsolute(env.WAYLAND_DISPLAY)) env.WAYLAND_DISPLAY = join(env.XDG_RUNTIME_DIR || `/run/user/${process.getuid?.()}`, env.WAYLAND_DISPLAY);
if (!env.XAUTHORITY && existsSync(join(homedir(), ".Xauthority"))) env.XAUTHORITY = join(homedir(), ".Xauthority");
const freePort = () => new Promise<number>((resolve, reject) => {
  const server = createServer(); server.once("error", reject);
  server.listen(0, "127.0.0.1", () => { const address = server.address(); if (!address || typeof address === "string") { server.close(); reject(new Error("No local TCP address")); return; } server.close(error => error ? reject(error) : resolve(address.port)); });
});
// Recovery profiles are keyed by endpoint, so a resumed preview keeps its ports.
const launchPath=join(profile,"launch.json");
const previous: {daemonPort?:number;agentPort?:number}=existsSync(launchPath)?JSON.parse(readFileSync(launchPath,"utf8")):{};
const daemonPort = previous.daemonPort ?? await freePort(), agentPort = previous.agentPort ?? await freePort();
if([daemonPort,agentPort].some(port=>!Number.isInteger(port)||port<1||port>65535)||daemonPort===agentPort)throw new Error("Invalid debug profile ports");
writeFileSync(launchPath,JSON.stringify({daemonPort,agentPort}),{mode:0o600});
const userState = env.XDG_STATE_HOME || join(homedir(), ".local/state");
const providerAuth = env.NEOISM_AGENT_AUTH_PATH || join(userState, "neoism-agent/auth.json");
const mcpAuth = env.NEOISM_AGENT_MCP_AUTH_PATH || join(env.NEOISM_AGENT_STATE_DIR || join(userState, "neoism"), "mcp-auth.json");
for (const key of Object.keys(env)) if (key.startsWith("NEOISM_GPUIX_NATIVE_CHECK") || key.startsWith("NEOISM_GPUIX_TEST_")) delete env[key];
for (const key of ["NEOISM_GPUIX_DAEMON_URL", "NEOISM_DAEMON_TOKEN", "NEOISM_PAIRING_TOKEN", "NEOISM_AGENT_TOKEN", "NEOISM_AGENT_AUTH_CONFIG", "NEOISM_WEB_ROOT", "NEOISM_AGENT_GUI_ROOT"]) delete env[key];
Object.assign(env, {
  RUST_MIN_STACK: "67108864", NEOISM_GPUIX_DISPLAY_DEBUG: production ? "0" : "1", NEOISM_GPUIX_CORE: core,
  XDG_RUNTIME_DIR: profile, XDG_STATE_HOME: join(profile, "state"), XDG_DATA_HOME: join(profile, "data"), NEOISM_CONFIG_HOME: profile, NEOISM_CONFIG_DIR: profile,
  NEOISM_NOTES_HOME: join(profile, "notes"),
  NEOISM_GPUIX_PRIVATE_DAEMON: "1",
  NEOISM_STATE_DIR: join(profile, "state"), NEOISM_GPUIX_STATE_PATH: join(profile, "session.json"),
  NEOISM_DAEMON_DATA_DIR: join(profile, "daemon"), NEOISM_AGENT_STATE_DIR: join(profile, "agent"),
  NEOISM_WORKSPACE_REGISTRY: join(profile, "registry.json"), NEOISM_DAEMON_SOCKET: join(profile, "neoism.sock"),
  NEOISM_DAEMON_ADDR: `127.0.0.1:${daemonPort}`, NEOISM_AGENT_SERVER: `http://127.0.0.1:${agentPort}`,
  NEOISM_GPUIX_WORKSPACE_DAEMON: daemon, NEOISM_LAUNCH_DIRECTORY: workspace, NEOISM_WORKSPACE_ROOT: workspace,
  NEOISM_AGENT_AUTH_PATH: providerAuth, NEOISM_AGENT_MCP_AUTH_PATH: mcpAuth,
});
const log = join(profile, "app.log"), fd = openSync(log, "a", 0o600);
let pid: number | undefined;
try {
  const child = spawn(executable, [...(production ? [] : [join(frontendRoot, "src/main.tsx")]), "--config", configPath, "--maximized"], { cwd: frontendRoot, env, detached: true, stdio: ["ignore", fd, fd] });
  await new Promise<void>((resolve, reject) => { child.once("spawn", resolve); child.once("error", reject); });
  pid = child.pid; child.unref();
} finally { closeSync(fd); }
console.log(JSON.stringify({ pid, production, executable, workspace, profile, configPath, log, core, display: display.binding, daemon, agentUrl: env.NEOISM_AGENT_SERVER }));