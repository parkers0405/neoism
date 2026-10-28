import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { TreeNode } from "@gpuix/solid/automation";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";

// Use the production entrypoint and native input. The full daemon catalog is
// intentionally larger than this window's saved/open workspace membership.
const directory = mkdtempSync(join(tmpdir(), "neoism-workspace-window-"));
const config = join(directory, "config.json"), recovery = join(directory, "state.json");
const configuration = { appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } };
writeFileSync(config, JSON.stringify(configuration));
const server = createAgentDaemon({ configuration, deferCreations: false });
server.workspaces.push(
  { id: "old-home-1", host_id: "host", title: "Home", root_dir: "/home/fixture" },
  { id: "old-home-2", host_id: "host", title: "Home", root_dir: "/home/fixture" },
);
const packaged = process.argv.includes("--packaged");
let app: Awaited<ReturnType<typeof launchNative>> | undefined;
const launch = async () => app = await launchNative({
  command: packaged ? join(process.cwd(), "dist/neoism-gpuix") : process.execPath,
  args: [...(packaged ? [] : ["src/main.tsx"]), "--daemon", server.endpoint, "--config", config],
  cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: recovery },
});
const until = async (predicate: () => boolean | Promise<boolean>) => {
  const deadline = Date.now() + 15000;
  while (!await predicate()) { if (Date.now() > deadline) throw new Error("Workspace window state did not converge"); await Bun.sleep(15); }
};
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
const tree = async () => { const root = (await app!.call("getTree", {})).tree; return flatten(root ? [root] : []); };
const workspaceTabs = async () => {
  const strip = (await tree()).find(node => node.testId === "workspace-tabs");
  return flatten(strip?.children || []).filter(node => /^workspace:/.test(node.testId || ""));
};
const assertOpen = async (ids: string[]) => {
  const actual = (await workspaceTabs()).map(node => node.testId!.slice("workspace:".length));
  if (JSON.stringify(actual) !== JSON.stringify(ids)) throw new Error(`Window workspace membership differs ${JSON.stringify({ actual, expected: ids })}`);
};
const geometry: unknown[] = [];
const assertGeometry = async (sidebarVisible: boolean) => {
  const nodes = await tree(), bounds = (id: string) => nodes.find(node => node.testId === id)?.bounds;
  const topbar = bounds("original-topbar"), strip = bounds("workspace-tabs"), content = bounds("workspace-content"), sidebar = bounds("sidebar");
  const pane = nodes.find(node => /^pane-tabs:/.test(node.testId || ""))?.bounds;
  if (!topbar || !strip || !content || !pane || strip.width <= 100 || content.height <= 100) return false;
  const near = (a: number, b: number) => Math.abs(a - b) <= 1.3;
  const aligned = near(strip.y, topbar.y + topbar.height) && near(strip.x, content.x) && near(strip.width, content.width)
    && near(pane.y, strip.y + strip.height)
    && (sidebarVisible ? !!sidebar && near(sidebar.y, strip.y) && near(strip.x, sidebar.x + sidebar.width) : !sidebar && near(strip.x, topbar.x));
  if (aligned) geometry.push({ sidebarVisible, topbar, strip, content, sidebar, pane });
  return aligned;
};
const assertNoWrites = () => {
  const mutations = server.workspaceRequests.filter(request => /^(Create|Close|Remove|Delete)/.test(request.tag));
  if (mutations.length || server.ptyRequests.length) throw new Error(`Workspace enumeration mutated daemon state ${JSON.stringify({ mutations, pty: server.ptyRequests })}`);
};
const press = (keys: string) => app!.call("keystrokes", { keys });
let stage = "fresh window resolves only A from the full four-workspace catalog";
try {
  await launch(); await app!.getByTestId("welcome-page").waitFor({ timeoutMs: 20000 });
  await until(async () => (await workspaceTabs()).length > 0); await assertOpen(["a"]);
  if (server.workspaces.length !== 4 || !server.workspaceRequests.some(request => request.tag === "RequestHostWorkspaceTree")) throw new Error("Fixture did not supply the complete workspace catalog");
  assertNoWrites();
  stage = "workspace bar starts beside the sidebar directly under the topbar";
  await until(() => assertGeometry(true));
  stage = "hiding and showing sidebar recomputes workspace bar width and origins";
  await press("alt-e"); await until(() => assertGeometry(false));
  await press("alt-e"); await until(() => assertGeometry(true));
  const originalA = (await workspaceTabs())[0]!.id;
  stage = "actual command palette opens the daemon workspace picker";
  await press("alt-p"); await app!.getByTestId("command-dialog").waitFor();
  await app!.getByTestId("command-palette").fill("Workspaces");
  await app!.getByTestId("command:ShowWorkplaces").waitFor();
  await app!.getByTestId("command:ShowWorkplaces").click();
  await app!.getByTestId("chrome-action-overlay").waitFor();
  stage = "picker keyboard search opens B without adopting old duplicate home entries";
  await app!.getByTestId("chrome-action-overlay").press("b"); await app!.getByTestId("chrome-action-overlay").press("enter");
  await until(async () => (await workspaceTabs()).length === 2 && await app!.getByTestId("chrome-action-overlay").count() === 0);
  await assertOpen(["a", "b"]); await app!.getByTestId("welcome-page").waitFor();
  stage = "existing A tab host survives catalog refresh and opening B";
  const currentA = (await workspaceTabs())[0]!.id;
  if (currentA !== originalA) throw new Error(`Opening another workspace recreated the existing A tab ${JSON.stringify({ originalA, currentA })}`);
  await until(() => assertGeometry(true)); assertNoWrites();
  await until(() => { try { return JSON.stringify(JSON.parse(readFileSync(recovery, "utf8")).ui.openWorkspaceIds) === '["a","b"]'; } catch { return false; } });
  stage = "same daemon identity and recovery restore only A+B and Welcome";
  await app!.close(); app = undefined; await launch();
  await app!.getByTestId("welcome-page").waitFor({ timeoutMs: 20000 });
  await until(async () => (await workspaceTabs()).length === 2); await assertOpen(["a", "b"]);
  await until(() => assertGeometry(true)); assertNoWrites();
  const restored = JSON.parse(readFileSync(recovery, "utf8"));
  if (!restored.surfaces.some((surface: { kind: string; workspaceId: string }) => surface.kind === "welcome" && surface.workspaceId === "b")) throw new Error("Welcome was not recovered for the selected workspace");
  console.log(JSON.stringify({ packaged, fullProductionApp: true, catalogIds: server.workspaces.map(workspace => workspace.id), initialWindowIds: ["a"], restoredWindowIds: ["a", "b"], stableExistingWorkspaceTab: true, workspacePickerUsesNativeKeyboard: true, noDaemonOrPtyWrites: true, welcomeRestored: true, geometry }));
} catch (error) {
  console.error(`Native workspace window check failed at ${stage}`);
  console.error(JSON.stringify({ nodes: app ? (await tree()).filter(node => node.testId).map(node => ({ testId: node.testId, bounds: node.bounds })) : [], workspace: server.workspaceRequests, pty: server.ptyRequests }));
  throw error;
} finally { await app?.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
