import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { parse } from "jsonc-parser";
import type { TreeNode } from "@gpuix/solid/automation";
import { launchNative } from "../tests/support/native-launch";
import { coreCall } from "../src/native-core";
import type { MenuFrame } from "../src/menu-actions/ChromeActionsController";
import { createAgentDaemon } from "../tests/support/agent-daemon";

const directory = mkdtempSync(join(tmpdir(), "neoism-menu-check-")), configPath = join(directory, "config.json");
const configuration = { appearance: { theme: "pastel_dark", fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } };
writeFileSync(configPath, JSON.stringify(configuration));
const server = createAgentDaemon({ deferCreations: false, configuration });
const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", configPath, "--reduced-motion"], cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "session.json") } });
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap((node) => [node, ...flatten(node.children || [])]);
const until = async (predicate: () => boolean | Promise<boolean>) => { const end = Date.now() + 12000; while (!await predicate()) { if (Date.now() > end) throw new Error("Native menu state did not converge"); await Bun.sleep(20); } };
const overlay = app.getByTestId("chrome-action-overlay");
let reference: number | undefined;
const menu = async (action: string) => { await app.getByTestId("topbar-menu").click(); await app.getByTestId(`menu:${action}`).waitFor(); await app.getByTestId(`menu:${action}`).click(); await overlay.waitFor();
  if (reference !== undefined) { coreCall("menu.dispose", { id: reference }); reference = undefined; }
  if (action === "OpenSettings") { const bounds = await overlay.bounds(); reference = coreCall<{ id: number }>("menu.new", { kind: "settings", ...bounds, values: parse(server.configDocument().content), descriptors: server.descriptors }).id; }
};
const clickNode = async (node: TreeNode) => { if (!node.bounds) throw new Error("Original painted target has no native bounds"); await app.mouse.click({ x: node.bounds.x + node.bounds.width / 2, y: node.bounds.y + node.bounds.height / 2 }); };
// Native automation intentionally strips customProps (including img.alt).
// Use the SAME original CPU painter to locate its icon, then click the real
// native app and verify the resulting transport effect; no TS geometry copy.
const referenceFrame = () => coreCall<MenuFrame>("menu.frame", { id: reference });
const glyph = async (label: string): Promise<TreeNode> => {
  const command = referenceFrame().commands.find((op) => op.kind === "text" && op.text === label);
  if (!command || command.kind !== "text") throw new Error(`Original painter did not expose ${label}`);
  return { id: -1, type: "paint-target", bounds: { x: command.x, y: command.y, width: command.width, height: command.size } };
};

let stage = "About";
try {
  await app.getByTestId("file-tree").waitFor({ timeoutMs: 15000 });
  await menu("OpenAbout"); await until(async () => (await overlay.getByType("canvas").all()).some(node => (node.bounds?.width || 0) > 1)); await overlay.press("enter"); await until(async () => await overlay.count() === 0);
  stage = "Settings actual Config write and same-window reopen";
  await menu("OpenSettings");
  let toggle: TreeNode | undefined;
  const findToggle = () => { const command = referenceFrame().commands.find((op) => op.kind === "rect" && Math.abs(op.rect[2] - 40) < .1 && Math.abs(op.rect[3] - 22) < .1); if (command?.kind === "rect") toggle = { id: -1, type: "paint-target", bounds: { x: command.rect[0], y: command.rect[1], width: command.rect[2], height: command.rect[3] } }; return !!toggle; };
  if (!findToggle()) { const general = await glyph("General"); await clickNode(general); coreCall("menu.click", { id: reference, x: general.bounds!.x + general.bounds!.width / 2, y: general.bounds!.y + general.bounds!.height / 2 }); }
  if (!findToggle()) throw new Error("Original Settings toggle is absent");
  const previousOverlay = (await overlay.element()).id; await clickNode(toggle!);
  await until(() => server.configRequests.some((request) => request.tag === "SetSetting"));
  await until(async () => await overlay.count() === 1 && (await overlay.element()).id !== previousOverlay);
  if (parse(server.configDocument().content).editor["vim-mode"] !== false) throw new Error("Settings toggle did not persist its real Config.SetSetting");
  if (await app.getByTestId("original-topbar").count() !== 1) throw new Error("Appearance reload replaced or duplicated the window root");
  await overlay.press("escape"); await until(async () => await overlay.count() === 0);
  stage = "Themes revision-checked write";
  await menu("OpenThemes"); for (const key of "tokyo") await overlay.press(key); await overlay.press("enter");
  await until(() => parse(server.configDocument().content).appearance.theme === "tokyo_night");
  await until(async () => await overlay.count() === 0);
  stage = "Config document code editor and consecutive saves";
  await menu("OpenSettings"); await clickNode(await glyph("\uf15c"));
  const editor = app.getByTestId("code-editor"); await editor.waitFor(); await until(async () => await overlay.count() === 0);
  const initial = server.configDocument().content;
  await editor.press("space"); await editor.press("ctrl-s");
  await until(() => server.configDocument().content.startsWith(" ") && server.configRequests.filter((request) => request.tag === "SaveConfigDocument").length >= 2);
  await editor.waitFor(); await editor.press("ctrl-z"); await editor.press("ctrl-s"); await until(() => server.configDocument().content === initial);
  if (server.fileRequests.some((request) => request.tag === "WriteFile" && String(request.body.path).includes("config.json"))) throw new Error("Config buffer escaped into the project Files service");
  stage = "Config conflict retains dirty buffer and host document";
  await editor.waitFor(); await editor.press("space"); const external = server.configDocument().content + "\n// external revision\n"; server.mutateConfigDocument(external);
  const before = server.configRequests.length; await editor.press("ctrl-s");
  await until(() => server.configRequests.slice(before).some((request) => request.tag === "SaveConfigDocument"));
  await app.getByText("Config revision conflict: reload before saving").waitFor();
  if (server.configDocument().content !== external) throw new Error("Revision conflict overwrote the daemon document");
  await editor.press("ctrl-z");
  stage = "Extensions actual singleton page";
  await app.getByTestId("topbar-menu").click(); await app.getByTestId("menu:OpenExtensions").click(); await app.getByTestId("extensions-page").waitFor();
  await until(() => server.configRequests.some((request) => request.tag === "ListExtensions"));
  if (await overlay.count()) throw new Error("Extensions opened an overlay instead of its buffer page");
  console.log("Full native app menus passed: original About/Settings/Themes, actual Config writes and Settings reopen, same-window reload, code-editor config undo/consecutive saves/conflict, and real singleton Extensions buffer page.");
} catch (error) { console.error(`Native menu check failed at ${stage}`); throw error; }
finally { if (reference !== undefined) coreCall("menu.dispose", { id: reference }); await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
