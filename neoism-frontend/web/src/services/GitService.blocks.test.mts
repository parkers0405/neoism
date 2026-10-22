import assert from "node:assert/strict";
import test from "node:test";
import { GitPanelService, type GitPanelBridge } from "./GitService";
import { TerminalPanel } from "../terminal/TerminalPanel";
import type { ProtocolClient } from "../workspace/ProtocolClient";

const tick = () => new Promise<void>(resolve => setImmediate(resolve));
const block = { old_start: 8, new_start: 9, old_lines: ["before"], new_lines: ["after", "extra"] };
function harness(error: string | null = null, hold = false) {
  const panel: any = Object.create(TerminalPanel.prototype);
  const sent: any[] = [];
  const queued: any[] = [];
  const restored: unknown[] = [];
  let callback!: (id: number, json: string) => void;
  let dirty = true;
  let revision = "9007199254740999";
  let insideProvider = false;
  const bridge: GitPanelBridge & Record<string, any> = {
    gitPanelContext: () => panel.wasmAdapter === bridge ? panel.options.workspaceRoot : null,
    set_git_panel_ops(cb) { callback = cb; },
    prepareGitStageBlock(path, json) {
      assert.equal(insideProvider, false);
      if (!dirty) return true;
      queued.push({root: "/repo", path, absolute: "/repo/" + path, block: JSON.parse(json), revision});
      return false;
    },
    git_panel_prepare_restore() { return !dirty; },
    git_panel_restore_identity() { return JSON.stringify({root: "/repo", absolute: "/repo/a.rs", revision, text: "after\nextra\n"}); },
    git_panel_block_restored(json, bufferId) { restored.push([JSON.parse(json).absolute, bufferId]); return true; },
    takeGitDiffBlockSaveStages: () => queued.splice(0),
    editorDirty: () => dirty,
    editorRevision: () => revision,
  };
  let release!: () => void;
  const waiting = new Promise<void>(resolve => { release = resolve; });
  const client = { async requestGit(message: unknown) { sent.push(message); if (hold) await waiting; return {ChangedFiles: {files: [], error, branch: "main"}}; } };
  panel.options = {workspaceRoot: "/repo", client};
  panel.wasmAdapter = bridge;
  panel.pendingDiffSaveStages = new Map();
  panel.activeCodeBufferId = () => "file:///repo/a.rs";
  panel.saveActiveEditorPane = () => { sent.push("save"); return true; };
  panel.bufferTabs = [];
  panel.refreshGitSidePanelData = async () => {};
  panel.scheduleDraw = () => {};
  panel.pushInAppNotification = () => sent.push("notification");
  new GitPanelService(client as unknown as ProtocolClient, bridge).install();
  return { panel, sent, restored, release, clean: () => { dirty = false; }, edit: () => { revision += "1"; }, emit(restore = false) {
    insideProvider = true;
    callback(1, JSON.stringify({ApplyBlock: {path: "a.rs", block, restore}}));
    insideProvider = false;
  }};
}

test("block Stage waits for the matching Saved and preserves exact payload, never Stage", async () => {
  const h = harness();
  h.emit(); await tick();
  assert.deepEqual(h.sent, []);
  h.panel.drainSidePanelOpens();
  assert.deepEqual(h.sent, ["save"]);
  h.clean();
  h.panel.markBufferSaved("file:///repo/unrelated.rs");
  assert.deepEqual(h.sent, ["save"]);
  h.panel.markBufferSaved("file:///repo/a.rs"); await tick();
  assert.deepEqual(h.sent, ["save", {ApplyBlock: {path: "a.rs", block, restore: false}}]);
});

for (const change of ["edit", "workspace", "adapter"] as const) {
  test(`block Stage cancels after ${change} during save`, async () => {
    const h = harness(); h.emit(); await tick(); h.panel.drainSidePanelOpens();
    if (change === "edit") h.edit();
    if (change === "workspace") h.panel.options.workspaceRoot = "/other";
    if (change === "adapter") h.panel.wasmAdapter = {};
    h.clean(); h.panel.markBufferSaved("file:///repo/a.rs"); await tick();
    assert.deepEqual(h.sent, ["save", "notification"]);
  });
}

test("block Stage cancels an edit between preflight and queue drain", async () => {
  const h = harness(); h.emit(); await tick(); h.edit(); h.panel.drainSidePanelOpens();
  assert.deepEqual(h.sent, ["notification"]);
});

for (const change of ["workspace", "adapter"] as const) {
  test(`successful restore reply after ${change} switch cannot touch the new owner`, async () => {
    const h = harness(null, true); h.clean(); h.emit(true); await tick();
    if (change === "workspace") h.panel.options.workspaceRoot = "/other";
    else h.panel.wasmAdapter = {};
    h.release(); await tick();
    assert.deepEqual(h.restored, []);
  });
}

test("edits while restoring use an authoritative delta, never a disk read or replacement", async () => {
  const h = harness(null, true); h.clean(); h.emit(true); await tick();
  h.edit(); h.release(); await tick();
  assert.deepEqual(h.restored, [["/repo/a.rs", "file:///repo/a.rs"]]);
  // No requestFiles exists in this harness: a disk reload would fail this test.
});

test("failed block Restore never reconciles the owner", async () => {
  const h = harness("stale block"); h.clean(); h.emit(true); await tick();
  assert.deepEqual(h.restored, []);
});

test("block Restore refuses dirty source; clean confirmed restore notifies canonical owner", async () => {
  const h = harness(); h.emit(true); await tick();
  assert.deepEqual(h.sent, []);
  h.clean(); h.emit(true); await tick();
  assert.deepEqual(h.sent[0], {ApplyBlock: {path: "a.rs", block, restore: true}});
  assert.deepEqual(h.restored, [["/repo/a.rs", "file:///repo/a.rs"]]);
});

for (const change of ["workspace", "adapter"] as const) {
  test(`provider block preflight drops a stale ${change} before dispatch`, async () => {
    const h = harness(); h.clean(); h.emit();
    if (change === "workspace") h.panel.options.workspaceRoot = "/other";
    else h.panel.wasmAdapter = {};
    await tick();
    assert.deepEqual(h.sent, []);
    assert.deepEqual(h.restored, []);
  });
}

test("a second block during Saved wait is explicitly refused without replacing the first", async () => {
  const h = harness(); h.emit(); h.emit(); await tick();
  h.panel.drainSidePanelOpens(); h.clean(); h.panel.markBufferSaved("file:///repo/a.rs"); await tick();
  assert.deepEqual(h.sent, ["save", "notification", {ApplyBlock: {path: "a.rs", block, restore: false}}]);
});

test("clean block Stage sends ApplyBlock without any whole-file preflight", async () => {
  const h = harness(); h.clean(); h.emit(); await tick();
  assert.deepEqual(h.sent[0], {ApplyBlock: {path: "a.rs", block, restore: false}});
});
