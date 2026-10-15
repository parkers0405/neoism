import { test } from "node:test";
import assert from "node:assert/strict";
import { ProtocolClient } from "../workspace/ProtocolClient.ts";
import { TerminalPanel } from "./TerminalPanel.ts";
import { browserFileParent, fileBrowserRecentsKey, parseFileBrowserRecents } from "./fileBrowserHost.ts";

const tick = () => new Promise<void>((resolve) => setImmediate(resolve));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
function fixture() {
  const client = new ProtocolClient({ url: "wss://host/ws?token=secret", authToken: "secret" });
  const calls: unknown[] = [];
  const commits: unknown[] = [];
  const replies: unknown[] = [];
  const notices: unknown[] = [];
  let session: string | null = "session-a";
  let key = "agent-a";
  let requests: unknown[] = [];
  let selection: unknown = null;
  let action: unknown = null;
  let path = "/host/current";
  let active = true;
  let service = async (_message: any): Promise<any> => ({ DirListing: { entries: [] } });
  (client as any).requestFiles = (message: unknown) => { calls.push(message); return service(message); };
  const panel = Object.create(TerminalPanel.prototype) as any;
  Object.assign(panel, {
    options: { client, activeWorkspaceId: "workspace-a", workspaceRoot: "/host/root", onWorkspaceRootRequested: (...args: unknown[]) => commits.push(args) },
    bufferTabs: [{ kind: "neoism-agent", title: "Agent", neoismAgentRouteId: 1 }], activeTabIndex: 0,
    fileBrowserQueue: [], fileBrowserRequestInFlight: false, fileBrowserTarget: null,
    activeTabKey: () => key,
    scheduleDraw() {},
    pushInAppNotification: (...args: unknown[]) => notices.push(args),
    wasmAdapter: {
      openFileBrowser: (...args: unknown[]) => { replies.push(["open", ...args]); },
      agentDirectory: () => "/host/agent",
      fileBrowserPath: () => path,
      fileBrowserActive: () => active,
      agentSessionId: () => session,
      agentChangeDirectory: (...args: unknown[]) => { commits.push(args); return true; },
      agentAttachFile: (...args: unknown[]) => { commits.push(args); return true; },
      drainAgentFileBrowserAction: () => { const result = action; action = null; return result; },
      drainFileBrowserRequests: () => { const result = requests; requests = []; return result; },
      drainFileBrowserSelection: () => { const result = selection; selection = null; return result; },
      setFileBrowserLocations: (value: string) => replies.push(["locations", value]),
      setFileBrowserEntries: (...args: unknown[]) => replies.push(["entries", ...args]),
      setFileBrowserError: (error: string) => replies.push(["error", error]),
    },
  });
  return { panel, client, calls, commits, replies, notices,
    service(fn: typeof service) { service = fn; },
    requests(value: unknown[]) { requests = value; },
    selection(value: unknown) { selection = value; },
    action(value: unknown) { action = value; },
    session(value: string | null) { session = value; },
    key(value: string) { key = value; },
    path(value: string) { path = value; },
    active(value: boolean) { active = value; },
  };
}

test("recents are host-scoped, validated, and preserve Windows/Unix root parents", () => {
  const a = new ProtocolClient({ url: "wss://a/ws?token=secret" });
  const b = new ProtocolClient({ url: "wss://b/ws?token=secret" });
  assert.notEqual(fileBrowserRecentsKey(a), fileBrowserRecentsKey(b));
  assert.ok(!fileBrowserRecentsKey(a).includes("secret"));
  assert.notEqual(fileBrowserRecentsKey({ getConnectedHostId: () => "a", agentCatalogIdentity: () => "" }), fileBrowserRecentsKey({ getConnectedHostId: () => "b", agentCatalogIdentity: () => "" }));
  assert.deepEqual(parseFileBrowserRecents('[null, 1, "", "/a", "/a", "C:\\\\host"]'), ["/a", "C:\\host"]);
  assert.deepEqual(parseFileBrowserRecents("{}"), []);
  assert.deepEqual(parseFileBrowserRecents("invalid"), []);
  assert.equal(browserFileParent("/image.png"), "/");
  assert.equal(browserFileParent("C:\\image.png"), "C:\\");
  assert.equal(browserFileParent("\\\\host\\share\\image.png"), "\\\\host\\share");
});

test("composer attachment action opens the image picker directly without a chooser menu", () => {
  const f = fixture();
  f.action({ mode: "attach_image", session_id: "session-a" });
  f.panel.pumpFileBrowser();
  assert.equal(f.panel.fileBrowserTarget.kind, "agent");
  assert.deepEqual(f.replies[0], ["open", "attach_image", "/host/agent", "[]"]);
  assert.deepEqual(f.commits, []);
});

test("workspace splash/status routing opens shared directory modal, not cd palette", () => {
  const f = fixture();
  f.panel.openWorkspaceDirectoryPalette();
  assert.equal(f.panel.fileBrowserTarget.kind, "workspace");
  assert.deepEqual(f.replies[0], ["open", "choose_directory", "/host/root", "[]"]);
});

test("all drained requests survive while the first request is in flight", async () => {
  const f = fixture();
  const locations = deferred<any>();
  f.service(async (message) => message === "ListBrowserLocations" ? locations.promise : { DirListing: { entries: [] } });
  f.panel.openHostFileBrowser("workspace", "choose_directory");
  f.requests([{ kind: "load_locations" }, { kind: "list_directory", path: "/host/current" }]);
  f.panel.pumpFileBrowser();
  f.requests([{ kind: "list_directory", path: "/host/current" }]);
  f.panel.pumpFileBrowser();
  assert.equal(f.calls.length, 1);
  locations.resolve({ BrowserLocations: { locations: [] } });
  await tick();
  f.panel.pumpFileBrowser(); await tick();
  f.panel.pumpFileBrowser(); await tick();
  assert.deepEqual(f.calls, ["ListBrowserLocations", { BrowserListDir: { path: "/host/current" } }, { BrowserListDir: { path: "/host/current" } }]);
});

test("reopening discards old success/error and queued work", async () => {
  const f = fixture();
  const old = deferred<any>();
  f.service(() => old.promise);
  f.panel.openHostFileBrowser("workspace", "choose_directory");
  f.requests([{ kind: "load_locations" }, { kind: "list_directory", path: "/old" }]);
  f.panel.pumpFileBrowser();
  f.panel.openHostFileBrowser("agent", "attach_image", "session-a");
  old.resolve({ BrowserLocations: { locations: [{ path: "/old" }] } });
  await tick();
  f.panel.pumpFileBrowser();
  assert.equal(f.calls.length, 1);
  assert.ok(!f.replies.some((reply: any) => reply[0] === "locations"));
});

test("late error for an old navigation cannot poison the current directory", async () => {
  const f = fixture();
  const old = deferred<any>();
  f.service(() => old.promise);
  f.panel.openHostFileBrowser("workspace", "choose_directory");
  f.requests([{ kind: "list_directory", path: "/host/current" }]);
  f.panel.pumpFileBrowser();
  f.path("/host/new");
  old.resolve({ Error: { message: "old navigation failed" } });
  await tick();
  assert.ok(!f.replies.some((reply: any) => reply[0] === "error"));
});

test("directory selection validates stat AND listing before workspace commit", async () => {
  const f = fixture();
  f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { name: "picked", is_dir: true } } } : { DirListing: { entries: [] } });
  f.panel.openHostFileBrowser("workspace", "choose_directory");
  await f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "C:\\host\\picked");
  assert.deepEqual(f.calls, [{ BrowserStat: { path: "C:\\host\\picked" } }, { BrowserListDir: { path: "C:\\host\\picked" } }]);
  assert.deepEqual(f.commits, [["C:\\host\\picked", "workspace-a"]]);
});

test("non-directory and inaccessible directories never commit", async () => {
  for (const isDirectory of [false, true]) {
    const f = fixture();
    f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { is_dir: isDirectory } } } : { Error: { message: "denied" } });
    f.panel.openHostFileBrowser("workspace", "choose_directory");
    await f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/picked");
    assert.equal(f.commits.length, 0);
    assert.equal(f.notices.length, 1);
  }
});

test("agent cd uses captured session without modifying workspace or composer", async () => {
  const f = fixture();
  f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { is_dir: true } } } : { DirListing: { entries: [] } });
  f.action({ mode: "choose_directory", session_id: "session-a" });
  f.panel.pumpFileBrowser();
  assert.equal(f.panel.fileBrowserTarget.kind, "agent");
  await f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/picked");
  assert.deepEqual(f.commits, [["session-a", "/picked"]]);
  assert.equal(f.panel.options.workspaceRoot, "/host/root");
});

test("stale session, pane, root, workspace, client, adapter, or generation rejects async commit", async () => {
  const changes = [
    (f: ReturnType<typeof fixture>) => f.session("other"),
    (f: ReturnType<typeof fixture>) => f.key("other-pane"),
    (f: ReturnType<typeof fixture>) => { f.panel.options.workspaceRoot = "/other"; },
    (f: ReturnType<typeof fixture>) => { f.panel.options.activeWorkspaceId = "other"; },
    (f: ReturnType<typeof fixture>) => { f.panel.options.client = new ProtocolClient({ url: "wss://host/ws" }); },
    (f: ReturnType<typeof fixture>) => { f.panel.wasmAdapter = {}; },
    (f: ReturnType<typeof fixture>) => { (f.client as any).generation++; },
    (f: ReturnType<typeof fixture>) => { f.panel.fileBrowserTarget = null; },
  ];
  for (const change of changes) {
    const f = fixture();
    const listing = deferred<any>();
    f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { is_dir: true } } } : listing.promise);
    f.panel.openHostFileBrowser("agent", "choose_directory", "session-a");
    const commit = f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/picked");
    await tick();
    change(f);
    listing.resolve({ DirListing: { entries: [] } });
    await commit;
    assert.equal(f.commits.length, 0);
  }
});

test("blank agent selection updates only its tab draft and survives reattach/reactivation", async () => {
  const f = fixture();
  f.session(null);
  const tab = f.panel.bufferTabs[0];
  const other: { kind: string; title: string; neoismAgentRouteId: number; agentDraftDirectory?: string } = { kind: "neoism-agent", title: "Other", neoismAgentRouteId: 2 };
  f.panel.bufferTabs.push(other);
  let footer = "/host/agent";
  let draft = "unfinished prompt";
  const newThreads: unknown[] = [];
  const attaches: unknown[] = [];
  Object.assign(f.panel.wasmAdapter, {
    agentChangeDirectory: (sessionId: string | null, directory: string) => {
      assert.equal(sessionId, null);
      footer = directory;
      f.commits.push([sessionId, directory]);
      return true;
    },
    agentDirectory: () => footer,
    fileTreeWorkspaceRoot: () => "/host/root",
    agentNewThread: (directory: string) => { newThreads.push(directory); footer = directory; },
    agentAttach: (directory: string) => { attaches.push(directory); footer = directory; },
    agentSendMessage: () => { draft = "submitted!"; },
  });
  f.panel.agentLastAttachAt = 0;
  f.panel.presenceIdentity = { displayName: "Test" };
  f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { is_dir: true } } } : { DirListing: { entries: [] } });
  f.action({ mode: "choose_directory", session_id: null });
  f.panel.pumpFileBrowser();
  await f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/host/picked");
  assert.deepEqual(f.commits, [[null, "/host/picked"]]);
  assert.equal(tab.agentDraftDirectory, "/host/picked");
  assert.equal(other.agentDraftDirectory, undefined);
  assert.equal(footer, "/host/picked");
  assert.equal(draft, "unfinished prompt");
  assert.equal(f.panel.options.workspaceRoot, "/host/root");
  assert.deepEqual(newThreads, [], "selection must not create/submit a conversation");
  f.panel.ensureNeoismAgentAttached();
  assert.deepEqual(attaches, ["/host/picked"]);
  f.panel.activateAgentTab(other);
  assert.equal(footer, "/host/root", "another blank tab must not inherit the selection");
  f.panel.activateAgentTab(tab);
  assert.equal(footer, "/host/picked");
  assert.deepEqual(newThreads, ["/host/root", "/host/picked"]);
});

test("null-session selection is stale if a session appears during stat or listing", async () => {
  for (const stage of ["stat", "listing"]) {
    const f = fixture();
    f.session(null);
    const validation = deferred<any>();
    f.service(async (message) => {
      if ("BrowserStat" in message) return stage === "stat" ? validation.promise : { Stat: { entry: { is_dir: true } } };
      return validation.promise;
    });
    f.panel.openHostFileBrowser("agent", "choose_directory", null);
    const commit = f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/picked");
    await tick();
    f.session("new-or-switched-session");
    validation.resolve(stage === "stat" ? { Stat: { entry: { is_dir: true } } } : { DirListing: { entries: [] } });
    await commit;
    assert.equal(f.commits.length, 0);
    assert.equal(f.panel.bufferTabs[0].agentDraftDirectory, undefined);
  }
});

test("blank pane change while validating cannot update the other blank tab", async () => {
  const f = fixture();
  f.session(null);
  const validation = deferred<any>();
  f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { is_dir: true } } } : validation.promise);
  f.panel.openHostFileBrowser("agent", "choose_directory", null);
  const commit = f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/picked");
  await tick();
  f.key("other-blank-tab");
  validation.resolve({ DirListing: { entries: [] } });
  await commit;
  assert.equal(f.commits.length, 0);
  assert.equal(f.panel.bufferTabs[0].agentDraftDirectory, undefined);
});

test("stale action cannot open for a new session, and invalidated targets cannot revive", () => {
  const f = fixture();
  f.action({ mode: "choose_directory", session_id: "old-session" });
  f.panel.pumpFileBrowser();
  assert.equal(f.panel.fileBrowserTarget, null);
  f.panel.openHostFileBrowser("agent", "choose_directory", "session-a");
  const target = f.panel.fileBrowserTarget;
  f.key("other-pane");
  f.panel.pumpFileBrowser();
  f.key("agent-a");
  assert.equal(f.panel.fileBrowserTargetCurrent(target), false);
});

test("selection mode must match the captured picker intent", async () => {
  const f = fixture();
  f.panel.openHostFileBrowser("agent", "attach_image", "session-a");
  await f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "choose_directory", "/picked");
  assert.deepEqual(f.calls, []);
  assert.deepEqual(f.commits, []);
});

test("image bytes arriving after a session switch cannot attach to the new session", async () => {
  const f = fixture();
  const bytes = deferred<any>();
  f.service(async (message) => "BrowserStat" in message ? { Stat: { entry: { is_dir: false, size: 10 } } } : bytes.promise);
  f.panel.openHostFileBrowser("agent", "attach_image", "session-a");
  const commit = f.panel.commitFileBrowserSelection(f.panel.fileBrowserTarget, "attach_image", "/image.png");
  await tick();
  f.session("session-b");
  bytes.resolve({ FileContent: { bytes: [1, 2, 3] } });
  await commit;
  assert.equal(f.commits.length, 0);
});
