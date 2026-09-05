// Unit tests for the multi-daemon workplace switcher (task F1).
//
// These tests exercise the "swap active client live" flow:
//   1. Two workplaces are registered.
//   2. `connect(A)` opens a `ProtocolClient` against A; the swap
//      records A as both `activeId` and `lastActiveId` and the
//      latter persists to localStorage.
//   3. `switchTo(B)` disconnects A's client (we assert
//      `disconnect()` was called) and constructs a fresh client
//      against B with B's stashed pairing token.
//   4. `requestPaneSnapshot()` re-issues `ListSessions` +
//      `ListEditorSurfaces` against the new active client so the
//      chrome can rehydrate pane state from B's daemon.
//   5. A reload (i.e. a fresh `WorkplaceService` constructed against
//      the same storage) recovers `lastActiveId` so the chrome can
//      default the connection screen back to B.
//
// The tests use the `clientFactory` DI hook to substitute a
// `FakeProtocolClient` for the real WebSocket-backed one — no
// network is involved.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  WorkplaceService,
  type DiscoveredWorkplace,
  type ProtocolClientFactory,
} from "./WorkplaceService.ts";
import type {
  ProtocolClient,
  ProtocolClientHandlers,
  ProtocolClientOptions,
} from "../workspace/ProtocolClient.ts";
import type {
  WorkspaceClientMessage,
  WorkspaceSummary,
} from "../workspace/types.ts";

interface SentWorkspaceMessage {
  message: WorkspaceClientMessage | string;
}

/** Minimal `ProtocolClient` stand-in. Records every `sendWorkspace`
 *  call (so tests can assert the rehydrate frames went out against
 *  the new daemon, not the old one) and a `disconnect` counter. */
class FakeProtocolClient {
  public readonly options: ProtocolClientOptions;
  public readonly handlers: ProtocolClientHandlers;
  public readonly sent: SentWorkspaceMessage[] = [];
  public disconnects = 0;
  public connects = 0;

  constructor(options: ProtocolClientOptions, handlers: ProtocolClientHandlers) {
    this.options = options;
    this.handlers = handlers;
  }

  connect(): void {
    this.connects += 1;
  }

  disconnect(): void {
    this.disconnects += 1;
  }

  sendWorkspace(message: WorkspaceClientMessage | string): void {
    this.sent.push({ message });
  }
}

/** In-memory `Storage` so the tests can observe persisted writes
 *  without touching the real `localStorage`. */
class MemoryStorage implements Storage {
  private readonly store = new Map<string, string>();
  get length(): number {
    return this.store.size;
  }
  key(index: number): string | null {
    return Array.from(this.store.keys())[index] ?? null;
  }
  getItem(key: string): string | null {
    return this.store.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.store.set(key, value);
  }
  removeItem(key: string): void {
    this.store.delete(key);
  }
  clear(): void {
    this.store.clear();
  }
}

function buildService(): {
  service: WorkplaceService;
  storage: MemoryStorage;
  built: FakeProtocolClient[];
} {
  const storage = new MemoryStorage();
  const built: FakeProtocolClient[] = [];
  const factory: ProtocolClientFactory = (options, handlers) => {
    const fake = new FakeProtocolClient(options, handlers);
    built.push(fake);
    return fake as unknown as ProtocolClient;
  };
  const service = new WorkplaceService(storage, factory);
  return { service, storage, built };
}

const HANDLERS: ProtocolClientHandlers = {};

test("switchTo swaps the active client live and rehydrates from the new daemon", () => {
  const { service, storage, built } = buildService();

  service.addWorkplace(
    { id: "manual:ws://laptop-a/session", label: "A", url: "ws://laptop-a:7878/session", transport: "manual" },
    "token-A",
  );
  service.addWorkplace(
    { id: "manual:ws://laptop-b/session", label: "B", url: "ws://laptop-b:7878/session", transport: "manual" },
    "token-B",
  );

  // (1) connect to A.
  const clientA = service.connect("manual:ws://laptop-a/session", HANDLERS);
  assert.equal(built.length, 1, "one client constructed for A");
  assert.equal(built[0].options.url, "ws://laptop-a:7878/session");
  assert.equal(built[0].options.pairingToken, "token-A");
  assert.equal(service.getActiveId(), "manual:ws://laptop-a/session");
  assert.equal(service.getLastActiveId(), "manual:ws://laptop-a/session");
  // `connect()` itself does NOT dial — caller drives `client.connect()`.
  // We just verify the live client is the one we got back.
  assert.equal(clientA, built[0] as unknown as ProtocolClient);

  // (2) switch to B — A's client must be disconnected and a fresh
  //     client built against B's URL + token.
  const clientB = service.switchTo("manual:ws://laptop-b/session", HANDLERS);
  assert.equal(built.length, 2, "second client constructed for B");
  assert.equal(built[0].disconnects, 1, "A's client was disconnected");
  assert.equal(built[1].options.url, "ws://laptop-b:7878/session");
  assert.equal(built[1].options.pairingToken, "token-B");
  assert.equal(service.getActiveId(), "manual:ws://laptop-b/session");
  assert.equal(service.getLastActiveId(), "manual:ws://laptop-b/session");
  assert.equal(clientB, built[1] as unknown as ProtocolClient);

  // (3) request the pane snapshot — the helper must send
  //     ListSessions + ListEditorSurfaces against B (the new active),
  //     never against A.
  assert.equal(service.requestPaneSnapshot(), true);
  const aFrames = built[0].sent.map((s) => s.message);
  const bFrames = built[1].sent.map((s) => s.message);
  assert.deepEqual(aFrames, [], "A received no frames after the swap");
  assert.deepEqual(
    bFrames,
    ["ListSessions", "ListEditorSurfaces"],
    "B got both rehydrate frames in order",
  );

  // (4) localStorage carries the lastActiveId but never the tokens.
  const raw = storage.getItem("neoism.workplaces.v1");
  assert.ok(raw, "registry was persisted");
  const parsed = JSON.parse(raw!) as Record<string, unknown>;
  assert.equal(parsed.lastActiveId, "manual:ws://laptop-b/session");
  assert.ok(!("activeId" in parsed), "activeId is NOT persisted");
  assert.ok(!raw!.includes("token-A"), "token-A is NOT persisted");
  assert.ok(!raw!.includes("token-B"), "token-B is NOT persisted");
});

test("requestPaneSnapshot returns false when no client is active", () => {
  const { service } = buildService();
  assert.equal(service.requestPaneSnapshot(), false);
});

test("accepted HelloAck caches the connected daemon stable host id", () => {
  const { service, built } = buildService();
  service.addWorkplace({
    id: "manual:alias",
    label: "Laptop",
    url: "wss://laptop.tailnet.ts.net/session",
    transport: "manual",
  });
  service.connect("manual:alias", HANDLERS);

  assert.equal(service.getConnectedHostId(), null);
  built[0].handlers.onHelloAck?.(true, null, null, "machine-a");
  assert.equal(service.getConnectedHostId(), "machine-a");

  service.disconnect();
  assert.equal(service.getConnectedHostId(), null);
});

test("a stale HelloAck cannot replace the active workplace host id", () => {
  const { service, built } = buildService();
  for (const [id, label] of [["a", "A"], ["b", "B"]] as const) {
    service.addWorkplace({
      id,
      label,
      url: `ws://${id}/session`,
      transport: "manual",
    });
  }
  service.connect("a", HANDLERS);
  service.switchTo("b", HANDLERS);
  built[1].handlers.onHelloAck?.(true, null, null, "machine-b");
  built[0].handlers.onHelloAck?.(true, null, null, "machine-a");
  assert.equal(service.getConnectedHostId(), "machine-b");
  assert.equal(service.getActiveId(), "b");
});

test("reload restores lastActiveId from storage but does NOT auto-reconnect", () => {
  const { service: first, storage } = buildService();
  first.addWorkplace(
    { id: "manual:ws://laptop-a/session", label: "A", url: "ws://laptop-a:7878/session", transport: "manual" },
    "",
  );
  first.connect("manual:ws://laptop-a/session", HANDLERS);
  assert.equal(first.getLastActiveId(), "manual:ws://laptop-a/session");

  // Simulate a page reload by building a fresh service against the
  // same storage.
  const built2: FakeProtocolClient[] = [];
  const factory2: ProtocolClientFactory = (options, handlers) => {
    const fake = new FakeProtocolClient(options, handlers);
    built2.push(fake);
    return fake as unknown as ProtocolClient;
  };
  const second = new WorkplaceService(storage, factory2);
  assert.equal(
    second.getLastActiveId(),
    "manual:ws://laptop-a/session",
    "reload remembers the last active workplace",
  );
  assert.equal(second.getActiveId(), null, "but no socket is live yet");
  assert.equal(built2.length, 0, "and no client was constructed");
});

test("connect to the already-active workplace is idempotent (no extra client)", () => {
  const { service, built } = buildService();
  service.addWorkplace(
    { id: "manual:ws://laptop-a/session", label: "A", url: "ws://laptop-a:7878/session", transport: "manual" },
    "",
  );
  const first = service.connect("manual:ws://laptop-a/session", HANDLERS);
  const again = service.connect("manual:ws://laptop-a/session", HANDLERS);
  assert.equal(first, again, "second connect returned the same client");
  assert.equal(built.length, 1, "no extra client was constructed");
});

test("removeWorkplace clears the active connection if it targets the active id", () => {
  const { service, built } = buildService();
  service.addWorkplace(
    { id: "manual:ws://laptop-a/session", label: "A", url: "ws://laptop-a:7878/session", transport: "manual" },
    "",
  );
  service.connect("manual:ws://laptop-a/session", HANDLERS);
  service.removeWorkplace("manual:ws://laptop-a/session");
  assert.equal(service.getActiveId(), null);
  assert.equal(built[0].disconnects, 1, "underlying client was disconnected");
});

// ---------------------------------------------------------------------
// Tailscale discovery -> promote -> connect (task 2C)
//
// These tests stub `globalThis.fetch` so `refreshTailscalePeers` can be
// driven without a real daemon, then prove the full path a switcher
// click takes: discover peers off `/tailnet-peers`, promote one via
// `addWorkplace`, and open a `ProtocolClient` against
// `ws://<ip>:7878/session` with the in-memory pairing token — and that
// the token never lands in storage.
// ---------------------------------------------------------------------

type FetchFn = typeof globalThis.fetch;

/** Swap `globalThis.fetch` for a stub that records every requested URL
 *  and returns the supplied JSON body. Returns a restore handle the
 *  test must call in a `finally`. */
function stubFetch(
  body: unknown,
  options: { ok?: boolean } = {},
): { calls: string[]; restore: () => void } {
  const calls: string[] = [];
  const previous = globalThis.fetch;
  const stub = (async (input: RequestInfo | URL) => {
    calls.push(String(input));
    return {
      ok: options.ok ?? true,
      json: async () => body,
    } as Response;
  }) as FetchFn;
  globalThis.fetch = stub;
  return { calls, restore: () => (globalThis.fetch = previous) };
}

test("refreshTailscalePeers hits /tailnet-peers and surfaces discovered peers", async () => {
  const { service } = buildService();
  const { calls, restore } = stubFetch({
    peers: [
      { hostname: "laptop-b", ip: "100.64.0.2", online: true },
      { hostname: "home-server", ip: "100.64.0.3", online: false },
    ],
  });
  try {
    const result = await service.refreshTailscalePeers(
      "ws://laptop-a:7878/session",
    );
    // The websocket URL is rewritten to the HTTP discovery endpoint.
    assert.deepEqual(calls, ["http://laptop-a:7878/tailnet-peers"]);
    assert.equal(result.total, 2);
    assert.equal(result.added.length, 2, "both peers are new");

    const discovered = service.listDiscovered();
    assert.equal(discovered.length, 2);
    const byLabel = new Map(discovered.map((d) => [d.label, d]));
    const peerB = byLabel.get("laptop-b")!;
    assert.equal(peerB.id, "tailscale:laptop-b@100.64.0.2");
    assert.equal(peerB.url, "ws://100.64.0.2:7878/session");
    assert.equal(peerB.transport, "tailscale");
    assert.equal(peerB.peer.online, true);
    assert.equal(byLabel.get("home-server")!.peer.online, false);
  } finally {
    restore();
  }
});

test("refreshTailscalePeers de-dupes and refreshes online state on re-discovery", async () => {
  const { service } = buildService();

  const first = stubFetch({
    peers: [{ hostname: "laptop-b", ip: "100.64.0.2", online: false }],
  });
  try {
    const r1 = await service.refreshTailscalePeers("ws://laptop-a:7878/session");
    assert.equal(r1.added.length, 1);
    assert.equal(service.listDiscovered()[0].peer.online, false);
  } finally {
    first.restore();
  }

  // Same peer comes back online — no new "added" entry, but the cached
  // online flag flips so the switcher can un-dim the row.
  const second = stubFetch({
    peers: [{ hostname: "laptop-b", ip: "100.64.0.2", online: true }],
  });
  try {
    const r2 = await service.refreshTailscalePeers("ws://laptop-a:7878/session");
    assert.equal(r2.total, 1);
    assert.equal(r2.added.length, 0, "already-known peer is not re-added");
    assert.equal(service.listDiscovered().length, 1, "no duplicate entry");
    assert.equal(
      service.listDiscovered()[0].peer.online,
      true,
      "cached online flag was refreshed",
    );
  } finally {
    second.restore();
  }
});

test("refreshTailscalePeers degrades to empty on a non-OK response", async () => {
  const { service } = buildService();
  const { restore } = stubFetch({ peers: [{ hostname: "x", ip: "1.2.3.4" }] }, {
    ok: false,
  });
  try {
    const result = await service.refreshTailscalePeers(
      "ws://laptop-a:7878/session",
    );
    assert.deepEqual(result, { added: [], total: 0 });
    assert.equal(service.listDiscovered().length, 0);
  } finally {
    restore();
  }
});

test("discover -> promote -> connect opens a ProtocolClient at the peer's ws URL with the in-memory token", async () => {
  const { service, storage, built } = buildService();
  const events: string[] = [];
  service.subscribe((event) => events.push(event.kind));

  // (1) Discover a peer off the focused daemon's tailnet.
  const { restore } = stubFetch({
    peers: [{ hostname: "laptop-b", ip: "100.64.0.2", online: true }],
  });
  let discovered: DiscoveredWorkplace;
  try {
    const result = await service.refreshTailscalePeers(
      "ws://laptop-a:7878/session",
    );
    assert.equal(result.added.length, 1);
    discovered = result.added[0];
  } finally {
    restore();
  }
  assert.ok(events.includes("discovered"), "a discovered event fired");

  // (2) Promote the candidate into the registry, stashing the pairing
  //     token the switcher prompted for.
  const promoted = service.addWorkplace(
    {
      id: discovered.id,
      url: discovered.url,
      label: discovered.label,
      transport: "tailscale",
    },
    "pair-secret-b",
  );
  assert.equal(promoted.transport, "tailscale");
  assert.equal(
    service.listWorkplaces().some((e) => e.id === discovered.id),
    true,
    "promoted peer is now a registered workplace",
  );
  assert.equal(service.hasPairingToken(discovered.id), true);

  // (3) Connect — a ProtocolClient is built against the peer's
  //     ws://<ip>:7878/session URL carrying the in-memory token on both
  //     the legacy `?token` and the `Hello { token }` channels.
  const client = service.connect(discovered.id, HANDLERS);
  assert.equal(built.length, 1, "exactly one client built");
  assert.equal(built[0].options.url, "ws://100.64.0.2:7878/session");
  assert.equal(built[0].options.authToken, "pair-secret-b");
  assert.equal(built[0].options.pairingToken, "pair-secret-b");
  assert.equal(built[0].options.clientName, "neoism-web");
  assert.equal(service.getActiveId(), discovered.id);
  assert.equal(client, built[0] as unknown as ProtocolClient);

  // (4) Threat model: the pairing token is NEVER written to storage.
  const raw = storage.getItem("neoism.workplaces.v1");
  assert.ok(raw, "registry persisted");
  assert.ok(
    !raw!.includes("pair-secret-b"),
    "pairing token must not be persisted to storage",
  );
  // The promoted URL + transport ARE persisted (they're not secrets).
  const parsed = JSON.parse(raw!) as { entries: Array<Record<string, unknown>> };
  const persisted = parsed.entries.find((e) => e.id === discovered.id);
  assert.ok(persisted, "promoted entry persisted");
  assert.equal(persisted!.url, "ws://100.64.0.2:7878/session");
  assert.equal(persisted!.transport, "tailscale");
});

test("hello-ack rejection preserves the stopped facade for immediate switch UI", async () => {
  const { service, built } = buildService();
  const events: Array<{ kind: string; accepted?: boolean }> = [];
  service.subscribe((event) =>
    events.push(event as { kind: string; accepted?: boolean }),
  );

  service.addWorkplace(
    {
      id: "tailscale:laptop-b@100.64.0.2",
      url: "ws://100.64.0.2:7878/session",
      label: "laptop-b",
      transport: "tailscale",
    },
    "bad-token",
  );
  service.connect("tailscale:laptop-b@100.64.0.2", HANDLERS);
  assert.equal(service.getActiveId(), "tailscale:laptop-b@100.64.0.2");

  // Daemon rejects the Hello handshake (e.g. wrong pairing token). The
  // service fans a `hello-ack` event. ProtocolClient itself closes the doomed
  // socket and stops retry; the service preserves its stable facade so the
  // connection gate can still identify/switch the selected workplace.
  built[0].handlers.onHelloAck?.(false, "invalid pairing token", null, null);

  const ack = events.find((e) => e.kind === "hello-ack");
  assert.ok(ack, "a hello-ack event fired");
  assert.equal(ack!.accepted, false);
  assert.equal(built[0].disconnects, 0, "service does not overwrite auth-rejected with closed");
  assert.equal(service.getActiveId(), "tailscale:laptop-b@100.64.0.2");
});

// ---------------------------------------------------------------------
// Follow-the-workspace re-homing (Wave 4B)
//
// When the workspace the web client is *viewing* gets re-homed to a
// different host (`running_on_host_id` flips — e.g. promoted to a cloud
// node), the client should FOLLOW it: resolve the new host's daemon URL
// and re-dial the active `ProtocolClient` there.
//
// host_id -> URL is only a HEURISTIC today (the daemon's `HostSummary`
// has no address field — see `resolveHostUrl`'s TODO), so these tests
// exercise both the resolvable path (host id matches a registered /
// discovered tailnet hostname) and the unresolvable path (emit a
// `rehome { resolved:false }` event, do NOT re-dial blindly).
// ---------------------------------------------------------------------

/** Build a minimal `WorkspaceSummary` for the re-home tests. */
function workspace(
  id: string,
  hostId: string,
  runningOnHostId: string | null,
): WorkspaceSummary {
  return {
    id,
    host_id: hostId,
    title: id,
    running_on_host_id: runningOnHostId,
  };
}

type RehomeEvent = Extract<
  Parameters<Parameters<WorkplaceService["subscribe"]>[0]>[0],
  { kind: "rehome" }
>;

test("re-home: resolvable move re-dials to the new host and emits a resolved event", () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  // Two hosts whose ids match their tailnet hostnames (the heuristic the
  // resolver leans on). Connect to laptop-a first.
  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "token-a",
  );
  service.addWorkplace(
    {
      id: "tailscale:cloud-burst@100.64.0.9",
      url: "ws://100.64.0.9:7878/session",
      label: "cloud-burst",
      transport: "tailscale",
    },
    "token-cloud",
  );
  service.setRehomeHandlers(HANDLERS);
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);
  assert.equal(built.length, 1, "one client for laptop-a");

  // The chrome reports it's viewing ws-1, currently homed on laptop-a.
  service.setFollowedWorkspace("ws-1", "laptop-a");

  // Daemon fans a tree where ws-1's home flipped to cloud-burst.
  service.observeWorkspaceHoming([workspace("ws-1", "laptop-a", "cloud-burst")]);

  // A fresh client was dialled against cloud-burst's URL + token, and
  // laptop-a's socket was torn down.
  assert.equal(built.length, 2, "second client constructed for cloud-burst");
  assert.equal(built[0].disconnects, 1, "laptop-a was disconnected");
  assert.equal(built[1].options.url, "ws://100.64.0.9:7878/session");
  assert.equal(built[1].options.pairingToken, "token-cloud");
  assert.equal(service.getActiveId(), "tailscale:cloud-burst@100.64.0.9");

  assert.equal(events.length, 1, "exactly one rehome event");
  assert.equal(events[0].resolved, true);
  assert.equal(events[0].workspaceId, "ws-1");
  assert.equal(events[0].previousHostId, "laptop-a");
  assert.equal(events[0].newHostId, "cloud-burst");
  assert.equal(events[0].targetId, "tailscale:cloud-burst@100.64.0.9");
  assert.equal(events[0].targetUrl, "ws://100.64.0.9:7878/session");
});

test("re-home: move resolvable via a discovered tailnet peer auto-registers and follows", async () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "",
  );
  service.setRehomeHandlers(HANDLERS);
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);

  // Discover the cloud node off laptop-a's tailnet (so it's a candidate
  // but NOT yet in the registry).
  const { restore } = stubFetch({
    peers: [{ hostname: "cloud-burst", ip: "100.64.0.9", online: true }],
  });
  try {
    await service.refreshTailscalePeers("ws://100.64.0.1:7878/session");
  } finally {
    restore();
  }
  assert.equal(
    service.listWorkplaces().some((e) => e.id === "tailscale:cloud-burst@100.64.0.9"),
    false,
    "cloud-burst is only discovered, not registered yet",
  );

  service.setFollowedWorkspace("ws-1", "laptop-a");
  service.observeWorkspaceHoming([workspace("ws-1", "laptop-a", "cloud-burst")]);

  // The discovered peer was promoted into the registry and dialled.
  assert.equal(
    service.listWorkplaces().some((e) => e.id === "tailscale:cloud-burst@100.64.0.9"),
    true,
    "discovered peer was auto-registered on re-home",
  );
  assert.equal(built.length, 2, "re-dialled to the discovered peer");
  assert.equal(built[1].options.url, "ws://100.64.0.9:7878/session");
  assert.equal(events.length, 1);
  assert.equal(events[0].resolved, true);
});

test("re-home: unresolvable move emits resolved:false and does NOT re-dial", () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "",
  );
  service.setRehomeHandlers(HANDLERS);
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);
  service.setFollowedWorkspace("ws-1", "laptop-a");

  // The new host id is an opaque operator string with no matching
  // registry entry or discovered peer (the real-world cloud-burst case
  // where `NEOISM_HOST_ID` != tailnet hostname).
  service.observeWorkspaceHoming([
    workspace("ws-1", "laptop-a", "opaque-host-7"),
  ]);

  assert.equal(built.length, 1, "no new client was dialled");
  assert.equal(built[0].disconnects, 0, "the live socket stays put");
  assert.equal(service.getActiveId(), "tailscale:laptop-a@100.64.0.1");
  assert.equal(events.length, 1, "an (unresolved) rehome event still fired");
  assert.equal(events[0].resolved, false);
  assert.equal(events[0].newHostId, "opaque-host-7");
  assert.equal(events[0].targetId, null);
  assert.equal(events[0].targetUrl, null);
  // The seam is also directly callable.
  assert.equal(service.resolveHostUrl("opaque-host-7"), null);
  assert.equal(
    service.resolveHostUrl("laptop-a"),
    "ws://100.64.0.1:7878/session",
    "resolveHostUrl finds a registered host by hostname",
  );
});

test("re-home: first observation only seeds the baseline (no re-dial on connect)", () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "",
  );
  service.setRehomeHandlers(HANDLERS);
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);

  // Follow with NO seeded baseline; the first tree observation just
  // records the home and must not yank the connection even though the
  // host would resolve.
  service.setFollowedWorkspace("ws-1", null);
  service.observeWorkspaceHoming([workspace("ws-1", "laptop-a", "laptop-a")]);

  assert.equal(built.length, 1, "no re-dial on the baseline observation");
  assert.equal(events.length, 0, "no rehome event for the first observation");

  // A *subsequent* genuine move now fires.
  service.addWorkplace(
    {
      id: "tailscale:cloud@100.64.0.9",
      url: "ws://100.64.0.9:7878/session",
      label: "cloud",
      transport: "tailscale",
    },
    "",
  );
  service.observeWorkspaceHoming([workspace("ws-1", "laptop-a", "cloud")]);
  assert.equal(events.length, 1, "the real move fires");
  assert.equal(events[0].resolved, true);
  assert.equal(built.length, 2, "and re-dials");
});

test("re-home: a move to the host we're already connected to is a no-op", () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "",
  );
  service.setRehomeHandlers(HANDLERS);
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);
  // Baseline says the workspace was homed elsewhere; the move brings it
  // HOME to laptop-a (the flip-back-to-local differentiator) — we're
  // already connected there, so nothing to re-dial.
  service.setFollowedWorkspace("ws-1", "cloud");
  service.observeWorkspaceHoming([workspace("ws-1", "laptop-a", "laptop-a")]);

  assert.equal(built.length, 1, "no extra client");
  assert.equal(built[0].disconnects, 0, "connection preserved");
  assert.equal(events.length, 0, "no rehome event — already home");
});

test("re-home: moves of a non-followed workspace are ignored", () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "",
  );
  service.addWorkplace(
    {
      id: "tailscale:cloud@100.64.0.9",
      url: "ws://100.64.0.9:7878/session",
      label: "cloud",
      transport: "tailscale",
    },
    "",
  );
  service.setRehomeHandlers(HANDLERS);
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);
  service.setFollowedWorkspace("ws-1", "laptop-a");

  // ws-2 (NOT the followed workspace) moves — must be ignored entirely.
  service.observeWorkspaceHoming([workspace("ws-2", "laptop-a", "cloud")]);

  assert.equal(built.length, 1, "no re-dial for a background workspace move");
  assert.equal(events.length, 0, "no rehome event");
});

test("re-home: detection without handlers emits the event but skips the auto-dial", () => {
  const { service, built } = buildService();
  const events: RehomeEvent[] = [];
  service.subscribe((event) => {
    if (event.kind === "rehome") events.push(event);
  });

  service.addWorkplace(
    {
      id: "tailscale:laptop-a@100.64.0.1",
      url: "ws://100.64.0.1:7878/session",
      label: "laptop-a",
      transport: "tailscale",
    },
    "",
  );
  service.addWorkplace(
    {
      id: "tailscale:cloud@100.64.0.9",
      url: "ws://100.64.0.9:7878/session",
      label: "cloud",
      transport: "tailscale",
    },
    "",
  );
  // No `setRehomeHandlers` — the chrome wants to drive the swap itself.
  service.connect("tailscale:laptop-a@100.64.0.1", HANDLERS);
  service.setFollowedWorkspace("ws-1", "laptop-a");
  service.observeWorkspaceHoming([workspace("ws-1", "laptop-a", "cloud")]);

  assert.equal(events.length, 1, "the resolved rehome event still fires");
  assert.equal(events[0].resolved, true);
  assert.equal(events[0].targetId, "tailscale:cloud@100.64.0.9");
  assert.equal(built.length, 1, "but the service did NOT auto-dial");
  assert.equal(service.getActiveId(), "tailscale:laptop-a@100.64.0.1");
});
