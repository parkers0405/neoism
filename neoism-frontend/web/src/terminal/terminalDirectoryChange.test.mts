import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";
import { ProtocolClient } from "../workspace/ProtocolClient.ts";

function fixture() {
  const panel = Object.create(TerminalPanel.prototype) as any;
  const requests: any[] = [], commits: any[] = [], writes: any[] = [], notices: any[] = [];
  let draft = "cargo locate-project";
  let submits = 0;
  Object.assign(panel, {
    options: {
      activeWorkspaceId: "workspace-a", workspaceRoot: "/old", sessionId: "primary",
      client: { getGeneration: () => 1, agentCatalogIdentity: () => "host-a", changeTerminalDirectory: (...args: any[]) => { requests.push(args); return true; }, sendInput: (...args: any[]) => writes.push(args) },
    },
    bufferTabs: [{ kind: "terminal", sessionId: "primary" }, { kind: "neoism-agent" }], activeTabIndex: 1,
    transportAvailable: true, directoryChanges: new Map(), latestDirectoryRequests: new Map(), terminalSessionCwds: new Map(),
    scheduleDraw() {}, notifyPaletteUnavailable: (...args: any[]) => notices.push(args),
    wasmAdapter: {
      terminalInput: () => draft,
      terminalSubmitPayload: () => { submits++; draft = ""; return new TextEncoder().encode("cargo locate-project\n"); },
      terminalInputInsert: (text: string) => { draft += text; },
      setActiveTerminalCwd: (cwd: string) => commits.push(["prefix", cwd]),
    },
  });
  panel.options.onWorkspaceRootRequested = (path: string, workspaceId: string) => panel.requestTerminalDirectoryChange(path, workspaceId, (actual: string) => commits.push([workspaceId, actual]));
  return { panel, requests, commits, writes, notices, draft: () => draft, submits: () => submits };
}

test("actual workspace picker intent requests primary host PTY from Agent focus, not metadata", () => {
  const f = fixture();
  const path = "/host/a 'quoted' dir";
  f.panel.syncAgentCatalogEndpoint = () => {};
  f.panel.conversationNavigation = {
    adapter: f.panel.wasmAdapter, client: f.panel.options.client,
    identity: "host-a", generation: 1, workspaceId: "workspace-a", root: "/old",
    mode: "projects", allowedProjects: new Set([path]), discovery: null,
  };
  f.panel.wasmAdapter.drainConversationNavigatorAction = () => ({ kind: "activate", value: path });
  f.panel.pumpConversationNavigator();
  assert.equal(f.requests.length, 1);
  assert.deepEqual(f.requests[0].slice(0, 2), ["primary", "/host/a 'quoted' dir"]);
  assert.deepEqual(f.commits, []);
  f.panel.activeTabIndex = 0;
  f.panel.routeTerminalComposerInput(Uint8Array.of(13));
  f.panel.sendPtyInput(new TextEncoder().encode("cargo locate-project\n"));
  assert.equal(f.submits(), 0);
  assert.equal(f.draft(), "cargo locate-project");
  assert.deepEqual(f.writes, []);
  f.panel.terminalDirectoryChanged("primary", f.requests[0][2], "/host/actual");
  assert.deepEqual(f.commits, [["prefix", "/host/actual"], ["workspace-a", "/host/actual"]]);
  assert.equal(f.panel.directorySubmissionBlocked(), false);
});

test("failure preserves draft and blocks old-cwd commands until explicit cancellation", () => {
  const f = fixture();
  f.panel.options.onWorkspaceRootRequested("/picked", "workspace-a");
  f.panel.activeTabIndex = 0;
  f.panel.terminalDirectoryChangeFailed("primary", f.requests[0][2], "Safe live control unavailable");
  f.panel.routeTerminalComposerInput(Uint8Array.of(13));
  assert.equal(f.submits(), 0);
  assert.equal(f.draft(), "cargo locate-project");
  assert.deepEqual(f.commits, []);
  assert.ok(f.notices[0][0].includes("Ctrl+C"));
  f.panel.routeInputBytesToChrome = () => false;
  f.panel.activeSurface = () => "terminal";
  f.panel.handleInputBytes(Uint8Array.of(3));
  assert.equal(f.panel.directorySubmissionBlocked(), true);
  assert.deepEqual(f.requests[1], ["primary", "", f.requests[0][2]]);
  f.panel.terminalDirectoryChangeFailed("primary", f.requests[0][2], "Directory request cancelled");
  assert.equal(f.panel.directorySubmissionBlocked(), false);
  assert.equal(f.draft(), "cargo locate-project");
  f.panel.terminalDirectoryChanged("primary", f.requests[0][2], "/late");
  assert.deepEqual(f.commits, []);
});

test("blocked pre-WASM fallback Enter preserves the unsent draft", () => {
  const f = fixture();
  f.panel.options.onWorkspaceRootRequested("/picked", "workspace-a");
  f.panel.activeTabIndex = 0;
  f.panel.wasmAdapter = null;
  f.panel.terminalInput = "cargo locate-project";
  f.panel.mirrorTerminalInput(Uint8Array.of(13));
  f.panel.sendPtyInput(Uint8Array.of(13));
  assert.equal(f.panel.terminalInput, "cargo locate-project");
  assert.deepEqual(f.writes, []);
});

test("stale A replies, peer sessions and switched workspace cannot commit or steal focus", () => {
  const f = fixture();
  f.panel.options.onWorkspaceRootRequested("/a", "workspace-a");
  f.panel.options.onWorkspaceRootRequested("/b", "workspace-a");
  f.panel.terminalDirectoryChanged("primary", f.requests[0][2], "/a");
  f.panel.terminalDirectoryChanged("peer", f.requests[1][2], "/b");
  assert.deepEqual(f.commits, []);
  assert.equal(f.panel.activeTabIndex, 1);
  f.panel.options.activeWorkspaceId = "workspace-b";
  f.panel.terminalDirectoryChanged("primary", f.requests[1][2], "/b");
  assert.deepEqual(f.commits, []);
  assert.equal(f.panel.activeTabIndex, 1);
});

test("shell-local cd never publishes a workspace root; fresh terminal uses selected root", () => {
  const f = fixture();
  f.panel.activeTabIndex = 0;
  f.panel.setTerminalCwd("primary", "/local-only");
  assert.deepEqual(f.requests, []);
  f.panel.options.workspaceRoot = "/selected";
  f.panel.options.pty = { spawn: (args: any) => f.requests.push(args) };
  f.panel.pendingTerminalTabSpawns = [];
  f.panel.spawnTerminalTab({});
  assert.equal((f.requests as any[])[0].cwd, "/selected");
});

test("host generation, client, identity and root changes invalidate pending success", () => {
  for (const invalidate of [
    (f: ReturnType<typeof fixture>) => { f.panel.options.client.getGeneration = () => 2; },
    (f: ReturnType<typeof fixture>) => { f.panel.options.client = { ...f.panel.options.client }; },
    (f: ReturnType<typeof fixture>) => { f.panel.options.client.agentCatalogIdentity = () => "host-b"; },
    (f: ReturnType<typeof fixture>) => { f.panel.options.workspaceRoot = "/peer-root"; },
  ]) {
    const f = fixture();
    f.panel.options.onWorkspaceRootRequested("/a", "workspace-a");
    invalidate(f);
    f.panel.terminalDirectoryChanged("primary", f.requests[0][2], "/a");
    assert.deepEqual(f.commits, []);
    f.panel.activeTabIndex = 0;
    assert.equal(f.panel.directorySubmissionBlocked(), true);
  }
});

test("explicit shell appcd stays separate from ambient cwd and picker requests", () => {
  const f = fixture();
  f.panel.handleTerminalEffects([{ type: "ChangeTerminalDirectory", path: "/shell-explicit" }], "primary", true);
  assert.deepEqual(f.commits, []);
  assert.deepEqual(f.requests[0].slice(0, 2), ["primary", "/shell-explicit"]);
  f.panel.handleTerminalEffects([{ type: "PtyWrite", bytes: [13] }], "primary", true);
  assert.deepEqual(f.writes, []);
});

test("protocol carries exact case-sensitive request and correlated replies", () => {
  const replies: any[] = [], sent: string[] = [];
  const client = new ProtocolClient({ url: "wss://host/ws" }, {
    onTerminalDirectoryChanged: (...args) => replies.push(args),
    onTerminalDirectoryChangeFailed: (...args) => replies.push(args),
  }) as any;
  client.authenticated = true;
  client.socket = { readyState: 1, send: (s: string) => sent.push(s) };
  client.changeTerminalDirectory("host-session", "/quoted 'dir'", "generation-b");
  assert.deepEqual(JSON.parse(sent[0]), { ChangeTerminalDirectory: { session_id: "host-session", directory: "/quoted 'dir'", request_id: "generation-b" } });
  client.dispatchServerMessage({ TerminalDirectoryChanged: { session_id: "host-session", request_id: "generation-b", directory: "/actual" } });
  client.dispatchServerMessage({ TerminalDirectoryChangeFailed: { session_id: "host-session", request_id: "generation-a", message: "busy" } });
  assert.deepEqual(replies, [["host-session", "generation-b", "/actual"], ["host-session", "generation-a", "busy"]]);
});
