import { test } from "node:test";
import assert from "node:assert/strict";
import { conversationSearchRows, projectNavigatorRows, navigatorContextCurrent, NavigatorProjectDiscovery, mergeNavigatorProjects, ADD_NAVIGATOR_WORKSPACE, canAddNavigatorWorkspace, ProjectRegistryHttpError, AuthorizedDashboardRootCache, type NavigatorProjectDiscoveryState } from "./conversationNavigatorHost.ts";
import { TerminalPanel } from "./TerminalPanel.ts";
import { ProtocolClient } from "../workspace/ProtocolClient.ts";

const project = (id: string) => ({ name: id, path: `/projects/${id}`, id, last_opened: 0 });

function discoveryFixture(registry: Promise<ReturnType<typeof project>[]>) {
  const requests: any[] = [];
  const updates: NavigatorProjectDiscoveryState[] = [];
  let generation = 1;
  const client = {
    requestProjectRoots: () => registry,
    sendAgent: (id: number, payload: unknown) => requests.push({ id, payload }),
    allocateRequestId: () => 7,
    agentCatalogIdentity: () => "authorized-endpoint",
    getGeneration: () => generation,
  };
  const discovery = new NavigatorProjectDiscovery(client, "same-workspace", (state) => updates.push(state));
  discovery.start();
  const query = requests[0].payload.ListSessionProjects;
  return { discovery, updates, requests, query, changeConnection: () => generation++,
    summary: (items: Array<{ directory: string; updatedMs: number }>) => discovery.accept({ SessionProjects: { ...query, items } }),
    failure: () => discovery.accept({ SessionProjectsFailed: { ...query, message: "denied" } }),
  };
}

test("root discovery unions registry with no-history projects and old chat-only dirs without recent scans", async () => {
  const f = discoveryFixture(Promise.resolve([project("no-history"), { ...project("busy"), name: "Registry title" }]));
  try {
    // Endpoint summary already groups >1000 busy-project conversations; there
    // must be no recent-session request that can starve the old directory.
    assert.deepEqual(Object.keys(f.requests[0].payload), ["ListSessionProjects"]);
    assert.deepEqual(Object.keys(f.query).sort(), ["catalog_identity", "generation"]);
    f.summary([{ directory: "/projects/busy/", updatedMs: 999999 }, { directory: "/old-chat-only", updatedMs: 1 }]);
    await new Promise((resolve) => setImmediate(resolve));
    const state = f.updates.at(-1)!;
    assert.equal(state.registry, "ready");
    assert.equal(state.chats, "ready");
    const rows = projectNavigatorRows(state.projects, "/authorized/home", "/authorized/home");
    assert.deepEqual(new Set(rows.map((r) => r.value)), new Set(["/authorized/home", "/projects/no-history", "/projects/busy", "/old-chat-only", ADD_NAVIGATOR_WORKSPACE]));
    assert.equal(rows[0].is_current, true);
    assert.equal(rows.find((r) => r.value === "/projects/busy")?.title, "Registry title");
  } finally { f.discovery.close(); }
});

test("project discovery retains each successful authorized source when the other fails", async () => {
  for (const registryFails of [false, true]) {
    const f = discoveryFixture(registryFails ? Promise.reject(new Error("registry unavailable")) : Promise.resolve([project("empty")]));
    try {
      if (registryFails) f.summary([{ directory: "/chat-only", updatedMs: 1 }]);
      else f.failure();
      await new Promise((resolve) => setImmediate(resolve));
      const state = f.updates.at(-1)!;
      assert.deepEqual(state.projects.map((p) => p.path), [registryFails ? "/chat-only" : "/projects/empty"]);
      assert.equal(state[registryFails ? "registry" : "chats"], "error");
      assert.match(state.errors[registryFails ? "registry" : "chats"]!, /unavailable|denied/);
    } finally { f.discovery.close(); }
  }
});

test("reopening retries an initializing summary instead of caching only the registry subset", async () => {
  const first = discoveryFixture(Promise.resolve([project("no-history")]));
  first.discovery.accept({ SessionProjectsFailed: { ...first.query, message: "503 index initializing" } });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(first.updates.at(-1)!.projects.map((p) => p.path), ["/projects/no-history"]);
  assert.equal(first.requests.length, 1);
  first.discovery.close();
  assert.equal(first.summary([{ directory: "/stale", updatedMs: 0 }]), false);
  const reopened = discoveryFixture(Promise.resolve([project("no-history")]));
  try {
    assert.equal(reopened.requests.length, 1);
    reopened.summary([{ directory: "/old-chat-only", updatedMs: 1 }]);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(reopened.updates.at(-1)!.projects.map((p) => p.path), ["/projects/no-history", "/old-chat-only"]);
    assert.deepEqual(reopened.updates.at(-1)!.errors, {});
  } finally { reopened.discovery.close(); }
});

test("typed optional management 404 is unavailable, not a registry error", async () => {
  const f = discoveryFixture(Promise.reject(new ProjectRegistryHttpError(404, "management routes disabled")));
  try {
    f.summary([{ directory: "/authorized-history", updatedMs: 1 }]);
    await new Promise((resolve) => setImmediate(resolve));
    const state = f.updates.at(-1)!;
    assert.equal(state.registry, "unavailable");
    assert.equal(state.chats, "ready");
    assert.deepEqual(state.errors, {});
    assert.deepEqual(state.projects.map((p) => p.path), ["/authorized-history"]);
  } finally { f.discovery.close(); }
});

test("TerminalPanel optional registry 404 stays quiet across reopen and Add workspace still routes", async () => {
  const f = projectHostFixture(() => Promise.reject(new ProjectRegistryHttpError(404, "management disabled")));
  try {
    for (let open = 0; open < 3; open++) {
      f.open(); f.reply();
      await flushNavigatorHost();
      assert.equal(f.notices.length, 0);
      assert.ok(f.rows.at(-1)!.some((row) => row.value === "/chat-only"));
      assert.ok(f.rows.at(-1)!.some((row) => row.value === ADD_NAVIGATOR_WORKSPACE));
      f.dismiss();
    }
    f.open(); f.reply();
    await flushNavigatorHost();
    f.activate(ADD_NAVIGATOR_WORKSPACE);
    assert.deepEqual(f.browsers, [["workspace", "choose_directory"]]);
    assert.deepEqual(f.commits, []);
  } finally { f.close(); }
});

test("registry auth, server, network, and string-only 404 errors remain visible", async () => {
  for (const error of [new ProjectRegistryHttpError(401, "unauthorized"), new ProjectRegistryHttpError(403, "forbidden"),
    new ProjectRegistryHttpError(503, "service unavailable"), new Error("network disconnected"), new Error("HTTP 404 wrong URL")]) {
    const f = projectHostFixture(() => Promise.reject(error));
    try {
      f.open(); f.reply();
      await flushNavigatorHost();
      assert.equal(f.notices.length, 1);
      assert.equal(f.notices[0][0], "Project registry");
      assert.ok(f.rows.at(-1)!.some((row) => row.value === "/chat-only"));
    } finally { f.close(); }
  }
  const f = projectHostFixture(() => Promise.reject(new ProjectRegistryHttpError(404, "management disabled")));
  try {
    f.open(); f.fail();
    await flushNavigatorHost();
    assert.equal(f.notices.length, 1);
    assert.equal(f.notices[0][0], "Saved chat projects");
  } finally { f.close(); }
});

test("summary 404 still reports a wrong endpoint when optional management is absent", async () => {
  const f = projectHostFixture(() => Promise.reject(new ProjectRegistryHttpError(404, "management disabled")));
  try {
    f.open();
    f.panel.agentReply({ SessionProjectsFailed: { ...f.query(), message: "HTTP 404 wrong agent endpoint" } });
    await flushNavigatorHost();
    assert.equal(f.notices.length, 1);
    assert.equal(f.notices[0][0], "Saved chat projects");
  } finally { f.close(); }
});

test("both source failures report separately and do not invent fallback directories", async () => {
  const f = discoveryFixture(Promise.reject(new Error("registry denied")));
  try {
    f.failure();
    await new Promise((resolve) => setImmediate(resolve));
    const state = f.updates.at(-1)!;
    assert.deepEqual(state.projects, []);
    assert.equal(state.registry, "error");
    assert.equal(state.chats, "error");
    assert.deepEqual(Object.keys(state.errors).sort(), ["chats", "registry"]);
  } finally { f.discovery.close(); }
});

test("project discovery rejects stale summary identity/generation and connection changes", async () => {
  const f = discoveryFixture(Promise.resolve([]));
  try {
    assert.equal(f.discovery.accept({ SessionProjects: { ...f.query, catalog_identity: "other", items: [] } }), false);
    assert.equal(f.discovery.accept({ SessionProjects: { ...f.query, generation: f.query.generation + 1, items: [] } }), false);
    f.changeConnection();
    assert.equal(f.summary([{ directory: "/stale", updatedMs: 1 }]), false);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(f.updates.length, 0);
  } finally { f.discovery.close(); }
});

test("directory union never truncates large authorized summaries or resolves parent segments", () => {
  const items = Array.from({ length: 1200 }, (_, i) => ({ directory: `/authorized/${i}`, updatedMs: i }));
  items.push({ directory: "/very-old", updatedMs: 0 });
  const projects = mergeNavigatorProjects([], items);
  assert.equal(projects.length, 1201);
  assert.ok(projects.some((p) => p.path === "/very-old"));
});

test("declared current root is retained without guessing home or scanning recent sessions", () => {
  const rows = projectNavigatorRows([], "/declared-current", null);
  assert.equal(rows[0].is_disabled, true);
  assert.equal(rows[1].value, "/declared-current");
  assert.equal(rows[1].is_current, true);
  assert.equal(rows.some((r) => r.value === ADD_NAVIGATOR_WORKSPACE), false);
  assert.deepEqual(mergeNavigatorProjects([], [{ directory: "", updatedMs: 0 }, { directory: "relative", updatedMs: 0 }]), []);
});

test("Add workspace is a root-only dropdown action with no folder-plus dependency", () => {
  for (const [root, home, enabled] of [
    ["/", null, true], ["/authorized/home/", "/authorized/home", true],
    ["/project", "/authorized/home", false], [null, null, false],
  ] as const) {
    assert.equal(canAddNavigatorWorkspace(root, home), enabled);
    const rows = projectNavigatorRows([], root, home);
    assert.equal(rows.some((r) => r.value === ADD_NAVIGATOR_WORKSPACE), enabled);
    if (enabled) assert.equal(rows.at(-1)?.title, "Add workspace…");
  }
});

test("global search combines title/content metadata without visible catalog lookup", () => {
  const rows = conversationSearchRows({ items: [{ id: "a", title: "Chat", directory: "/saved/a", source: "codex" }] }, { hits: [
    { session_id: "a", excerpt: "message needle", message_id: "m1", role: "user" },
    { session_id: "b", title: "Outside catalog", directory: "/saved/b", excerpt: "other", externalProvider: "gemini", distance: 0 },
  ] });
  assert.equal(rows.length, 2);
  assert.equal(rows[0].directory, "/saved/a");
  assert.equal(rows[0].description, "message needle");
  assert.equal(rows[0].footer, "/saved/a · codex · user");
  assert.equal(rows[0].metadata.message_id, "m1");
  assert.equal(rows[1].directory, "/saved/b");
  assert.match(rows[1].footer, /gemini/);
});

test("project choices are persisted directories, independent of workspace grids and chats", () => {
  const saved = [project("empty-project")];
  const rows = projectNavigatorRows(saved, "/projects/empty-project", null);
  assert.equal(rows[0].is_disabled, true);
  assert.equal(rows[0].value, "");
  assert.deepEqual(rows.slice(1).map((r) => r.value), ["/projects/empty-project"]);
  assert.equal(rows[1].is_current, true);
  const home = projectNavigatorRows(saved, "/projects/empty-project", "/authorized/home")[0];
  assert.equal(home.value, "/authorized/home");
  assert.equal(home.is_disabled, false);
});

test("project registry normalization keeps dashboard separate and current first", () => {
  const rows = projectNavigatorRows([
    { ...project("z"), name: "Zulu" },
    { ...project("b"), name: "beta", path: "/projects/b/" },
    { ...project("b"), name: "beta", path: "/projects/b/./" },
    { ...project("a"), name: "Alpha" },
    { ...project("home"), path: "/authorized/home/" },
  ], "/projects/z/", "/authorized/home");
  assert.deepEqual(rows.map((r) => r.value), ["/authorized/home", "/projects/z", "/projects/a", "/projects/b/"]);
  assert.equal(rows[0].title, "~");
  assert.equal(rows[0].footer, "Dashboard");
  assert.equal(rows[1].is_current, true);
  const unavailable = projectNavigatorRows([], null, null)[0];
  assert.equal(unavailable.is_disabled, true);
  assert.equal(unavailable.footer, "Unavailable");
  assert.match(unavailable.description, /unavailable/);
});

test("normalization is lexical and retains authorized activation paths", () => {
  const rows = projectNavigatorRows([
    { ...project("win"), path: "C:\\work\\repo\\" },
    { ...project("win"), path: "C:/work/repo" },
    { ...project("parent"), path: "/projects/a/../b" },
    { ...project("b"), path: "/projects/b" },
  ], "C:/work/repo/.", "/");
  assert.equal(rows.length, 4);
  assert.equal(rows[0].value, "/");
  assert.equal(rows[1].is_current, true);
  assert.ok(rows.some((r) => r.value === "/projects/a/../b"));
  assert.ok(rows.some((r) => r.value === "/projects/b"));
});

test("context guard rejects client, auth identity, generation and workspace changes", () => {
  const a = { client: {}, identity: "auth-a", generation: 1, workspaceId: "wa" };
  assert.equal(navigatorContextCurrent(a, { ...a }), true);
  for (const patch of [{ client: {} }, { identity: "auth-b" }, { generation: 2 }, { workspaceId: "wb" }]) {
    assert.equal(navigatorContextCurrent(a, { ...a, ...patch }), false);
  }
});

function projectHostFixture(registry: () => Promise<ReturnType<typeof project>[]> = () => Promise.resolve([project("no-history")])) {
  let generation = 1, identity = "authorized-endpoint", active = false;
  let header: string | null = null;
  let action: { kind: string; value?: string } | null = null;
  let registryCalls = 0;
  const sent: any[] = [], rows: any[][] = [], commits: unknown[] = [], browsers: unknown[] = [], notices: unknown[][] = [], forwarded: unknown[] = [];
  const client = {
    agentCatalogIdentity: () => identity, getGeneration: () => generation,
    requestProjectRoots: () => { registryCalls++; return registry(); },
    allocateRequestId: () => sent.length + 1,
    sendAgent: (_id: number, payload: unknown) => sent.push(payload),
  };
  const adapter = {
    agentSetCatalogEndpoint() {}, agentEvent: (payload: string) => forwarded.push(JSON.parse(payload)),
    conversationNavigatorSetDashboardRoot() {}, enableConversationNavigationHeader() {},
    conversationNavigatorActive: () => active, conversationNavigatorQuery: () => "",
    conversationNavigatorOpen: () => { active = true; }, conversationNavigatorClose: () => { active = false; },
    conversationNavigatorSetRows: (value: string) => rows.push(JSON.parse(value)),
    conversationNavigatorSetLoading() {}, conversationNavigatorSetError() {},
    drainCatalogHeaderAction: () => { const value = header; header = null; return value; },
    drainConversationNavigatorAction: () => { const value = action; action = null; return value; },
  };
  const panel = Object.create(TerminalPanel.prototype) as any;
  const dashboardRootCache = new AuthorizedDashboardRootCache();
  const binding = dashboardRootCache.bind(client, identity, "/authorized/home");
  // This fixture represents the existing authorized FilesLocations response,
  // never a guessed client home or the active chat's execution directory.
  dashboardRootCache.acceptHome(binding, "/authorized/home");
  Object.assign(panel, { options: { client, activeWorkspaceId: "same-workspace", workspaceRoot: "/authorized/home",
    onWorkspaceRootRequested: (...args: unknown[]) => commits.push(args),
    onWorkspaceSelected: () => assert.fail("must not create/select another workspace"),
  }, wasmAdapter: adapter, dashboardRootCache, conversationNavigation: null,
    scheduleDraw() {}, pushInAppNotification: (...args: unknown[]) => notices.push(args),
    openHostFileBrowser: (...args: unknown[]) => browsers.push(args),
  });
  const query = () => sent.at(-1).ListSessionProjects;
  return { panel, client, adapter, rows, commits, browsers, notices, sent, forwarded, query,
    open: () => { header = "projects"; panel.pumpConversationNavigator(); },
    reply: (items = [{ directory: "/chat-only", updatedMs: 1 }], captured = query()) => panel.agentReply({ SessionProjects: { ...captured, items, pinnedSessions: [{ directory: "/not-a-project-source" }] } }),
    fail: (captured = query()) => panel.agentReply({ SessionProjectsFailed: { ...captured, message: "503 index initializing" } }),
    activate: (value: string) => { action = { kind: "activate", value }; panel.pumpConversationNavigator(); },
    dismiss: () => { action = { kind: "dismiss" }; panel.pumpConversationNavigator(); },
    replaceWithSearch: () => { header = "search"; panel.pumpConversationNavigator(); },
    invalidateIdentity: () => { identity = "other-endpoint"; },
    invalidateGeneration: () => { generation++; },
    registryCalls: () => registryCalls,
    close: () => panel.clearConversationNavigation(),
  };
}

const flushNavigatorHost = () => new Promise((resolve) => setImmediate(resolve));

test("TerminalPanel root open loads real registry plus authorized summary and ignores pinned extras", async () => {
  const f = projectHostFixture();
  try {
    f.open();
    assert.deepEqual(Object.keys(f.sent[0]), ["ListSessionProjects"]);
    assert.deepEqual(Object.keys(f.query()).sort(), ["catalog_identity", "generation"]);
    f.reply();
    await flushNavigatorHost();
    assert.deepEqual(new Set(f.rows.at(-1)!.map((r) => r.value)), new Set(["/authorized/home", "/projects/no-history", "/chat-only", ADD_NAVIGATOR_WORKSPACE]));
    assert.equal(f.rows.at(-1)![0].title, "~");
    assert.equal(f.registryCalls(), 1);
    assert.equal(f.panel.conversationNavigation.allowedProjects.has(ADD_NAVIGATOR_WORKSPACE), false);
    assert.equal(f.commits.length, 0);
    f.activate("/chat-only");
    assert.deepEqual(f.commits, [["/chat-only", "same-workspace"]]);
  } finally { f.close(); }
});

test("TerminalPanel keeps each successful source and retries initializing summaries on reopen", async () => {
  for (const registryFails of [false, true]) {
    let rejectRegistry = registryFails;
    const f = projectHostFixture(() => rejectRegistry ? Promise.reject(new Error("registry unavailable")) : Promise.resolve([project("no-history")]));
    try {
      f.open();
      const oldQuery = f.query();
      if (registryFails) f.reply(); else f.fail();
      await flushNavigatorHost();
      const values = f.rows.at(-1)!.map((r) => r.value);
      assert.ok(values.includes(registryFails ? "/chat-only" : "/projects/no-history"));
      assert.equal(f.notices.length, 1);
      f.dismiss();
      assert.equal(f.panel.conversationNavigation, null);
      rejectRegistry = false;
      f.open();
      f.fail(oldQuery);
      f.reply();
      await flushNavigatorHost();
      assert.equal(f.registryCalls(), 2);
      assert.equal(f.notices.length, 1, "late failure from the dismissed picker must not notify again");
      assert.ok(f.rows.at(-1)!.some((r) => r.value === "/projects/no-history"));
      assert.ok(f.rows.at(-1)!.some((r) => r.value === "/chat-only"));
    } finally { f.close(); }
  }
});

test("TerminalPanel Add sentinel opens only the root directory browser and never reroots", async () => {
  for (const root of ["/authorized/home", "/projects/scoped"]) {
    const f = projectHostFixture();
    try {
      f.panel.options.workspaceRoot = root;
      f.open(); f.reply();
      await flushNavigatorHost();
      assert.equal(f.rows.at(-1)!.some((r) => r.value === ADD_NAVIGATOR_WORKSPACE), root === "/authorized/home");
      // Even an injected/stale action or a polluted allowed-path set cannot
      // turn this reserved intent into a directory or workspace-creation call.
      f.panel.conversationNavigation.allowedProjects.add(ADD_NAVIGATOR_WORKSPACE);
      f.activate(ADD_NAVIGATOR_WORKSPACE);
      assert.deepEqual(f.browsers, root === "/authorized/home" ? [["workspace", "choose_directory"]] : []);
      assert.deepEqual(f.commits, []);
      assert.equal(f.panel.conversationNavigation, null);
    } finally { f.close(); }
  }
});

test("TerminalPanel closes discovery on dismissal, replacement and endpoint/root/context invalidation", async () => {
  const invalidators = [
    (f: ReturnType<typeof projectHostFixture>) => f.dismiss(),
    (f: ReturnType<typeof projectHostFixture>) => f.replaceWithSearch(),
    (f: ReturnType<typeof projectHostFixture>) => f.invalidateIdentity(),
    (f: ReturnType<typeof projectHostFixture>) => f.invalidateGeneration(),
    (f: ReturnType<typeof projectHostFixture>) => { f.panel.options.workspaceRoot = "/other-root"; },
    (f: ReturnType<typeof projectHostFixture>) => { f.panel.options.activeWorkspaceId = "other-workspace"; },
    (f: ReturnType<typeof projectHostFixture>) => { f.panel.options.client = { ...f.client }; },
    (f: ReturnType<typeof projectHostFixture>) => { f.panel.wasmAdapter = { ...f.adapter }; },
  ];
  for (const invalidate of invalidators) {
    const f = projectHostFixture();
    try {
      f.open();
      const target = f.panel.conversationNavigation;
      let closed = 0;
      const close = target.discovery.close.bind(target.discovery);
      target.discovery.close = () => { closed++; close(); };
      invalidate(f);
      const count = f.rows.length;
      f.reply();
      await flushNavigatorHost();
      assert.equal(f.rows.length, count);
      assert.ok(closed > 0);
      f.activate(ADD_NAVIGATOR_WORKSPACE);
      assert.equal(f.commits.length, 0);
      assert.equal(f.browsers.length, 0);
    } finally { f.close(); }
  }
});

test("TerminalPanel ignores summary replies for another catalog query without disturbing current discovery", async () => {
  const f = projectHostFixture();
  try {
    f.open();
    f.reply([{ directory: "/foreign", updatedMs: 1 }], { ...f.query(), catalog_identity: "wrong-endpoint" });
    f.reply([{ directory: "/foreign", updatedMs: 1 }], { ...f.query(), generation: f.query().generation + 1 });
    await flushNavigatorHost();
    assert.equal(f.rows.at(-1)!.some((r) => r.value === "/foreign"), false);
    f.reply();
    assert.equal(f.rows.at(-1)!.some((r) => r.value === "/chat-only"), true);
  } finally { f.close(); }
});

function fixture() {
  let generation = 1;
  let active = true;
  let query = "needle";
  const rows: unknown[] = [];
  const attachments: unknown[] = [];
  const client = { agentCatalogIdentity: () => "endpoint-wa", getGeneration: () => generation };
  const adapter = {
    conversationNavigatorActive: () => active, conversationNavigatorQuery: () => query,
    conversationNavigatorSetRows: (value: string) => rows.push(JSON.parse(value)),
    conversationNavigatorSetLoading() {}, conversationNavigatorSetError() {},
    conversationNavigatorAttach: (...args: unknown[]) => attachments.push(args),
    conversationNavigatorClose() {},
  };
  const panel = Object.create(TerminalPanel.prototype) as any;
  const target = { adapter, client, identity: "endpoint-wa", generation: 1, workspaceId: "wa", mode: "search", serial: 12, query: "needle", results: new Map(), completed: false };
  Object.assign(panel, { options: { client, activeWorkspaceId: "wa" }, wasmAdapter: adapter, conversationNavigation: target,
    scheduleDraw() {}, openNeoismAgentTab() {}, activeAgentRouteId: () => 1, bindAgentSessionToRoute() {} });
  const reply = (serial = 12) => panel.agentReply({ ThreadSearchResult: { generation: serial, query: "needle", sessions: [{ id: "outside", title: "Outside", directory: "/saved/project" }], semantic: {}, error: null } });
  return { panel, target, adapter, rows, attachments, reply, setGeneration: (g: number) => generation = g, dismiss: () => active = false, setQuery: (q: string) => query = q };
}

test("real host ignores dismissed, stale query and prior connection search replies", () => {
  for (const invalidate of [(f: ReturnType<typeof fixture>) => f.dismiss(), (f: ReturnType<typeof fixture>) => f.setGeneration(2), (f: ReturnType<typeof fixture>) => f.setQuery("newer")]) {
    const f = fixture(); invalidate(f); f.reply(); assert.equal(f.rows.length, 0);
  }
  const f = fixture(); f.reply(11); assert.equal(f.rows.length, 0);
  f.reply(); assert.equal(f.rows.length, 1); assert.equal(f.target.completed, true);
});

test("search activation attaches by saved metadata without changing declared workspace", () => {
  const f = fixture(); f.reply();
  Object.assign(f.adapter, { drainConversationNavigatorAction: () => ({ kind: "activate", value: "outside" }) });
  f.panel.pumpConversationNavigator();
  assert.deepEqual(f.attachments, [["outside", "/saved/project"]]);
  assert.equal(f.panel.options.activeWorkspaceId, "wa");
  assert.equal(f.panel.conversationNavigation, null);
});

test("project and All projects selections re-root SAME workspace, never activate another grid", () => {
  for (const path of ["/projects/empty", "/authorized/home"]) {
    const f = fixture();
    const commits: unknown[] = [];
    f.target.mode = "projects";
    Object.assign(f.target, { allowedProjects: new Set([path]), projects: [], homePath: "/authorized/home" });
    Object.assign(f.panel.options, { onWorkspaceRootRequested: (...args: unknown[]) => commits.push(args),
      onWorkspaceSelected: () => assert.fail("must not activate another workspace") });
    Object.assign(f.adapter, { drainConversationNavigatorAction: () => ({ kind: "activate", value: path }) });
    f.panel.pumpConversationNavigator();
    assert.deepEqual(commits, [[path, "wa"]]);
    assert.equal(f.panel.options.activeWorkspaceId, "wa");
  }
});

test("Add-workspace intent reuses normal validated directory picker, not workspace creation", () => {
  const f = fixture();
  const opens: unknown[] = [];
  f.panel.openHostFileBrowser = (...args: unknown[]) => opens.push(args);
  f.panel.options.workspaceRoot = "/";
  Object.assign(f.adapter, { drainCatalogHeaderAction: () => "add_workspace" });
  f.panel.pumpConversationNavigator();
  assert.deepEqual(opens, [["workspace", "choose_directory"]]);
});

test("activation of search rows from a previous query is rejected", () => {
  const f = fixture(); f.reply(); f.setQuery("newer query");
  Object.assign(f.adapter, { drainConversationNavigatorAction: () => ({ kind: "activate", value: "outside" }) });
  f.panel.pumpConversationNavigator();
  assert.equal(f.attachments.length, 0);
});

test("registry request stays on the connected ProtocolClient and rejects changed generation", async () => {
  for (const changed of [false, true]) {
    const client = new ProtocolClient({ url: "wss://authorized/ws" });
    const calls: unknown[] = [];
    (client as any).authenticated = true;
    client.sendWorkspace = (message) => { calls.push(message); };
    const pending = client.requestProjectRoots();
    if (changed) (client as any).generation += 1;
    (client as any).handleRawMessage(JSON.stringify({ WorkspaceReply: { request_id: 1, message: { ProjectRootList: { project_roots: [project("empty")] } } } }));
    assert.deepEqual(calls, ["ListProjectRoots"]);
    if (changed) await assert.rejects(pending, /connection changed/);
    else assert.deepEqual(await pending, [project("empty")]);
  }
});
