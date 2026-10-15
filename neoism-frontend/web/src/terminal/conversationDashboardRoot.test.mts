import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { AuthorizedDashboardRootCache, projectNavigatorRows, isObservedFilesystemRoot } from "./conversationNavigatorHost.ts";
import { TerminalPanel } from "./TerminalPanel.ts";
import { ProtocolClient } from "../workspace/ProtocolClient.ts";

const tick = () => new Promise<void>((resolve) => setImmediate(resolve));

test("ProjectA without advertised home disables All projects but permits saved project rows", () => {
  const cache = new AuthorizedDashboardRootCache();
  const binding = cache.bind({}, "endpoint-a", "/work/ProjectA");
  const rows = projectNavigatorRows([{ id: "p", name: "ProjectB", path: "/work/ProjectB", last_opened: 0 }], "/work/ProjectA", binding.root);
  assert.equal(binding.root, null);
  assert.equal(rows[0].is_disabled, true);
  assert.equal(rows[0].value, "");
  assert.equal(rows[1].is_disabled, false);
});

test("fallback accepts only an observed filesystem root, never a home-looking project path", () => {
  for (const root of ["/work/ProjectA", "/home/alice", "/Users/alice", "C:\\Users\\alice"]) {
    assert.equal(isObservedFilesystemRoot(root), false);
    assert.equal(new AuthorizedDashboardRootCache().bind({}, "a", root).root, null);
  }
  for (const root of ["/", "C:/", "D:\\"]) {
    assert.equal(isObservedFilesystemRoot(root), true);
    assert.equal(new AuthorizedDashboardRootCache().bind({}, "a", root).root, root);
  }
});

test("advertised home survives root changes and reconnects of the same client identity", () => {
  const cache = new AuthorizedDashboardRootCache();
  const client = {};
  const binding = cache.bind(client, "a", "/work/ProjectA");
  cache.acceptHome(binding, "/home/alice");
  assert.equal(cache.bind(client, "a", "/work/ProjectB"), binding);
  assert.equal(cache.bind(client, "a", "/home/alice").root, "/home/alice");
});

test("endpoint/auth identity and client changes clear home permission cache", () => {
  const cache = new AuthorizedDashboardRootCache();
  const client = {};
  const old = cache.bind(client, "a", "/work/ProjectA");
  cache.acceptHome(old, "/home/alice");
  const changed = cache.bind(client, "b", "/work/ProjectA");
  assert.equal(changed.root, null);
  assert.equal(cache.acceptHome(old, "/stale/home"), false);
  cache.acceptHome(changed, "/authorized/home-b");
  assert.equal(cache.bind({}, "b", "/work/ProjectA").root, null);
});

function fixture(root: string, locations: unknown[] = []) {
  const client = new ProtocolClient({ url: "wss://authorized/ws" });
  const initialState = client.getConnectionState();
  client.getConnectionState = () => ({ ...initialState, phase: "connected", generation: client.getGeneration() });
  const sent: unknown[] = [];
  let requests = 0;
  let request: () => Promise<any> = async () => ({ BrowserLocations: { locations } });
  client.requestFiles = async (message) => { assert.equal(message, "ListBrowserLocations"); requests++; return request(); };
  const panel = Object.create(TerminalPanel.prototype) as any;
  Object.assign(panel, { options: { client, workspaceRoot: root, activeWorkspaceId: "wa" }, conversationNavigation: null,
    scheduleDraw() {}, wasmAdapter: {
      agentSetCatalogEndpoint: (endpoint: string) => sent.push(["endpoint", endpoint]),
      conversationNavigatorSetDashboardRoot: (home: string | null) => sent.push(["dashboard", home]),
      setWorkspaceRoot: (path: string) => sent.push(["root", path]),
    },
  });
  return { panel, client, sent, requests: () => requests, setRequest(value: () => Promise<any>) { request = value; } };
}

test("initial authorized home is sent to WASM before Projects menu is ever opened, once per binding", async () => {
  const f = fixture("/home/alice", [{ kind: "home", label: "Home", path: "/home/alice" }]);
  f.panel.syncAgentCatalogEndpoint();
  assert.equal(f.requests(), 1);
  assert.deepEqual(f.sent.at(-1), ["dashboard", null]);
  await tick();
  assert.deepEqual(f.sent.at(-1), ["dashboard", "/home/alice"]);
  for (let i = 0; i < 5; i++) f.panel.syncAgentCatalogEndpoint();
  f.panel.options.workspaceRoot = "/work/ProjectB";
  (f.client as any).generation += 1;
  f.panel.syncAgentCatalogEndpoint();
  assert.equal(f.requests(), 1, "re-root/reconnect must reuse confirmed permission, not re-fetch");
  assert.deepEqual(f.sent.at(-1), ["dashboard", "/home/alice"]);
});

test("unavailable home never falls back to arbitrary ProjectA or loops requests", async () => {
  const f = fixture("/work/ProjectA");
  f.setRequest(async () => { throw new Error("unavailable"); });
  f.panel.syncAgentCatalogEndpoint();
  await tick();
  for (let i = 0; i < 10; i++) f.panel.syncAgentCatalogEndpoint();
  assert.equal(f.requests(), 1);
  assert.equal(f.panel.syncAuthorizedDashboardRoot(), null);
  assert.deepEqual(f.sent.at(-1), ["dashboard", null]);
});

test("endpoint change clears WASM permission immediately and ignores old authorized home reply", async () => {
  const f = fixture("/work/ProjectA");
  let resolve!: (value: any) => void;
  f.setRequest(() => new Promise((r) => { resolve = r; }));
  f.panel.syncAgentCatalogEndpoint();
  f.panel.options.activeWorkspaceId = "wb";
  f.setRequest(async () => ({ BrowserLocations: { locations: [] } }));
  f.panel.syncAgentCatalogEndpoint();
  assert.deepEqual(f.sent.at(-1), ["dashboard", null]);
  resolve({ BrowserLocations: { locations: [{ kind: "home", label: "Home", path: "/old/home" }] } });
  await tick();
  assert.equal(f.requests(), 2);
  assert.equal(f.sent.some((value: any) => value[0] === "dashboard" && value[1] === "/old/home"), false);
  assert.equal(f.panel.syncAuthorizedDashboardRoot(), null);
});

test("dashboard discovery waits for authorized transport instead of caching startup socket-not-open", async () => {
  const f = fixture("/home/alice", [{ kind: "home", label: "Home", path: "/home/alice" }]);
  const connected = f.client.getConnectionState;
  f.client.getConnectionState = () => ({ ...connected(), phase: "authenticating" });
  f.panel.syncAgentCatalogEndpoint();
  assert.equal(f.requests(), 0);
  f.client.getConnectionState = connected;
  f.panel.syncAgentCatalogEndpoint();
  await tick();
  assert.equal(f.requests(), 1);
  assert.deepEqual(f.sent.at(-1), ["dashboard", "/home/alice"]);
});

test("actual WASM set_workspace_root uses the dashboard-aware shared scope method", () => {
  // Paired with executable shared Chrome scope tests; this pins the production
  // WASM setter wiring without requiring WebGPU construction in Node.
  const source = readFileSync(new URL("../../../wasm/src/rendered/file_tree_workspace.rs", import.meta.url), "utf8");
  const setter = source.split("pub fn set_workspace_root(")[1].split("pub fn set_active_workspace_id(")[0];
  assert.match(setter, /self\.chrome\.set_conversation_catalog_workspace_root\(&root\)/);
  assert.doesNotMatch(setter, /set_catalog_directory\(workspace_catalog_directory/);
  const bridge = readFileSync(new URL("../../../wasm/src/rendered/conversation_navigator.rs", import.meta.url), "utf8");
  assert.match(bridge, /pub fn conversation_navigator_set_dashboard_root/);
});
