import assert from "node:assert/strict";
import test from "node:test";

import { GitPanelService, type GitPanelBridge } from "./GitService";
import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { GitClientMessage, GitServerMessage } from "../workspace/types";

const tick = () => new Promise<void>(resolve => setImmediate(resolve));
const snapshot: GitServerMessage = { ChangedFiles: {
  files: [{ path: "joined.rs", status: "Modified", staged: false, additions: 1, deletions: 0 }],
  branch: "joined", error: null,
} };
const log = (sha: string): GitServerMessage => ({ Log: { commits: [
  { sha, short_sha: sha, author: "Ada", message: sha, timestamp: 1 },
] } });
const diff = (patch: string): GitServerMessage => ({ FileDiffs: { diffs: [{ path: "joined.rs", patch }] } });

function harness() {
  let callback!: (id: number, envelope: string) => void;
  let root: string | null = "/joined/nondefault";
  let generation = 7;
  const pending: { message: GitClientMessage; scope: string | null | undefined; resolve: (reply: GitServerMessage) => void }[] = [];
  const applied: { kind: string; value: unknown }[] = [];
  const client = {
    getGeneration: () => generation,
    requestGit(message: GitClientMessage, scope?: string | null): Promise<GitServerMessage> {
      return new Promise(resolve => pending.push({ message, scope, resolve }));
    },
  } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    gitPanelContext: () => root,
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_apply_changed_files(json) { applied.push({ kind: "files", value: JSON.parse(json) }); },
    git_panel_apply_file_diffs(json) { applied.push({ kind: "diff", value: JSON.parse(json) }); },
    git_panel_set_latest_commit(json) { applied.push({ kind: "log", value: JSON.parse(json) }); },
    git_panel_set_error(message) { applied.push({ kind: "error", value: message }); },
  };
  new GitPanelService(client, bridge).install();
  return {
    pending, applied,
    refresh: () => callback(1, JSON.stringify("ChangedFiles")),
    switchRoot: (next: string | null) => { root = next; },
    reconnect: () => { generation++; },
  };
}

test("nondefault joined root is retained as the scope of both refresh follow-ups", async () => {
  const h = harness();
  h.refresh();
  await tick();
  assert.equal(h.pending[0].scope, "/joined/nondefault");
  h.pending[0].resolve(snapshot);
  await tick();
  assert.deepEqual(h.pending.map(p => ({ message: p.message, scope: p.scope })), [
    { message: "ChangedFiles", scope: "/joined/nondefault" },
    { message: { DiffFiles: { paths: ["joined.rs"] } }, scope: "/joined/nondefault" },
    { message: { Log: { max_count: 1 } }, scope: "/joined/nondefault" },
  ]);
  h.pending[1].resolve(diff("joined patch"));
  h.pending[2].resolve(log("joined commit"));
  await tick();
  assert.deepEqual(h.applied.map(a => a.kind), ["files", "diff", "log"]);
});

for (const change of ["workspace", "retired", "connection"] as const) {
  for (const phase of ["snapshot", "follow-ups"] as const) {
    test(`${change} change during ${phase} rejects old replies without a newer sequence or cancelling new state`, async () => {
      const h = harness();
      h.refresh();
      await tick();
      if (phase === "follow-ups") {
        h.pending[0].resolve(snapshot);
        await tick();
      }
      if (change === "connection") h.reconnect();
      else h.switchRoot(change === "retired" ? null : "/joined/new");
      const before = h.applied.slice();
      // No new request yet: the old sequence numbers still match.
      if (phase === "snapshot") h.pending[0].resolve(snapshot);
      else {
        h.pending[1].resolve(diff("old"));
        h.pending[2].resolve(log("old"));
      }
      await tick();
      assert.deepEqual(h.applied, before);
      assert.equal(h.pending.length, phase === "snapshot" ? 1 : 3);

      h.switchRoot("/joined/new");
      const start = h.pending.length;
      h.refresh();
      await tick();
      h.pending[start].resolve(snapshot);
      await tick();
      assert.equal(h.pending[start + 1].scope, "/joined/new");
      assert.equal(h.pending[start + 2].scope, "/joined/new");
      h.pending[start + 1].resolve(diff("new"));
      h.pending[start + 2].resolve(log("new"));
      await tick();
      assert.deepEqual(h.applied.slice(before.length).map(a => a.kind), ["files", "diff", "log"]);
    });
  }
}

test("late old follow-ups do not apply or cancel a new workspace's pending follow-ups", async () => {
  const h = harness();
  h.refresh();
  await tick();
  h.pending[0].resolve(snapshot);
  await tick();
  h.switchRoot("/joined/new");
  h.refresh();
  await tick();
  h.pending[3].resolve(snapshot);
  await tick();
  h.pending[1].resolve(diff("old"));
  h.pending[2].resolve(log("old"));
  await tick();
  assert.deepEqual(h.applied.map(a => a.kind), ["files", "files"]);
  h.pending[4].resolve(diff("new"));
  h.pending[5].resolve(log("new"));
  await tick();
  assert.deepEqual(h.applied.slice(2), [
    { kind: "diff", value: [{ path: "joined.rs", patch: "new" }] },
    { kind: "log", value: [{ sha: "new", short_sha: "new", author: "Ada", message: "new", timestamp: 1 }] },
  ]);
});

test("late old snapshot cannot invalidate a new workspace's pending follow-ups", async () => {
  const h = harness();
  h.refresh();
  await tick();
  h.switchRoot("/joined/new");
  h.refresh();
  await tick();
  h.pending[1].resolve(snapshot);
  await tick();
  h.pending[0].resolve(snapshot);
  await tick();
  assert.equal(h.pending.length, 4, "old snapshot must not start follow-ups or advance their sequences");
  h.pending[2].resolve(diff("new"));
  h.pending[3].resolve(log("new"));
  await tick();
  assert.deepEqual(h.applied.map(a => a.kind), ["files", "diff", "log"]);
});
