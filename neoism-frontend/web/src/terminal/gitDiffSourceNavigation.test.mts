import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function harness(retained: boolean, existing = false, path = "/repo/a.rs") {
  const panel: any = Object.create(TerminalPanel.prototype);
  const calls: unknown[] = [];
  const review = { title: "Review", kind: "chrome-git-diff", chromePage: "git_diff", diffRoot: "/repo", diffPath: "/repo/a.rs", diffRouteId: 7 };
  const source = { title: "a.rs", kind: "file", path, modified: true };
  panel.bufferTabs = existing ? [review, source] : [review];
  panel.activeTabIndex = 0;
  panel.options = {};
  panel.pumpCrdtOutbox = () => calls.push("flush-markdown");
  panel.pumpCodeCrdt = () => calls.push("flush-code");
  panel.syncActiveTabModified = () => {};
  panel.tabIdentityPath = (path: string) => path;
  panel.explicitlyReopenTabKey = (key: string) => calls.push(["reopen", key]);
  panel.syncActiveBreadcrumbs = () => {};
  panel.syncPaneTerminals = () => {};
  panel.renderPaneLayoutOverlay = () => {};
  panel.notifyBufferTabsChanged = () => {};
  panel.ensureSessionLayoutState = () => {};
  panel.assignActiveTabToFocusedEditorPane = () => calls.push(["bind", panel.bufferTabs[panel.activeTabIndex].path]);
  panel.setMarkdownLayerVisible = () => {};
  panel.requestFileContent = (path: string, index: number) => calls.push(["read", path, index]);
  panel.openFileTabContent = (path: string) => calls.push(["open-buffer", path]);
  panel.scheduleDraw = () => {};
  let published: any[] = [];
  panel.wasmAdapter = {
    sourceEditorPolicy: () => retained ? "code" : "load",
    setBufferTabs: (json: string, active: number) => {
      published = JSON.parse(json);
      calls.push(["tabs", active]);
    },
    setActiveTab: (active: number) => calls.push(["select", active]),
    activateSourceEditor: (index: number, path: string, line: number, column: number) => {
      assert.equal(published[index].kind, "file", "source metadata must precede activation");
      assert.equal(published[index].path, path);
      assert.equal(published[index].sourceCode, true);
      assert.equal(published[index].chromePage, null);
      calls.push(["activate-source", index, path, line, column]);
      return retained;
    },
    activateGitDiffEditor: () => { throw new Error("source navigation must never enter inline editing"); },
    editorOpenFile: () => { throw new Error("retained source must not be reseeded"); },
    focusEditorInput: () => calls.push("focus"),
  };
  return { panel, calls, review, source };
}

for (const existing of [false, true]) {
  test(`source gutter ${existing ? "reuses" : "creates"} ordinary tab before activating retained canonical pane`, () => {
    const { panel, calls, review, source } = harness(true, existing);
    panel.openGitDiffSource({ path: "/repo/a.rs", line: 4, column: 2 });
    assert.equal(panel.bufferTabs.length, 2);
    assert.equal(panel.bufferTabs[0], review);
    assert.equal(panel.activeTabIndex, 1);
    if (existing) assert.equal(panel.bufferTabs[1], source);
    assert.deepEqual(calls.filter((c) => Array.isArray(c) && ["tabs", "select", "activate-source", "bind"].includes(c[0])), [
      ["tabs", 1], ["select", 1], ["activate-source", 1, "/repo/a.rs", 4, 2], ["bind", "/repo/a.rs"],
    ]);
    assert.equal(calls.some((c) => Array.isArray(c) && c[0] === "read"), false);
    assert.deepEqual(calls.slice(0, 2), ["flush-markdown", "flush-code"]);
  });
}

test("unloaded source opens ordinary tab and requests bytes after arming zero-based target", () => {
  const { panel, calls } = harness(false);
  panel.openGitDiffSource({ path: "/repo/a.rs", line: 0, column: 0 });
  const activation = calls.findIndex((c) => Array.isArray(c) && c[0] === "activate-source");
  const read = calls.findIndex((c) => Array.isArray(c) && c[0] === "read");
  assert.ok(activation >= 0 && read > activation);
  assert.deepEqual(calls[activation], ["activate-source", 1, "/repo/a.rs", 0, 0]);
  assert.deepEqual(calls[read], ["read", "/repo/a.rs", 1]);
  assert.equal(panel.bufferTabs[panel.activeTabIndex].kind, "file");
});

test("side-panel drain consumes source requests separately from inline edit requests", () => {
  const { panel, calls } = harness(true);
  let requests = [{ path: "/repo/a.rs", line: 9, column: 0 }];
  panel.wasmAdapter.drainGitDiffSourceOpens = () => { const rows = requests; requests = []; return rows; };
  panel.drainSidePanelOpens();
  panel.drainSidePanelOpens();
  assert.equal(calls.filter((c) => Array.isArray(c) && c[0] === "activate-source").length, 1);
  assert.equal(panel.activeTabIndex, 1);
});

for (const extension of ["md", "ipynb", "neodraw"]) {
  test(`${extension} source jump uses raw code routing and preserves retained code owner`, () => {
    const path = `/repo/file.${extension}`;
    const { panel, calls } = harness(true, true, path);
    panel.bufferTabs[1].sourceCode = true;
    panel.useWasmMarkdown = () => true;
    panel.wasmAdapter.editorActiveKind = () => "code";
    panel.openGitDiffSource({ path, line: 7, column: 0 });
    assert.equal(panel.bufferTabs[1].sourceCode, true);
    assert.equal(panel.activeTabIsMarkdown(), false);
    assert.equal(panel.isEditorLikeTab(panel.bufferTabs[1]), true);
    assert.equal(panel.activeEditorPaneKind(), "code");
    assert.equal(calls.some((call) => Array.isArray(call) && call[0] === "read"), false);
    assert.ok(calls.some((call) => Array.isArray(call) && call[0] === "activate-source" && call[3] === 7));
  });

  test(`${extension} special owner is explicitly unsupported without changing selection or reading bytes`, () => {
    const path = `/repo/file.${extension}`;
    const { panel, calls, source, review } = harness(false, true, path);
    panel.wasmAdapter.sourceEditorPolicy = () => "unsupported";
    const notices: unknown[] = [];
    panel.pushInAppNotification = (...args: unknown[]) => notices.push(args);
    panel.openGitDiffSource({ path, line: 3, column: 0 });
    assert.equal(panel.activeTabIndex, 0);
    assert.deepEqual(panel.bufferTabs, [review, source]);
    assert.equal((source as any).sourceCode, undefined);
    assert.equal(notices.length, 1);
    assert.equal(calls.some((call) => Array.isArray(call) && ["tabs", "activate-source", "read"].includes(call[0])), false);
  });
}

test("dirty Markdown tab metadata blocks conversion even if only a CodePane is currently retained", () => {
  const { panel, calls, source } = harness(true, true, "/repo/file.md");
  let notified = false;
  panel.pushInAppNotification = () => { notified = true; };
  panel.openGitDiffSource({ path: source.path, line: 1, column: 0 });
  assert.equal(notified, true);
  assert.equal(panel.activeTabIndex, 0);
  assert.equal((source as any).sourceCode, undefined);
  assert.equal(calls.some((call) => Array.isArray(call) && call[0] === "activate-source"), false);
});

function enableRealRead(panel: any, calls: unknown[]) {
  panel.requestFileContent = TerminalPanel.prototype["requestFileContent"];
  panel.options = { client: { sendFiles: () => calls.push("read") } };
  panel.pendingServiceMappers = new Map();
  panel.toDaemonWorkspacePath = (path: string) => path;
  panel.filesRootForPath = () => "/repo";
  panel.requestCodeHighlight = () => {};
  panel.wasmAdapter.setTabContent = () => calls.push("content");
  panel.wasmAdapter.editorOpenFile = (_index: number, path: string) => { calls.push(["loaded-code", path]); return "code"; };
  panel.renderMarkdownLayer = () => { throw new Error("raw source must never seed Markdown"); };
}

test("unloaded markdown source uses code loader, not markdown preview or placeholder", () => {
  const { panel, calls } = harness(false);
  enableRealRead(panel, calls);
  panel.openGitDiffSource({ path: "/repo/file.md", line: 2, column: 0 });
  assert.equal(panel.bufferTabs[1].sourceCode, true);
  const reply = [...panel.pendingServiceMappers.values()][0];
  reply({ FileContent: { bytes: [...new TextEncoder().encode("# one\n\ntarget")] } });
  assert.ok(calls.some((call) => Array.isArray(call) && call[0] === "loaded-code" && call[1] === "/repo/file.md"));
});

test("late source bytes never activate or move the newly selected current file", () => {
  const { panel, calls } = harness(false);
  enableRealRead(panel, calls);
  panel.openGitDiffSource({ path: "/repo/file.md", line: 25, column: 0 });
  panel.bufferTabs.push({ kind: "file", path: "/repo/current.rs" });
  panel.activeTabIndex = 2;
  const before = calls.length;
  const reply = [...panel.pendingServiceMappers.values()][0];
  assert.equal(reply({ FileContent: { bytes: [120] } }), null);
  assert.equal(panel.activeTabIndex, 2);
  assert.equal(calls.length, before);
});
