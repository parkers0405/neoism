import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";
const directory = mkdtempSync(join(tmpdir(), "neoism-command-app-")), config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true },terminal:{cursor:{blinking:false}},
  keybinds: { keys: [{ key: "y", with: "control | shift", action: "OpenCommandPalette" }] } }));
const server = createAgentDaemon({ deferCreations: false });
server.files.set("command.ts", "# Neoism " + "long wrap fixture ".repeat(25) + "\nLine 2");
const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config], cwd: process.cwd(),
  env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "state.json") } });
const until = async (test: () => boolean | Promise<boolean>) => { const end = Date.now() + 10000; while (!await test()) { if (Date.now() > end) throw new Error("App command check timed out"); await Bun.sleep(20); } };
const select = async (query: string, action: string) => {
  await app.getByTestId("command-dialog").waitFor(); await app.getByTestId("command-palette").fill(query);
  await app.getByTestId(`command:${action}`).waitFor(); await app.getByTestId(`command:${action}`).click();
};
const rapidPalette = async (testId: string) => {
  const surface=await app.getByTestId(testId).waitFor();await app.call("focus",{elementId:surface.id});
  // One native burst, with no wait for a dialog, frame, or focus event.
  await app.call("keystrokes",{keys:"alt-p q"});
  await app.getByTestId("command:CloseTab").waitFor();
  await app.getByTestId("command-palette").press("escape");
};
let stage = "composer custom binding";
try {
  await app.getByTestId("file:command.ts").waitFor({ timeoutMs: 15000 });
  await app.call("keystrokes",{keys:"alt-a"});
  const composer = app.getByTestId("composer"); await composer.waitFor();await rapidPalette("composer"); await composer.click(); await composer.press("ctrl-shift-y");
  await select("New Tab", "TabCreate"); await app.getByTestId("terminal").waitFor();
  stage = "terminal custom binding"; await app.getByTestId("terminal").press("ctrl-shift-y"); await app.getByTestId("command-dialog").waitFor();
  await app.getByTestId("command-palette").press("escape");
  await rapidPalette("terminal");
  stage = "editor custom binding"; await app.getByTestId("file:command.ts").waitFor(); await app.getByTestId("file:command.ts").click();
  const editor = app.getByTestId("code-editor"); await editor.waitFor(); await editor.press("ctrl-shift-y");
  await app.getByTestId("command-palette").press("escape");
  await rapidPalette("code-editor");
  // Text is rasterized now; verify wrapping through the real caret geometry.
  await editor.press("G");await app.getByTestId("code-editor-caret").waitFor();const wrapped=await app.getByTestId("code-editor-caret").bounds();
  await editor.press("ctrl-shift-y"); await select("Word Wrap", "ToggleWordWrap");
  await until(async () => !!await app.getByTestId("code-editor-caret").count()&&(await app.getByTestId("code-editor-caret").bounds()).y<wrapped.y-10);
  stage = "editor command save"; await editor.press("g");await editor.press("g");await editor.press("i"); await editor.press("Z"); await editor.press("escape");
  await editor.press("ctrl-shift-y"); await select("Write File", "SaveDocument");
  await until(() => server.fileRequests.some((r) => r.tag === "WriteFile"));
  if (!server.files.get("command.ts")?.startsWith("Z# Neoism")) throw new Error("Command save lost the edit or typed the custom binding into the buffer");
  console.log("Full native app: custom bindings work in composer, Ghostty terminal, and code editor; real command dispatch opens a terminal, toggles real word wrap, and saves edited fixture bytes.");
} catch (error) { console.error(`Full app command check failed at ${stage}`); throw error; }
finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
