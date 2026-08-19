import { test } from "node:test";
import assert from "node:assert/strict";

import { RemotePresenceStore } from "./RemotePresenceStore.ts";
import { presenceBufferIdForPath } from "./presence.ts";
import {
  loadWasmInputPolicyModule,
  skipReason,
} from "./wasmPolicyTestSupport.mts";
import type {
  CrdtPeerPresence,
  CrdtServerMessage,
} from "../workspace/types.ts";
import type {
  WasmInputPolicyModule,
  WasmPresenceStoreInstance,
} from "../terminal/createTerminal.ts";

// The store itself is the SHARED RUST `RemotePresenceStore` (see
// `neoism-frontend/shared/src/editor/crdt/remote_presence.rs` for the
// canonical unit tests); these tests drive the same scenarios through
// the wasm `PresenceStoreBridge` + the TS adapter, plus adapter-only
// glue (message filtering, pre-load queue & replay).

const wasm = await loadWasmInputPolicyModule();
const bridgeSkip = skipReason(wasm, "PresenceStoreBridge");

function wirePresence(
  bufferId: string,
  peerId: string,
  line: number,
  at: number,
): CrdtPeerPresence {
  return {
    buffer_id: bufferId,
    peer_id: peerId,
    display_name: peerId.toUpperCase(),
    color: { r: 1, g: 2, b: 3 },
    cursor: { line, column: 4, offset: null },
    selection: null,
    updated_at_ms: at,
  };
}

function upsert(presence: CrdtPeerPresence): CrdtServerMessage {
  return { Presence: { update: { Upsert: presence } } };
}

function bridgeStore(): RemotePresenceStore {
  return new RemotePresenceStore(wasm);
}

test("store tracks upserts per buffer and exposes cursors", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  assert.ok(store.applyServerMessage(upsert(wirePresence("buf-a", "alice", 3, 10))));
  assert.ok(store.applyServerMessage(upsert(wirePresence("buf-b", "bob", 7, 11))));

  const inA = store.cursorsFor("buf-a");
  assert.equal(inA.length, 1);
  assert.equal(inA[0].peer_id, "alice");
  assert.equal(inA[0].cursor.line, 3);
  assert.equal(inA[0].display_name, "ALICE");
  assert.equal(inA[0].color.r, 1);
  assert.ok(store.hasRemoteCursors("buf-b"));
  assert.ok(!store.hasRemoteCursors("buf-missing"));
});

test("store dedupes identical upserts for cheap redraw gating", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  const presence = wirePresence("buf-a", "alice", 3, 10);
  assert.ok(store.applyServerMessage(upsert(presence)));
  assert.ok(
    !store.applyServerMessage(upsert({ ...presence })),
    "identical re-publish must not report a change",
  );
  assert.ok(store.applyServerMessage(upsert(wirePresence("buf-a", "alice", 4, 12))));
});

test("store filters local peer and applies removes", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  store.setLocalPeerId("me");
  assert.ok(
    !store.applyServerMessage(upsert(wirePresence("buf-a", "me", 1, 1))),
    "defensive echo filter: own peer id never lands in the store",
  );
  store.applyServerMessage(upsert(wirePresence("buf-a", "alice", 2, 2)));

  assert.ok(
    store.applyServerMessage({
      Presence: { update: { Remove: { buffer_id: "buf-a", peer_id: "alice" } } },
    }),
  );
  assert.ok(!store.hasRemoteCursors("buf-a"));
  assert.ok(
    !store.applyServerMessage({
      Presence: { update: { Remove: { buffer_id: "buf-a", peer_id: "alice" } } },
    }),
    "removing an unknown peer is a no-change",
  );
});

test("store snapshot replaces buffer state", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  store.setLocalPeerId("me");
  store.applyServerMessage(upsert(wirePresence("buf-a", "stale", 9, 1)));

  assert.ok(
    store.applyServerMessage({
      PresenceSnapshot: {
        buffer_id: "buf-a",
        peers: [
          wirePresence("buf-a", "alice", 1, 5),
          wirePresence("buf-a", "me", 0, 5),
        ],
      },
    }),
  );

  const peers = store.cursorsFor("buf-a");
  assert.equal(peers.length, 1, "snapshot replaces + filters local peer");
  assert.equal(peers[0].peer_id, "alice");
});

test("store prunes stale entries by ttl", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  store.applyServerMessage(upsert(wirePresence("buf-a", "old", 1, 100)));
  store.applyServerMessage(upsert(wirePresence("buf-a", "fresh", 2, 950)));

  assert.ok(store.pruneStale(1_000, 500));
  const peers = store.cursorsFor("buf-a");
  assert.equal(peers.length, 1);
  assert.equal(peers[0].peer_id, "fresh");
  assert.ok(!store.pruneStale(1_001, 500));
});

test("non-presence messages do not disturb the store", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  store.applyServerMessage(upsert(wirePresence("buf-a", "alice", 1, 1)));
  assert.ok(
    !store.applyServerMessage({
      Error: { buffer_id: null, message: "nope" },
    }),
  );
  assert.ok(store.hasRemoteCursors("buf-a"));
});

test("avatar peers index carries the set_presence_index feed shape", { skip: bridgeSkip }, () => {
  const store = bridgeStore();
  store.setLocalPeerId("me@host");
  store.applyServerMessage(upsert(wirePresence("file:///a.rs", "alice", 1, 10)));
  store.applyServerMessage(upsert(wirePresence("file:///a.rs", "bob", 3, 12)));
  store.applyServerMessage(upsert(wirePresence("file:///b.rs", "alice", 2, 11)));
  store.applyServerMessage(upsert(wirePresence("file:///a.rs", "me@host", 4, 13)));

  const byBuffer = store.avatarPeersByBuffer();
  const a = byBuffer.find((entry) => entry.buffer_id === "file:///a.rs");
  assert.ok(a, "buffer a present");
  assert.deepEqual(
    a?.peers.map((p) => p.peer_id),
    ["alice", "bob"],
    "peers sorted by peer_id, local excluded",
  );
  assert.deepEqual(a?.peers[0].color, [1, 2, 3]);
  assert.equal(a?.peers[0].rainbow, false);
  const b = byBuffer.find((entry) => entry.buffer_id === "file:///b.rs");
  assert.equal(b?.peers.length, 1);
  // clear() drops everything and reports the change once.
  assert.ok(store.clear());
  assert.equal(store.avatarPeersByBuffer().length, 0);
  assert.ok(!store.clear());
});

// -------------------------------------------------------------------
// Adapter glue: pre-load queue & replay (scripted fake bindings — this
// tests the ADAPTER, not the policy, which stays wasm-only).
// -------------------------------------------------------------------

interface FakeCall {
  method: string;
  args: unknown[];
}

function fakeBindings(): {
  module: WasmInputPolicyModule;
  calls: FakeCall[];
} {
  const calls: FakeCall[] = [];
  class FakeStore implements WasmPresenceStoreInstance {
    set_local_peer_id(peerId: string): void {
      calls.push({ method: "set_local_peer_id", args: [peerId] });
    }
    apply_server_message(message: unknown): boolean {
      calls.push({ method: "apply_server_message", args: [message] });
      return true;
    }
    cursors_for(bufferId: string): unknown {
      calls.push({ method: "cursors_for", args: [bufferId] });
      return [];
    }
    has_remote_cursors(): boolean {
      return false;
    }
    any_rainbow(): boolean {
      return false;
    }
    has_any_peers(): boolean {
      return false;
    }
    avatar_peers_by_buffer(): unknown {
      return [];
    }
    prune_stale(): boolean {
      calls.push({ method: "prune_stale", args: [] });
      return false;
    }
    clear(): boolean {
      calls.push({ method: "clear", args: [] });
      return false;
    }
  }
  return { module: { PresenceStoreBridge: FakeStore }, calls };
}

test("messages queue before the bundle loads and replay in order", () => {
  const { module, calls } = fakeBindings();
  let loaded: WasmInputPolicyModule | null = null;
  const store = new RemotePresenceStore(() => loaded);

  store.setLocalPeerId("me");
  assert.equal(
    store.applyServerMessage(upsert(wirePresence("buf-a", "alice", 1, 1))),
    false,
    "pre-load: nothing to draw yet",
  );
  assert.equal(
    store.applyServerMessage(upsert(wirePresence("buf-a", "bob", 2, 2))),
    false,
  );
  assert.deepEqual(store.cursorsFor("buf-a"), [], "pre-load queries are empty");
  assert.equal(calls.length, 0, "fake untouched before the bundle loads");

  // Bundle arrives; the next boolean-returning call replays the queue
  // and reports the replay as a change so the host redraws.
  loaded = module;
  assert.equal(store.pruneStale(1_000, 500), true, "replay reported as change");
  assert.deepEqual(
    calls.map((c) => c.method),
    ["set_local_peer_id", "apply_server_message", "apply_server_message", "prune_stale"],
    "local peer id first, then queued messages in arrival order",
  );
  const replayed = calls
    .filter((c) => c.method === "apply_server_message")
    .map((c) => (c.args[0] as { Presence: { update: { Upsert: CrdtPeerPresence } } }).Presence.update.Upsert.peer_id);
  assert.deepEqual(replayed, ["alice", "bob"]);
});

test("non-presence traffic short-circuits without queueing", () => {
  const { module, calls } = fakeBindings();
  let loaded: WasmInputPolicyModule | null = null;
  const store = new RemotePresenceStore(() => loaded);
  assert.equal(
    store.applyServerMessage({ Error: { buffer_id: null, message: "nope" } }),
    false,
  );
  loaded = module;
  store.pruneStale(0, 0);
  assert.deepEqual(
    calls.map((c) => c.method),
    ["prune_stale"],
    "no queued replay for non-presence traffic",
  );
});

test("pre-load queue is bounded", () => {
  const { module, calls } = fakeBindings();
  let loaded: WasmInputPolicyModule | null = null;
  const store = new RemotePresenceStore(() => loaded);
  for (let i = 0; i < 300; i += 1) {
    store.applyServerMessage(upsert(wirePresence("buf-a", `p${i}`, i, i)));
  }
  loaded = module;
  store.pruneStale(0, 0);
  const applied = calls.filter((c) => c.method === "apply_server_message");
  assert.equal(applied.length, 256, "oldest entries dropped past the cap");
});

test("buffer id scheme matches the daemon's file scheme", () => {
  assert.equal(
    presenceBufferIdForPath("/work/notes/a.md"),
    "file:///work/notes/a.md",
  );
  assert.equal(
    presenceBufferIdForPath("file:///work/notes/a.md"),
    "file:///work/notes/a.md",
    "already-canonical ids pass through untouched",
  );
  assert.equal(
    presenceBufferIdForPath("notes/a.md", "/work"),
    "file:///work/notes/a.md",
    "workspace-relative paths resolve against the workspace root",
  );
  assert.equal(
    presenceBufferIdForPath("./notes/a.md", "/work/"),
    "file:///work/notes/a.md",
  );
});
