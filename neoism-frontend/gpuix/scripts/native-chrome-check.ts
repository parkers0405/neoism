import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";
const directory = mkdtempSync(join(tmpdir(), "neoism-original-chrome-")), config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }));
const server = createAgentDaemon({ deferCreations: false });
server.files.set("main.rs", "fn main() { println!(\"Neoism\"); }\n");
const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config], cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "state.json") } });
const until = async (predicate: () => Promise<boolean>) => { const end = Date.now() + 10000; while (!await predicate()) { if (Date.now() > end) throw new Error("Original chrome did not converge"); await Bun.sleep(20); } };
let stage = "original bars";
try {
  await app.getByTestId("activity-rail").waitFor({ timeoutMs: 15000 }); await app.getByTestId("original-statusline").waitFor();
  const icons = ["topbar-menu", "toggle-files", "toggle-notes", "toggle-conversations", "open-finder", "open-servers"];
  await until(async () => (await app.getByTestId("open-finder").bounds()).width > 0);
  if (await app.getByTestId("sidebar").count()) throw new Error("Fresh windows must start with the sidebar closed");
  await app.getByTestId("toggle-files").click();
  const boxes = await Promise.all(icons.map(id => app.getByTestId(id).bounds()));
  const rail = await app.getByTestId("activity-rail").bounds(), shell = await app.getByTestId("app-shell").bounds();
  // GPUI snaps to the compositor's physical pixel grid (e.g. .6 logical px
  // at 166.67% scaling). The Rust unit check verifies exact logical geometry.
  if (Math.abs(rail.width - 52) > .65 || Math.abs(rail.x - shell.x) > .65 || boxes.some((box, i) => Math.abs(box.width - 52) > .65 || Math.abs(box.height - 48) > .65 || Math.abs(box.x - rail.x) > .65 || Math.abs(box.width - boxes[0].width) > .01 || Math.abs(box.y - rail.y - 6 - i * 48) > .65)) throw new Error(`Vertical activity rail geometry drifted: ${JSON.stringify({ rail, boxes })}`);
  stage = "original hamburger menu";
  await app.getByTestId("topbar-menu").click(); await app.getByTestId("original-hamburger-menu").waitFor();
  for (const action of ["OpenAbout", "OpenSettings", "OpenDatabases", "OpenThemes", "OpenBackgrounds", "OpenExtensions"]) await app.getByTestId(`menu:${action}`).waitFor();
  if (await app.getByTestId("menu:StartWebServer").count()) throw new Error("Open Agent in Browser is still visible in the menu");
  if (await app.getByTestId("toggle-databases").count()) throw new Error("Database still has a standalone toolbar button");
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
  stage = "Files restored in rail, details limited to agents";
  if (await app.getByTestId("toggle-files").count() !== 1) throw new Error("Expected only the rail Files control");
  if (await app.getByTestId("toggle-details").count() || await app.getByTestId("agent-details").count()) throw new Error("Agent details leaked into Markdown");
  await app.getByTestId("toggle-files").click(); await until(async () => await app.getByTestId("file-tree").count() === 0);
  await app.getByTestId("toggle-files").click(); await app.getByTestId("file:README.md").waitFor();
  stage = "code breadcrumbs have no floating controls";
  await app.getByTestId("file:main.rs").click(); await app.getByTestId("code-breadcrumbs").waitFor();
  if (await app.getByTestId("toggle-details").count() || await app.getByTestId("agent-details").count()) throw new Error("Agent details leaked into the code editor");
  const height = (await app.getByTestId("original-statusline").bounds()).height;
  if (Math.abs(height - 22) > .65) throw new Error(`Original status strip height drifted: ${height}`);
  console.log("Production native chrome passed: activity rail with Files, no agent details on files, hamburger actions, About glyphs, rich Markdown, and status strip.");
} catch (error) { console.error(`Original chrome check failed at ${stage}`); throw error; }
finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
