import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function harness() {
  const panel: any = Object.create(TerminalPanel.prototype);
  const reveals: Array<[number, string]> = [];
  panel.bufferTabs = [{ kind: "file", title: "draft", path: "/repo/a.rs", modified: true }];
  panel.nextGitDiffRouteId = 1;
  panel.assignActiveTabToFocusedEditorPane = () => {};
  panel.replayBufferTabs = () => {};
  panel.activateCurrentTabContents = () => {};
  panel.scheduleDraw = () => {};
  panel.wasmAdapter = {
    revealGitDiffFile: (route: number, path: string) => reveals.push([route, path]),
  };
  return { panel, reveals };
}

test("sidebar file opens share one repository review tab and explicitly reveal each file", () => {
  const { panel, reveals } = harness();
  const draft = panel.bufferTabs[0];
  panel.openGitDiffTab("/repo", "a.rs");
  const review = panel.bufferTabs[1];
  const identity = panel.tabLifecycleKey(review);
  panel.openGitDiffTab("/repo", "nested/b.md");
  assert.equal(panel.tabLifecycleKey(review), identity);
  assert.equal(panel.bufferTabs.length, 2);
  assert.equal(panel.bufferTabs[0], draft);
  assert.equal(draft.modified, true);
  assert.equal(panel.bufferTabs[1], review);
  assert.equal(panel.activeTabIndex, 1);
  assert.equal(review.title, "Uncommitted Changes");
  assert.equal(review.diffPath, "nested/b.md");
  assert.equal(review.path, undefined);
  assert.deepEqual(reveals, [[1, "a.rs"], [1, "nested/b.md"]]);
  panel.openGitDiffTab("/other", "a.rs");
  assert.equal(panel.bufferTabs.length, 3);
  assert.notEqual(panel.bufferTabs[2].diffRouteId, review.diffRouteId);
});

test("same-file explicit opens reveal again, ordinary replay does not navigate", () => {
  const { panel, reveals } = harness();
  panel.openGitDiffTab("/repo", "a.rs");
  panel.replayBufferTabs();
  assert.equal(reveals.length, 1);
  panel.openGitDiffTab("/repo", "a.rs");
  assert.deepEqual(reveals, [[1, "a.rs"], [1, "a.rs"]]);
  assert.equal(panel.bufferTabs.length, 2);
});
