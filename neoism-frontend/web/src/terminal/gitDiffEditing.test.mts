import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function harness(loaded: boolean) {
  const panel: any = Object.create(TerminalPanel.prototype);
  const calls: string[] = [];
  const tab = { kind: "chrome-git-diff", chromePage: "git_diff", diffRoot: "/repo", diffRouteId: 7 };
  panel.bufferTabs = [tab];
  panel.activeTabIndex = 0;
  panel.options = { workspaceRoot: "/repo", client: {
    sendFiles: () => calls.push("read"),
  } };
  panel.useWasmMarkdown = () => true;
  panel.pumpCrdtOutbox = () => calls.push("flush-markdown");
  panel.pumpCodeCrdt = () => calls.push("flush-code");
  panel.activePaneExternalId = () => 1;
  panel.bindEditorSurface = (_id: number, path: string) => calls.push("bind:" + path);
  panel.scheduleDraw = () => {};
  panel.pendingServiceMappers = new Map();
  panel.toDaemonWorkspacePath = (path: string) => path;
  panel.filesRootForPath = () => "/repo";
  panel.wasmAdapter = {
    activateGitDiffEditor: () => { calls.push("activate"); return loaded; },
    editorOpenFile: () => calls.push("open"),
    editorActiveKind: () => "code",
    gitDiffEditingPath: () => "/repo/a.rs",
  };
  return { panel, calls, tab };
}

test("inline edit reuses canonical loaded source without disk reads or ordinary tabs", () => {
  const { panel, calls, tab } = harness(true);
  panel.beginGitDiffEdit({ path: "/repo/a.rs", line: 4, column: 2 });
  assert.deepEqual(calls, ["flush-markdown", "flush-code", "activate", "bind:/repo/a.rs", "flush-code"]);
  assert.deepEqual(panel.bufferTabs, [tab]);
  assert.equal(panel.activeEditorPaneKind(), "code");
  assert.equal(panel.activeCodeBufferId(), "file:///repo/a.rs");
});

test("missing canonical source requests bytes, never copies patch text", () => {
  const { panel, calls } = harness(false);
  panel.beginGitDiffEdit({ path: "/repo/a.rs", line: 4, column: 2 });
  assert.deepEqual(calls, ["flush-markdown", "flush-code", "activate", "read"]);
  assert.equal(panel.bufferTabs.length, 1);
  assert.equal(panel.pendingServiceMappers.size, 1);
});

test("dirty state belongs to source while review remains a helper tab", () => {
  const { panel, tab } = harness(true);
  const source = {kind: "file", path: "/repo/a.rs", modified: false};
  panel.bufferTabs.push(source);
  panel.activeTabIsMarkdown = () => false;
  panel.wasmAdapter.editorDirty = () => true;
  assert.equal(panel.syncActiveTabModified(), true);
  assert.equal(source.modified, true);
  assert.equal((tab as any).modified, true);
});

test("stage waits for Saved and refuses a newer dirty revision", async () => {
  const { panel, calls } = harness(true);
  panel.pendingDiffSaveStages = new Map();
  panel.options.client.requestGit = async (message: unknown) => { calls.push(JSON.stringify(message)); return { Status: {entries: []} }; };
  panel.refreshGitSidePanelData = async () => {};
  panel.pushInAppNotification = () => calls.push("notification");
  panel.wasmAdapter.editorDirty = () => false;
  const bufferId = panel.activeCodeBufferId();
  panel.pendingDiffSaveStages.set(bufferId, [{root: "/repo", path: "a.rs", adapter: panel.wasmAdapter, scope: "/repo"}]);
  assert.equal(calls.length, 0);
  panel.markBufferSaved(bufferId);
  await Promise.resolve();
  assert.deepEqual(calls, ['{"Stage":{"path":"a.rs"}}']);
  panel.pendingDiffSaveStages.set(bufferId, [{root: "/repo", path: "a.rs", adapter: panel.wasmAdapter, scope: "/repo"}]);
  panel.wasmAdapter.editorDirty = () => true;
  panel.markBufferSaved(bufferId);
  assert.equal(calls.at(-1), "notification");
  assert.equal(calls.length, 2);
});

test("review helper tab routes typing through the existing typed editor controller", () => {
  const { panel } = harness(true);
  const keys: unknown[] = [];
  panel.isChromeKeyboardCaptureActive = () => false;
  panel.activeTabIsMarkdown = () => false;
  panel.wasmAdapter.editorKey = (...args: unknown[]) => { keys.push(args); return true; };
  panel.wasmAdapter.editorDrainClipboardOut = () => null;
  panel.drainEditorLspHostActions = () => {};
  let prevented = false;
  panel.handleKeyDown({ key: "h", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false,
    preventDefault: () => { prevented = true; }, stopPropagation: () => {} });
  assert.deepEqual(keys, [["h", false, false, false]]);
  assert.equal(prevented, true);
});
