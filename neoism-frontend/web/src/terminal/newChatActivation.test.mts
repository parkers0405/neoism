import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

// Exercise the actual host activation, without a GPU/DOM renderer. The adapter
// records when its selected body becomes keyboard-ready relative to tab replay.
for (const directory of ["/projects/scoped", "/authorized/home"]) {
  test(`New Chat selects a scoped draft before handing off keyboard focus: ${directory}`, () => {
    const panel: any = Object.create(TerminalPanel.prototype);
    const calls: string[] = [];
    let sidebarFocused = true;
    let input = "";
    let draftDirectory: string | null = null;
    panel.bufferTabs = [{ kind: "terminal", title: "Terminal" }];
    panel.activeTabIndex = 0;
    panel.nextNeoismAgentRouteId = 1;
    panel.rememberActiveAgentSession = () => calls.push("remember");
    panel.explicitlyReopenTabKey = () => {};
    panel.assignActiveTabToFocusedEditorPane = () => calls.push("assign");
    panel.replayBufferTabs = () => {
      assert.equal(panel.bufferTabs[panel.activeTabIndex].kind, "neoism-agent");
      calls.push("selected");
    };
    panel.ensureNeoismAgentAttached = () => calls.push("attached");
    panel.scheduleDraw = () => {};
    panel.focus = () => calls.push("browser-focus");
    panel.wasmAdapter = {
      showConversationsForAgent: () => calls.push("show-catalog"),
      fileTreeWorkspaceRoot: () => directory,
      agentNewThread: (root: string) => { draftDirectory = root; calls.push("draft"); },
      focusEditorInput: () => {
        assert.ok(calls.includes("selected"));
        assert.ok(calls.includes("draft"));
        sidebarFocused = false;
        calls.push("composer-focus");
      },
    };
    panel.openNeoismAgentTab();
    assert.equal(panel.bufferTabs.length, 2, "exactly one new tab");
    assert.equal(draftDirectory, directory, "execution scope is not rewritten");
    assert.equal(sidebarFocused, false);
    assert.deepEqual(calls.slice(-2), ["composer-focus", "browser-focus"]);
    // Model the next browser text event: the catalog must no longer capture it.
    if (!sidebarFocused) input += "ready";
    assert.equal(input, "ready");
  });
}

for (const failed of [false, true]) {
  test(`${failed ? "failed" : "late"} async creation reply never steals focus from the new draft`, () => {
    const panel: any = Object.create(TerminalPanel.prototype);
    const active = { kind: "neoism-agent", neoismAgentRouteId: 2, agentDraftDirectory: "/new", agentSessionId: undefined };
    const origin = { kind: "neoism-agent", neoismAgentRouteId: 1, agentSessionId: undefined };
    panel.bufferTabs = [origin, active];
    panel.activeTabIndex = 1;
    panel.pendingAgentSessionRouteId = 1;
    panel.syncAgentCatalogEndpoint = () => {};
    panel.scheduleDraw = () => {};
    let restored = false;
    let delivered: any;
    panel.ensureNeoismAgentAttached = () => {};
    panel.focus = () => assert.fail("reply must not take browser focus");
    panel.wasmAdapter = {
      agentNewThread: (directory: string) => { assert.equal(directory, "/new"); restored = true; },
      focusEditorInput: () => assert.fail("reply must not take composer focus"),
      agentEvent: (raw: string) => { delivered = JSON.parse(raw); },
    };
    const payload = failed ? { Error: { message: "Could not create session: offline" } }
      : { ThreadCreated: { session_id: "created-for-old-route" } };
    panel.agentReply(payload);
    assert.deepEqual(delivered, payload, "failure remains visible through the normal agent UI");
    assert.equal(panel.activeTabIndex, 1);
    assert.equal(panel.bufferTabs[1], active);
    assert.equal(active.agentSessionId, undefined);
    if (!failed) {
      assert.equal(origin.agentSessionId, "created-for-old-route");
      assert.equal(restored, true);
    }
  });
}
