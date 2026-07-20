import { test } from "node:test";
import assert from "node:assert/strict";

import {
  PresencePublisher,
  PRESENCE_HEARTBEAT_INTERVAL_MS,
} from "./PresencePublisher.ts";
import { stablePresenceColor } from "./presenceColor.ts";
import type {
  CrdtClientMessage,
  CrdtPeerPresence,
} from "../workspace/types.ts";

// These mirror the shared Rust publisher tests in
// `neoism-frontend/shared/src/editor/crdt/remote_presence.rs` so web
// and desktop peers stay behaviorally identical on the wire.

function published(messages: CrdtClientMessage[]): CrdtPeerPresence[] {
  return messages.flatMap((message) =>
    "PublishPresence" in message ? [message.PublishPresence.presence] : [],
  );
}

function cursor(line: number, column: number) {
  return { line, column, offset: null };
}

test("publisher coalesces rapid movement to the rate limit", () => {
  const publisher = new PresencePublisher("me", "My Browser");
  // First sight publishes immediately.
  const first = publisher.tick(
    { bufferId: "buf-a", cursor: cursor(0, 0) },
    1_000,
  );
  assert.equal(published(first).length, 1);

  // 60 frames of movement over ~1s (16ms apart) must publish at most
  // ceil(1000/75)+1 times, not 60.
  let sent = 0;
  for (let frame = 1; frame <= 60; frame += 1) {
    const messages = publisher.tick(
      { bufferId: "buf-a", cursor: cursor(frame, 0) },
      1_000 + frame * 16,
    );
    sent += published(messages).length;
  }
  assert.ok(
    sent >= 1 && sent <= 14,
    `expected ~13Hz coalescing, got ${sent} publishes`,
  );
});

test("publisher is silent when nothing changed until heartbeat", () => {
  const publisher = new PresencePublisher("me", "My Browser");
  publisher.tick({ bufferId: "buf-a", cursor: cursor(1, 2) }, 1_000);

  for (let frame = 1; frame <= 10; frame += 1) {
    const messages = publisher.tick(
      { bufferId: "buf-a", cursor: cursor(1, 2) },
      1_000 + frame * 100,
    );
    assert.equal(messages.length, 0, "unchanged cursor must not republish");
  }

  const heartbeat = publisher.tick(
    { bufferId: "buf-a", cursor: cursor(1, 2) },
    1_000 + PRESENCE_HEARTBEAT_INTERVAL_MS,
  );
  assert.equal(published(heartbeat).length, 1);
});

test("publisher clears old buffer when switching or closing", () => {
  const publisher = new PresencePublisher("me", "My Browser");
  publisher.tick({ bufferId: "buf-a", cursor: cursor(1, 2) }, 1_000);

  const switched = publisher.tick(
    { bufferId: "buf-b", cursor: cursor(0, 0) },
    2_000,
  );
  assert.deepEqual(switched[0], {
    ClearPresence: { buffer_id: "buf-a", peer_id: "me" },
  });
  const upserts = published(switched);
  assert.equal(upserts.length, 1);
  assert.equal(upserts[0].buffer_id, "buf-b");

  const closed = publisher.tick(null, 3_000);
  assert.deepEqual(closed[0], {
    ClearPresence: { buffer_id: "buf-b", peer_id: "me" },
  });
  assert.equal(publisher.tick(null, 4_000).length, 0, "clear only once");
});

test("publisher stamps identity and stable color", () => {
  const publisher = new PresencePublisher("user@web", "Chrome · web");
  const messages = publisher.tick(
    {
      bufferId: "buf-a",
      cursor: cursor(2, 4),
      selection: { anchor: cursor(2, 4), head: cursor(2, 9) },
    },
    1_000,
  );
  const presence = published(messages)[0];
  assert.equal(presence.peer_id, "user@web");
  assert.equal(presence.display_name, "Chrome · web");
  assert.deepEqual(presence.color, stablePresenceColor("user@web"));
  assert.ok(presence.selection);
  assert.equal(presence.updated_at_ms, 1_000);
});

test("publisher republishes a selection change at the rate limit", () => {
  const publisher = new PresencePublisher("me", "My Browser");
  publisher.tick({ bufferId: "buf-a", cursor: cursor(1, 1) }, 1_000);
  const withSelection = publisher.tick(
    {
      bufferId: "buf-a",
      cursor: cursor(1, 1),
      selection: { anchor: cursor(1, 1), head: cursor(2, 0) },
    },
    1_100,
  );
  assert.equal(published(withSelection).length, 1);
});
