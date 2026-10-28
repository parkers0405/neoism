import { launchNative as launch } from "../tests/support/native-launch";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { mkdtempSync, rmSync } from "node:fs";
import { loadConfig } from "../src/config";
import { dimensions } from "../src/design/generated";

// The real daemon; this check never saves documents or submits prompts.
const args = process.argv.slice(2).filter((arg) => arg !== "--packaged");
const packaged = process.argv.includes("--packaged"), config = loadConfig(args);
const directory = mkdtempSync(join(tmpdir(), "neoism-native-smoke-"));
const app = await launch({ command: packaged ? resolve("dist/neoism-gpuix") : process.execPath,
  args: [...(packaged ? [] : ["src/main.tsx"]), ...args], cwd: packaged ? tmpdir() : process.cwd(),
  env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "session.json") } });
const until = async (predicate: () => Promise<boolean>) => { const end = Date.now() + 15000; while (!await predicate()) { if (Date.now() > end) throw new Error("Native smoke did not converge"); await Bun.sleep(25); } };
let stage = "workspace", editorChecked = false;
try {
  await app.getByTestId("workspace-tabs").waitFor({ timeoutMs: 15000 });
  await app.getByTestId("file-tree").waitFor({ timeoutMs: 15000 });
  const width = (await app.getByTestId("file-tree").bounds()).width;
  if (Math.abs(width - dimensions.sidebarWidth) > 8) throw new Error(`Sidebar width drifted: ${width}`);
  stage = "loaded workspace body geometry";
  await app.getByTestId("pane-layout").waitFor({ timeoutMs: 15000 });
  await until(async () => await app.getByTestId("loading-skeleton:tree").count() === 0);
  // Loading labels have intrinsic height; loaded file rows are positioned
  // absolutely. Check the settled flex region rather than just its width or
  // the existence of nodes, which also pass when the body has collapsed.
  const assertBody = async () => {
    const tree = await app.getByTestId("file-tree").bounds(), panes = await app.getByTestId("pane-layout").bounds();
    if (tree.height < 100 || panes.height < 100 || panes.width < 100) throw new Error(`Loaded workspace body collapsed: tree=${JSON.stringify(tree)}, panes=${JSON.stringify(panes)}`);
    const composer = app.getByTestId("composer");
    if (await composer.count()) { const bounds = await composer.bounds(); if (bounds.width < 100 || bounds.height < 20) throw new Error(`Loaded composer collapsed: ${JSON.stringify(bounds)}`); }
  };
  await assertBody(); await Bun.sleep(800); await assertBody();
  const painted = (await app.call("getPaintedText", {})).text;
  if (painted.some((line) => /img:\s*(no src|error)/i.test(line))) throw new Error("Native image placeholder remains after workspace loading");
  const firstRow = (await app.getByTestId("file-tree").element()).children?.find((node) => node.testId?.startsWith("file:"));
  const rowLabel = firstRow?.testId?.slice(5).split("/").pop()?.slice(0, 10);
  if (painted.length && rowLabel && !painted.some((line) => line.includes(rowLabel))) throw new Error(`Loaded tree row exists but is not painted: ${rowLabel}`);
  if (!painted.length) console.log("Painted-text introspection is unavailable; native geometry/input checks continue. Use compositor capture to assess appearance.");
  stage = "sidebar mouse interaction";
  await app.getByTestId("toggle-conversations").click();
  await app.getByTestId("conversation-list").waitFor();
  if (await app.getByTestId("file-tree").count()) throw new Error("File navigator did not close");
  await app.getByTestId("toggle-files").click(); await app.getByTestId("file-tree").waitFor();
  if (await app.getByTestId("conversation-list").count()) throw new Error("Conversation navigator did not close");
  stage = "Agent picker input";
  if (await app.getByTestId("select-thinking").count()) {
    await app.getByTestId("select-thinking").click(); await app.getByTestId("agent-picker:thinking").waitFor();
    const filter = app.getByTestId("inline-picker-input"); await filter.fill("medium"); await filter.press("down"); await filter.press("escape");
    await app.getByTestId("select-model").click(); await app.getByTestId("agent-picker:model").waitFor(); await app.getByTestId("inline-picker-input").press("escape");
  }
  stage = "file loading";
  interface Node { testId?: string; children?: Node[] }
  const flatten = (nodes: Node[]): Node[] => nodes.flatMap((node) => [node, ...flatten(node.children || [])]);
  let fileId: string | undefined;
  const end = Date.now() + 10000;
  do {
    const entries = flatten(await app.getByTestId("file-tree").all()).map((node) => node.testId || "");
    fileId = entries.find((id) => id === "file:README.md") || entries.find((id) => /^file:.*\.(rs|ts|tsx|js|jsx|md|json|toml|txt|py|yaml|yml|lua|zig|c|h)$/i.test(id));
    if (fileId || await app.getByText("No files").count()) break;
    await Bun.sleep(25);
  } while (Date.now() < end);
  if (fileId) {
    await app.getByTestId(fileId).click(); const editor = app.getByTestId(/\.(md|markdown|mdown|mkd)$/i.test(fileId) ? "rich-markdown-editor" : "code-editor"); await editor.waitFor({ timeoutMs: 10000 });
    stage = "editor navigation";
    if (config.vim) {
      await editor.press("escape"); await editor.press("j"); await editor.press("ctrl-d"); await editor.wheel(0, -20);
      await editor.press("/"); const prompt = app.getByTestId("command-palette"); await prompt.waitFor(); await prompt.fill("Neoism"); await prompt.press("escape");
      await editor.press(":"); await prompt.waitFor(); await prompt.fill("$"); await prompt.press("enter");
      await app.getByTestId("original-statusline").waitFor();
    }
    stage = "real daemon Finder";
    await editor.press("ctrl-p"); await app.getByTestId("finder").waitFor(); const finder = app.getByTestId("command-palette");
    await finder.fill(fileId.slice("file:".length));
    await until(async () => await app.getByTestId("finder-result:0").count() > 0);
    await finder.press("down"); await finder.press("escape");
    editorChecked = true;
  }
  console.log(`Native SolidJS window, sidebar geometry and actual mouse switching passed. ${editorChecked ? "Real file loading, editor navigation, and daemon Finder passed." : "Editor/Finder skipped: this workspace has no readable top-level text file; use check:recovery and check:finder for their isolated scenarios."}`);
  try { await app.screenshot({ path: resolve("artifacts/native.png") }); }
  catch { console.log("Native GPU screenshot capture is unavailable on Linux; compositor captures are kept private."); }
} catch (error) { console.error(`Read-only native smoke failed at ${stage}`); throw error; }
finally { await app.close(); rmSync(directory, { recursive: true, force: true }); }
