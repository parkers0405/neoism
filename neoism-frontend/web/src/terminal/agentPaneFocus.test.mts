import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function focusHarness() {
  const panel: any = Object.create(TerminalPanel.prototype);
  const calls: string[] = [];
  let viewedSession: string | null = "a";
  panel.bufferTabs = [
    { kind: "neoism-agent", title: "A", neoismAgentRouteId: 1, agentSessionId: "a" },
    { kind: "neoism-agent", title: "B", neoismAgentRouteId: 2, agentSessionId: "b" },
    { kind: "file", title: "file.rs", path: "file.rs" },
  ];
  panel.activeTabIndex = 0;
  panel.paneTabState = new Map([
    [10, { activeTabIndex: 0 }],
    [20, { activeTabIndex: 1 }],
    [30, { activeTabIndex: 2 }],
  ]);
  panel.paneSessionIds = new Map();
  panel.wasmAdapter = {
    setActiveTab: (index: number) => calls.push(`tab:${index}`),
    agentSessionId: () => viewedSession,
    agentSwitchThread: (id: string) => {
      viewedSession = id;
      calls.push(`agent:${id}`);
    },
    agentNewThread: () => calls.push("new-draft"),
  };
  panel.syncActiveTabModified = () => {};
  panel.ensureNeoismAgentAttached = () => {};
  panel.replayBufferTabs = () => {};
  panel.openFileTabContent = (path: string) => calls.push(`file:${path}`);
  panel.bindEditorSurfaceForTab = (_pane: number, index: number) => calls.push(`editor:${index}`);
  return { panel, calls, viewed: () => viewedSession, setViewed: (id: string | null) => { viewedSession = id; } };
}

test("split focus restores the conversation actually bound to each agent pane", () => {
  const h = focusHarness();
  h.panel.activatePaneExternalId(20, true);
  assert.equal(h.panel.activeTabIndex, 1);
  assert.equal(h.viewed(), "b");
  h.panel.activatePaneExternalId(10, true);
  assert.equal(h.viewed(), "a");
  assert.deepEqual(h.calls.filter((call) => call.startsWith("agent:")), ["agent:b", "agent:a"]);
  assert.ok(!h.calls.some((call) => call.startsWith("editor:")));
});

test("leaving a child view remembers it on its buffer route before switching", () => {
  const h = focusHarness();
  h.setViewed("child-of-a");
  h.panel.activatePaneExternalId(20, true);
  assert.equal(h.panel.bufferTabs[0].agentSessionId, "child-of-a");
  h.panel.activatePaneExternalId(10, true);
  assert.equal(h.viewed(), "child-of-a");
});

test("refocusing an already viewed conversation or draft does not reset it", () => {
  const h = focusHarness();
  h.panel.activatePaneExternalId(10, true);
  assert.ok(!h.calls.some((call) => call.startsWith("agent:")));
  delete h.panel.bufferTabs[0].agentSessionId;
  h.setViewed(null);
  h.panel.activatePaneExternalId(10, true);
  assert.ok(!h.calls.includes("new-draft"));
});

test("file focus stays a file and agent fallback restores its transcript", () => {
  const h = focusHarness();
  h.panel.activatePaneExternalId(30, true);
  assert.equal(h.panel.activeTabIndex, 2);
  assert.ok(h.calls.includes("file:file.rs"));
  assert.ok(!h.calls.some((call) => call.startsWith("agent:")));
  h.setViewed("b");
  h.panel.activatePaneExternalId(99, true);
  assert.equal(h.panel.activeTabIndex, 0);
  assert.equal(h.viewed(), "a");
  assert.ok(h.calls.includes("agent:a"));
});
