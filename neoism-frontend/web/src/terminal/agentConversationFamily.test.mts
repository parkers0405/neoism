import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

function harness() {
  const panel: any = Object.create(TerminalPanel.prototype);
  let viewed: string | null = "other";
  let root: string | null = "other";
  let input = "unsent";
  const switches: string[] = [];
  const focuses: number[] = [];
  let drafts = 0;
  panel.bufferTabs = [
    { kind: "neoism-agent", neoismAgentRouteId: 1, agentSessionId: "other" },
    { kind: "neoism-agent", neoismAgentRouteId: 2, agentSessionId: "family" },
  ];
  panel.activeTabIndex = 0;
  panel.nextNeoismAgentRouteId = 3;
  panel.paneTabState = new Map([[20, { tabIndices: [1], activeTabIndex: 1 }]]);
  panel.wasmAdapter = {
    agentSessionId: () => viewed,
    agentConversationRootId: () => root,
    agentSwitchThread: (id: string) => { viewed = id; switches.push(id); },
    agentNewThread: () => { viewed = null; drafts++; input = ""; },
    agentInput: () => input,
    agentSetInput: (text: string) => { input = text; },
  };
  panel.applySessionLayoutPolicy = (_action: string, _a: unknown, _b: unknown, owner: number) => focuses.push(owner);
  panel.assignActiveTabToFocusedEditorPane = () => {};
  panel.replayBufferTabs = () => {};
  panel.ensureNeoismAgentAttached = () => {};
  panel.explicitlyReopenTabKey = () => {};
  panel.focus = () => {};
  panel.scheduleDraw = () => {};
  return { panel, switches, focuses, drafts: () => drafts, input: () => input,
    setView: (id: string | null, family: string | null) => { viewed = id; root = family; } };
}

test("catalog child reuses the root's route and existing split without starting a draft", () => {
  const h = harness();
  h.panel.openAgentConversation("child", "family");
  assert.equal(h.panel.bufferTabs.length, 2);
  assert.equal(h.panel.activeTabIndex, 1);
  assert.equal(h.panel.bufferTabs[1].agentSessionId, "child");
  assert.equal(h.panel.bufferTabs[1].agentRootSessionId, "family");
  assert.deepEqual(h.focuses, [20]);
  assert.deepEqual(h.switches, ["child"]);
  assert.equal(h.drafts(), 0);
  assert.equal(h.input(), "unsent");
});

test("siblings and return-to-root stay on the family route after viewing a child", () => {
  const h = harness();
  h.panel.openAgentConversation("child", "family");
  h.setView("child", "family");
  h.panel.openAgentConversation("sibling", "family");
  h.setView("sibling", "family");
  h.panel.openAgentConversation("family", "family");
  assert.equal(h.panel.bufferTabs.length, 2);
  assert.deepEqual(h.switches, ["child", "sibling", "family"]);
});

test("Enter on the exact viewed child is explicit navigation, not a toggle or reset", () => {
  const h = harness();
  h.panel.openAgentConversation("child", "family");
  h.setView("child", "family");
  h.panel.openAgentConversation("child", "family");
  assert.deepEqual(h.switches, ["child"]);
  assert.equal(h.drafts(), 0);
});

test("a new family allocates one route without clearing the old transcript via new-thread", () => {
  const h = harness();
  h.panel.openAgentConversation("new-child", "new-family");
  assert.equal(h.panel.bufferTabs.length, 3);
  assert.equal(h.panel.bufferTabs[0].agentSessionId, "other");
  assert.equal(h.panel.bufferTabs[2].agentRootSessionId, "new-family");
  assert.equal(h.drafts(), 0);
});

test("opening a catalog family preserves and restores an unrelated unsent draft", () => {
  const h = harness();
  delete h.panel.bufferTabs[0].agentSessionId;
  h.setView(null, null);
  h.panel.openAgentConversation("child", "family");
  assert.equal(h.panel.bufferTabs[0].agentDraftInput, "unsent");
  h.panel.activateAgentTab(h.panel.bufferTabs[0]);
  assert.equal(h.input(), "unsent");
});

test("a cold child switch cannot overwrite its known family with a temporary child-as-root", () => {
  const h = harness();
  h.panel.openAgentConversation("cold-child", "new-family");
  h.setView("cold-child", "cold-child");
  h.panel.rememberActiveAgentSession();
  h.panel.openAgentConversation("sibling", "new-family");
  assert.equal(h.panel.bufferTabs.length, 3);
  assert.equal(h.panel.bufferTabs[2].agentRootSessionId, "new-family");
});

test("structured catalog drain passes exact child and owning root to route activation", () => {
  const h = harness();
  h.panel.wasmAdapter.drainConversationOpenIntent = () => ({ sessionId: "child", rootSessionId: "family" });
  h.panel.wasmAdapter.drainConversationOpen = () => { throw new Error("legacy drain consumed too"); };
  h.panel.drainConversationOpens();
  assert.equal(h.panel.activeTabIndex, 1);
  assert.equal(h.panel.bufferTabs.length, 2);
  assert.deepEqual(h.switches, ["child"]);
});
