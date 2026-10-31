import { Effect, Schema } from "effect";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { TreeSchema, expectVariant, isRecord, variant } from "../src/daemon/protocol";
import { launchNative } from "../tests/support/native-launch";
import type { TreeNode } from "@gpuix/solid/automation";
// This check creates and closes only its own shell. Existing tabs/shells are untouched.
const config = loadConfig(), client = new DaemonConnection(config);
const directory = mkdtempSync(join(tmpdir(), "neoism-ghostty-window-")), recovery = join(directory, "session.json");
const until = async (test: () => boolean | Promise<boolean>) => { const deadline = Date.now() + 15000; while (!await test()) { if (Date.now() > deadline) throw new Error("Native terminal verification timed out"); await Bun.sleep(20); } };
let sessionId: string | undefined, app: Awaited<ReturnType<typeof launchNative>> | undefined, output = "", stage = "daemon connection";
const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
const selectTerminal = async () => {
  let tab: TreeNode | undefined;
  await until(async () => {
    const { tree } = await app!.call("getTree", {});
    tab = flatten(tree ? [tree] : []).find(node => /^pane-tab:\d+$/.test(node.testId || "") && flatten([node]).some(child => child.text === "Terminal"));
    return !!tab;
  });
  await app!.getByTestId(tab!.testId!).click();
};
const cleanupOutput = client.subscribe((event) => {
  const entry = variant(event.message);
  if (event.family === "Pty" && entry?.[0] === "PtyOutput" && isRecord(entry[1]) && entry[1].session_id === sessionId && Array.isArray(entry[1].bytes)) output += new TextDecoder().decode(Uint8Array.from(entry[1].bytes));
});
try {
  client.start(); await until(() => client.state.phase === "connected");
  const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await Effect.runPromise(client.request("Workspace", "RequestHostWorkspaceTree")), "HostWorkspaceTree"));
  const workspace = tree.workspaces.find((w) => w.id === config.workspaceId) || tree.workspaces.find((w) => w.root_dir);
  if (!workspace?.root_dir) throw new Error("No workspace available for native terminal verification");
  stage = "create isolated shell";
  const created = expectVariant(await Effect.runPromise(client.request("Pty", { CreatePty: { cwd: workspace.root_dir, cols: 90, rows: 28, shell: "/bin/sh" } })), "PtyCreated");
  if (!isRecord(created) || typeof created.session_id !== "string") throw new Error("Daemon did not create a PTY");
  sessionId = created.session_id;
  const id = `terminal:${sessionId}`;
  writeFileSync(recovery, JSON.stringify({ version: 1, activeWorkspaceId: workspace.id, activeSurfaceId: id,
    surfaces: [{ id, workspaceId: workspace.id, kind: "terminal", title: "Terminal", sessionId }],
    editors: {}, providerStates: {}, drafts: {}, composerDrafts: {}, ui: { openWorkspaceIds: [workspace.id] } }));
  const launch = () => launchNative({ command: process.execPath, args: ["src/main.tsx", ...process.argv.slice(2), "--workspace", workspace.id], cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: recovery } });
  app = await launch(); await selectTerminal();
  const terminal = app.getByTestId("terminal"); await terminal.waitFor({ timeoutMs: 15000 });
  stage = "Ghostty keyboard and ANSI output";
  await terminal.fill("printf '\\033[32mNEOISM_GHOSTTY_%s\\033[0m\\n' READY; printf 'NEOISM_PID_%s\\n' \"$$\""); await terminal.press("enter");
  await until(() => output.includes("NEOISM_GHOSTTY_READY"));
  await until(() => /NEOISM_PID_\d+/.test(output)); const pid = output.match(/NEOISM_PID_(\d+)/)![1];
  stage = "UI restart and retained shell"; await app.close(); app = await launch();
  await selectTerminal();
  await app.getByTestId("terminal").waitFor({ timeoutMs: 15000 }); await app.getByTestId("terminal-viewport").waitFor();
  output = "";
  await app.getByTestId("terminal").fill("printf 'NEOISM_PID_%s\\n' \"$$\""); await app.getByTestId("terminal").press("enter");
  await until(() => /NEOISM_PID_\d+/.test(output));
  if (output.match(/NEOISM_PID_(\d+)/)?.[1] !== pid) throw new Error("Frontend restart replaced the shell process");
  stage = "shell exit closes only the terminal tab";
  await app.getByTestId("terminal").fill("exit"); await app.getByTestId("terminal").press("enter");
  await until(async () => await app!.getByTestId("terminal").count() === 0);
  await app.getByTestId("topbar-menu").click();
  await app.getByTestId("original-hamburger-menu").waitFor();
  sessionId = undefined;
  console.log("Ghostty keyboard/ANSI output, retained shell across frontend restart, and shell exit closing only its tab while Neoism remains interactive passed.");
} catch (error) { console.error(`Native terminal check failed at ${stage}`); throw error; }
finally {
  await app?.close();
  if (sessionId && client.state.phase === "connected") try { await Effect.runPromise(client.request("Pty", { ClosePty: { session_id: sessionId } })); } catch {}
  cleanupOutput(); client.stop(); rmSync(directory, { recursive: true, force: true });
}
