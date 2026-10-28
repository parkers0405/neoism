import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import type { TreeNode } from "@gpuix/solid/automation";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";

// Production main/model/App and actual native pointer/key input. The daemon and
// files are isolated; timing includes automation polling, not compositor latency.
const directory = mkdtempSync(join(tmpdir(), "neoism-native-switching-")), config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }));
const server = createAgentDaemon({ deferCreations: false, fileReadDelayMs: 120, directoryEntries: {
  "": [{ name: "folder", is_dir: true }, { name: "first.rs", is_dir: false }, { name: "second.rs", is_dir: false }],
  folder: [{ name: "nested.rs", is_dir: false }],
} });
for (const file of ["first.rs", "second.rs", "folder/nested.rs"]) server.files.set(file, Array.from({ length: 80 }, (_, i) => `pub fn ${file.includes("second") ? "second" : "first"}_${i}() -> usize { ${i} }`).join("\n"));
const app = await launchNative({ openWorkspaceIds:["a","b"], command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config, "--reduced-motion"],
  cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: join(directory, "state.json") } });
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
const nodes = async () => { const root = (await app.call("getTree", {})).tree; return flatten(root ? [root] : []); };
const until = async (predicate: () => Promise<boolean> | boolean) => { const end = performance.now() + 10000; while (!await predicate()) { if (performance.now() > end) throw new Error("Switching state did not converge"); await Bun.sleep(10); } };
const timings: Record<string, number> = {};
const measure = async (name: string, operation: () => Promise<void>, ready: () => Promise<boolean>) => { const start = performance.now(); await operation(); await until(ready); timings[name] = performance.now() - start; };
const reads = () => server.fileRequests.filter(request => request.tag === "ReadFile").length;
const listings = () => server.fileRequests.filter(request => request.tag === "ListDir").length;
const subscriptions = () => server.workspaceRequests.filter(request => request.tag === "SubscribeWorkspace").length;
const editorReady = async () => {
  if (await app.getByTestId("code-editor").count() !== 1) return false;
  const bounds = await app.getByTestId("code-editor").bounds();
  return bounds.width > 100 && bounds.height > 100 && (await app.getByTestId("code-editor").getByType("canvas").all()).some(node => (node.bounds?.height || 0) > 100);
};
const hosts = async () => { const tree = await nodes(); return { pane: tree.find(node => /^pane:\d+$/.test(node.testId || ""))?.id, strip: tree.find(node => /^pane-tabs:/.test(node.testId || ""))?.id }; };
const tab = async (title: string) => { const found = (await nodes()).find(node => /^pane-tab:\d+$/.test(node.testId || "") && flatten([node]).some(child => child.text === title)); if (!found?.testId) throw new Error(`Missing buffer tab ${title}`); return found; };
let stage = "initial workspace";
try {
  await app.getByTestId("file:first.rs").waitFor({ timeoutMs: 15000 });
  const rootFile = (await app.getByTestId("file:first.rs").element()).id;
  stage = "cold folder expansion";
  await measure("coldFolderReadyMs", () => app.getByTestId("file:folder").click(), async () => await app.getByTestId("file:folder/nested.rs").count() === 1);
  if ((await app.getByTestId("file:first.rs").element()).id !== rootFile) throw new Error("Expanding a folder remounted an unchanged root row");
  await app.getByTestId("file:folder").click(); await until(async () => await app.getByTestId("file:folder/nested.rs").count() === 0);
  const directoryReads = listings();
  await measure("warmFolderReadyMs", () => app.getByTestId("file:folder").click(), async () => await app.getByTestId("file:folder/nested.rs").count() === 1);
  if (listings() !== directoryReads) throw new Error("Warm folder expansion issued another ListDir RPC");
  stage = "cold file transitions from pending to editor";
  const coldStart = performance.now(); await app.getByTestId("file:first.rs").click();
  await app.getByTestId("loading-skeleton:document").waitFor(); await until(editorReady); timings.coldFileReadyMs = performance.now() - coldStart;
  if (reads() !== 1) throw new Error("Initial cold file caused duplicate ReadFile RPCs");
  await app.getByTestId("code-editor").press("i"); await app.getByTestId("code-editor").press("X"); await app.getByTestId("code-editor").press("escape");
  await app.getByTestId("unsaved-buffer").waitFor();
  const originalHosts = await hosts(), firstTab = await tab("first.rs"), firstEditorId = (await app.getByTestId("code-editor").element()).id;
  stage = "second file and stable pane/tab hosts";
  await measure("secondColdFileReadyMs", () => app.getByTestId("file:second.rs").click(), async () => await editorReady() && (await app.getByTestId("code-editor").element()).id !== firstEditorId);
  // Generic editor readiness can belong to the old view until the click reaches
  // JS. Require the second buffer tab before comparing identities.
  await until(async () => { try { await tab("second.rs"); return await editorReady(); } catch { return false; } });
  const nextHosts = await hosts();
  if (nextHosts.pane !== originalHosts.pane || nextHosts.strip !== originalHosts.strip || (await tab("first.rs")).id !== firstTab.id) throw new Error("Selecting another buffer recreated the pane, tabstrip, or previous tab host");
  const warmReads = reads();
  stage = "warm buffer selection";
  const secondEditorId = (await app.getByTestId("code-editor").element()).id;
  await measure("warmTabReadyMs", () => app.getByTestId(firstTab.testId!).click(), async () => await editorReady() && (await app.getByTestId("code-editor").element()).id !== secondEditorId);
  if (reads() !== warmReads || (await hosts()).strip !== originalHosts.strip) throw new Error("Warm buffer switch refetched bytes or recreated tabstrip");
  stage = "workspace round trip preserves navigation and dirty file";
  await measure("workspaceBReadyMs", () => app.getByTestId("workspace:b").click(), async () => await app.getByTestId("code-editor").count() === 0 && await app.getByTestId("file:first.rs").count() === 1);
  await app.getByTestId("file:first.rs").click(); await until(editorReady);
  const afterBReads = reads();
  await measure("warmWorkspaceAReadyMs", () => app.getByTestId("workspace:a").click(), async () => await app.getByTestId("file:folder/nested.rs").count() === 1 && await editorReady());
  if (reads() !== afterBReads) throw new Error("Returning to workspace A refetched a cached file");
  await app.getByTestId("unsaved-buffer").waitFor();
  const beforeSame = subscriptions(); await app.getByTestId("workspace:a").click(); await Bun.sleep(30);
  if (subscriptions() !== beforeSame) throw new Error("Clicking the active workspace resubscribed");
  await app.getByTestId("code-editor").press("ctrl-s");
  await until(() => server.fileRequests.some(request => request.tag === "WriteFile"));
  if (!server.files.get("first.rs")?.startsWith("Xpub fn first_0")) throw new Error("Workspace/tab round trip lost dirty bytes or saved the wrong surface");
  console.log(JSON.stringify({ fullProductionApp: true, switching: "passed", coldFileReadRpc: 1, warmFolderRpc: 0, warmFileRpc: 0, stablePaneAndTabStrip: true,
    timings, timingScope: "input-through-automation-to-native-positive-bounds-and-glyph-image-nodes; includes polling; excludes compositor presentation" }));
} catch (error) {
  console.error(`Native switching check failed at ${stage}`, JSON.stringify({ requests: server.fileRequests, workspace: server.workspaceRequests }));
  console.error(JSON.stringify((await nodes()).map(node => ({ id: node.id, testId: node.testId, text: node.text })).filter(node => node.testId || node.text).slice(-100)));
  throw error;
} finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
