import { test } from "node:test";
import assert from "node:assert/strict";

import {
  NotesRefreshCoordinator,
  collectNotesSnapshotWithRetry,
  normalizeNotesVaultRoot,
  notesChangedTouchesActiveVault,
  synchronizeNotesVault,
} from "./notesRefresh.ts";

test("same-vault panel synchronization can replay the normalized cached vault", () => {
  const cached = normalizeNotesVaultRoot("/vault///");
  const received: Array<string | null> = [];
  const panel = { setNotesVaultRoot: (vault: string | null) => received.push(vault) };

  assert.equal(cached, "/vault");
  assert.equal(synchronizeNotesVault(cached, null), "/vault");
  synchronizeNotesVault(cached, panel);
  synchronizeNotesVault("/vault/", panel);
  assert.deepEqual(received, ["/vault", "/vault"]);
});

test("a stale vault A result cannot commit after vault B starts", () => {
  const adapter = {};
  const coordinator = new NotesRefreshCoordinator<object>();
  const a = coordinator.begin(adapter, "/vault-a/");
  const b = coordinator.begin(adapter, "/vault-b");

  assert.equal(coordinator.isCurrent(a, adapter, "/vault-a"), false);
  assert.equal(coordinator.isCurrent(b, adapter, "/vault-b/"), true);
});

test("null vault invalidation rejects an in-flight result", () => {
  const adapter = {};
  const coordinator = new NotesRefreshCoordinator<object>();
  const pending = coordinator.begin(adapter, "/vault");
  coordinator.invalidate();

  assert.equal(coordinator.isCurrent(pending, adapter, null), false);
});

test("an adapter remount rejects an old adapter's result", () => {
  const oldAdapter = {};
  const newAdapter = {};
  const coordinator = new NotesRefreshCoordinator<object>();
  const pending = coordinator.begin(oldAdapter, "/vault");

  assert.equal(coordinator.isCurrent(pending, newAdapter, "/vault"), false);
});

test("nested failure publishes no partial snapshot and retry starts fresh", async () => {
  let attempt = 0;
  const visits: string[] = [];
  let published: unknown = null;

  const snapshot = await collectNotesSnapshotWithRetry(
    "/vault",
    async (dir) => {
      visits.push(`${attempt}:${dir}`);
      if (dir === "") return [{ name: "folder", is_dir: true }];
      if (attempt === 0) throw new Error("nested listing failed");
      return [{ name: "note.md", is_dir: false }];
    },
    async () => {
      assert.equal(published, null, "failed prefix must not be committed");
      attempt += 1;
    },
  );
  published = snapshot;

  assert.deepEqual(visits, ["0:", "0:folder", "1:", "1:folder"]);
  assert.deepEqual(published, [
    { path: "/vault/folder", is_dir: true, icon: undefined },
    { path: "/vault/folder/note.md", is_dir: false, icon: undefined },
  ]);
});

test("failed desired snapshot stays dirty until a successful retry", () => {
  const adapter = {};
  const coordinator = new NotesRefreshCoordinator<object>();
  coordinator.ensure(adapter, "/vault", "workspace", 4);
  const first = coordinator.beginDesired()!;
  assert.equal(coordinator.finish(first, false), true);
  assert.equal(coordinator.needsRefresh(), true);
  assert.equal(coordinator.retryDelayMs(), 250);
  const retry = coordinator.beginDesired()!;
  assert.equal(coordinator.finish(retry, true), true);
  assert.equal(coordinator.needsRefresh(), false);
});

test("reconnect, adapter install, workspace and same-root replay invalidate old results", () => {
  const oldAdapter = {};
  const newAdapter = {};
  const coordinator = new NotesRefreshCoordinator<object>();
  coordinator.ensure(oldAdapter, "/vault/", "a", 1);
  const old = coordinator.beginDesired()!;

  coordinator.ensure(newAdapter, "/vault", "a", 2);
  assert.equal(coordinator.finish(old, true), false, "old generation/adapter is stale");
  const reconnected = coordinator.beginDesired()!;
  coordinator.ensure(newAdapter, "/vault", "b", 2);
  assert.equal(coordinator.finish(reconnected, true), false, "old workspace is stale");
  const switched = coordinator.beginDesired()!;
  assert.equal(coordinator.finish(switched, true), true);
  coordinator.ensure(newAdapter, "/vault", "b", 2, true);
  assert.equal(coordinator.needsRefresh(), true, "same-root force replay dirties snapshot");
});

test("coalesces one in-flight desired refresh", () => {
  const coordinator = new NotesRefreshCoordinator<object>();
  coordinator.ensure({}, "/vault", "workspace", 1);
  assert.ok(coordinator.beginDesired());
  assert.equal(coordinator.beginDesired(), null);
});

test("request-zero Changed refreshes only the normalized active vault", () => {
  assert.equal(notesChangedTouchesActiveVault("/vault/", "/vault"), true);
  assert.equal(notesChangedTouchesActiveVault("/other", "/vault"), false);
  assert.equal(notesChangedTouchesActiveVault("/vault", null), false);
});