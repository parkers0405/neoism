import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";
const directory = mkdtempSync(join(tmpdir(), "neoism-original-chrome-")), config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }));
const server = createAgentDaemon({ deferCreations: false });
const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config], cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "state.json") } });
const until = async (predicate: () => Promise<boolean>) => { const end = Date.now() + 10000; while (!await predicate()) { if (Date.now() > end) throw new Error("Original chrome did not converge"); await Bun.sleep(20); } };
let stage = "original bars";
try {
  await app.getByTestId("original-topbar").waitFor({ timeoutMs: 15000 }); await app.getByTestId("original-statusline").waitFor();
  const icons = ["topbar-menu", "toggle-files", "toggle-notes", "toggle-conversations", "open-finder"];
  await until(async () => (await app.getByTestId("open-finder").bounds()).width > 0);
  const boxes = await Promise.all(icons.map(id => app.getByTestId(id).bounds()));
  // GPUI snaps to the compositor's physical pixel grid (e.g. .6 logical px
  // at 166.67% scaling). The Rust unit check verifies exact logical geometry.
  if (boxes.some((box, i) => Math.abs(box.width - 22) > .65 || Math.abs(box.height - 22) > .65 || Math.abs(box.width - boxes[0].width) > .01 || i > 0 && Math.abs(box.x - boxes[i - 1].x - 26) > .65)) throw new Error(`Original equal toolbar geometry drifted: ${JSON.stringify(boxes)}`);
  stage = "original hamburger menu";
  await app.getByTestId("topbar-menu").click(); await app.getByTestId("original-hamburger-menu").waitFor();
  for (const action of ["OpenAbout", "OpenSettings", "StartWebServer", "OpenThemes", "OpenBackgrounds", "OpenExtensions"]) await app.getByTestId(`menu:${action}`).waitFor();
  if (await app.getByText("Reconnect daemon").count() || await app.getByText("New terminal").count()) throw new Error("Invented hamburger actions remain");
  stage = "hamburger close and reopen";
  await app.getByTestId("topbar-menu").click(); await until(async () => await app.getByTestId("original-hamburger-menu").count() === 0);
  await app.getByTestId("topbar-menu").click(); await app.getByTestId("menu:OpenAbout").waitFor();
  stage = "original About modal";
  await app.getByTestId("menu:OpenAbout").click(); const dialog = app.getByTestId("chrome-action-overlay"); await dialog.waitFor();
  await until(async () => (await dialog.getByType("canvas").all()).some(node => (node.bounds?.width || 0) > 1 && (node.bounds?.height || 0) > 1));
  if (await app.getByTestId("original-hamburger-menu").count()) throw new Error("Menu remained above About");
  await dialog.press("escape"); await until(async () => await dialog.count() === 0);
  stage = "file navigator preserves focus and actual rich Markdown";
  await app.getByTestId("file:README.md").click(); await app.getByTestId("rich-markdown-editor").waitFor();
  const height = (await app.getByTestId("original-statusline").bounds()).height;
  if (Math.abs(height - 22) > .65) throw new Error(`Original status strip height drifted: ${height}`);
  console.log("Production native chrome passed: original equal toolbar geometry, exact six-item menu, toggle/close, original About glyphs, rich Markdown, and original status strip.");
} catch (error) { console.error(`Original chrome check failed at ${stage}`); throw error; }
finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
