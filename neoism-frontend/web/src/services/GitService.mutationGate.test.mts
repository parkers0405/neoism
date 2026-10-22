import assert from "node:assert/strict";
import test from "node:test";
import { GitPanelService, type GitPanelBridge } from "./GitService";
import { TerminalPanel } from "../terminal/TerminalPanel";
import type { ProtocolClient } from "../workspace/ProtocolClient";

const tick = () => new Promise<void>(resolve => setImmediate(resolve));

function harness() {
  const panel: any = Object.create(TerminalPanel.prototype);
  const sent: unknown[] = [];
  const queued: unknown[] = [];
  let callback!: (id: number, envelope: string) => void;
  let dirty = true;
  let insideProvider = false;
  const bridge: GitPanelBridge & Record<string, any> = {
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_prepare_stage(path) {
      assert.equal(insideProvider, false, "must unwind wasm before re-entering safety gate");
      if (!dirty) return true;
      queued.push({root: "/repo", path, absolute: "/repo/" + path, unstage: false});
      return false;
    },
    git_panel_prepare_restore() { return !dirty; },
    takeGitDiffSaveStages: () => queued.splice(0),
    editorDirty: () => dirty,
  };
  const client = { async requestGit(message: unknown) { sent.push(message); return { ChangedFiles: {files: [], error: null, branch: "main"} }; } };
  panel.options = {workspaceRoot: "/repo", client};
  panel.wasmAdapter = bridge;
  panel.pendingDiffSaveStages = new Map();
  panel.activeCodeBufferId = () => "file:///repo/a.rs";
  panel.saveActiveEditorPane = () => { sent.push("save-requested"); return true; };
  panel.bufferTabs = [];
  panel.refreshGitSidePanelData = async () => {};
  panel.scheduleDraw = () => {};
  panel.pushInAppNotification = () => sent.push("notification");
  new GitPanelService(client as unknown as ProtocolClient, bridge).install();
  return {panel, sent, setDirty: (value: boolean) => { dirty = value; }, emit: (message: unknown) => {
    insideProvider = true;
    callback(1, JSON.stringify(message));
    insideProvider = false;
  }};
}

test("sidebar Stage marshalling queues canonical dirty source and forwards only after Saved", async () => {
  const {panel, sent, emit, setDirty} = harness();
  emit({Stage: {path: "a.rs"}});
  await tick();
  assert.deepEqual(sent, []);
  panel.drainSidePanelOpens();
  assert.deepEqual(sent, ["save-requested"]);
  setDirty(false);
  panel.markBufferSaved("file:///repo/a.rs");
  await tick();
  assert.deepEqual(sent, ["save-requested", {Stage: {path: "a.rs"}}]);
});

test("sidebar Unstage bypasses source-save preparation", async () => {
  const {panel, sent, emit} = harness();
  emit({Unstage: {path: "a.rs"}});
  await tick();
  assert.deepEqual(sent[0], {Unstage: {path: "a.rs"}});
  assert.equal(panel.pendingDiffSaveStages.size, 0);
});

test("confirmed Restore cannot forward a dirty canonical source", async () => {
  const {sent, emit} = harness();
  emit({Restore: {path: "a.rs"}});
  await tick();
  assert.deepEqual(sent, []);
});

test("workspace switches invalidate queued save acknowledgements", async () => {
  const {panel, sent, emit, setDirty} = harness();
  emit({Stage: {path: "a.rs"}});
  await tick();
  panel.drainSidePanelOpens();
  panel.options.workspaceRoot = "/other";
  setDirty(false);
  panel.markBufferSaved("file:///repo/a.rs");
  await tick();
  assert.deepEqual(sent, ["save-requested", "notification"]);
});
