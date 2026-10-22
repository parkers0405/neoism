import assert from "node:assert/strict";
import test from "node:test";

import { DaemonGitService, GitPanelService, type GitPanelBridge } from "./GitService";
import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { GitClientMessage, GitServerMessage } from "../workspace/types";

const changed = (error: string | null = null): GitServerMessage => ({
  ChangedFiles: { files: [], branch: "main", upstream: "origin/main", error },
});

test("publish uses the distinct tracking operation", async () => {
  const sent: GitClientMessage[] = [];
  const client = {
    async requestGit(message: GitClientMessage) {
      sent.push(message);
      return changed();
    },
  } as unknown as ProtocolClient;
  const reply = await new DaemonGitService(client).publish();
  assert.deepEqual(sent, ["Publish"]);
  assert.equal(reply.upstream, "origin/main");
});

test("a clean snapshot invalidates an older in-flight diff fetch", async () => {
  let callback: ((id: number, envelope: string) => void) | undefined;
  let resolveDiff!: (reply: GitServerMessage) => void;
  let statusRequests = 0;
  const applied: string[] = [];
  const client = {
    async requestGit(message: GitClientMessage): Promise<GitServerMessage> {
      if (typeof message === "object" && "DiffFiles" in message) {
        return new Promise((resolve) => { resolveDiff = resolve; });
      }
      if (statusRequests++ === 0) {
        return { ChangedFiles: { files: [{ path: "a.rs", status: "Modified", staged: false, additions: 1, deletions: 0 }], branch: "main", error: null } };
      }
      return changed();
    },
  } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_apply_changed_files() {},
    git_panel_apply_file_diffs(json) { applied.push(json); },
  };
  new GitPanelService(client, bridge).install();
  callback!(1, JSON.stringify("ChangedFiles"));
  await new Promise<void>((resolve) => setImmediate(resolve));
  callback!(2, JSON.stringify("ChangedFiles"));
  await new Promise<void>((resolve) => setImmediate(resolve));
  resolveDiff({ FileDiffs: { diffs: [{ path: "a.rs", patch: "stale" }] } });
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(applied, []);
});

test("latest-commit follow-up cannot recursively replay an unexpected status reply", async () => {
  let callback: ((id: number, envelope: string) => void) | undefined;
  const sent: GitClientMessage[] = [];
  const client = {
    async requestGit(message: GitClientMessage) {
      sent.push(message);
      if (sent.length > 3) throw new Error("recursive status replay");
      return changed();
    },
  } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = { set_git_panel_ops(cb) { callback = cb; }, git_panel_apply_changed_files() {} };
  new GitPanelService(client, bridge).install();
  callback!(1, JSON.stringify("ChangedFiles"));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, ["ChangedFiles", { Log: { max_count: 1 } }]);
});

test("latest-commit metadata drops stale replies after a newer status refresh", async () => {
  let callback: ((id: number, envelope: string) => void) | undefined;
  const resolveLogs: ((reply: GitServerMessage) => void)[] = [];
  const applied: string[] = [];
  const client = {
    async requestGit(message: GitClientMessage): Promise<GitServerMessage> {
      if (typeof message === "object" && "Log" in message) return new Promise((resolve) => resolveLogs.push(resolve));
      return changed();
    },
  } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    set_git_panel_ops(cb) { callback = cb; }, git_panel_apply_changed_files() {},
    git_panel_set_latest_commit(json) { applied.push(json); },
  };
  new GitPanelService(client, bridge).install();
  callback!(1, JSON.stringify("ChangedFiles"));
  await new Promise<void>((resolve) => setImmediate(resolve));
  callback!(2, JSON.stringify("ChangedFiles"));
  await new Promise<void>((resolve) => setImmediate(resolve));
  const log = (sha: string): GitServerMessage => ({ Log: { commits: [{ sha, short_sha: sha, author: "Ada", message: "release", timestamp: 1 }] } });
  resolveLogs[1](log("new"));
  resolveLogs[0](log("old"));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(applied.length, 1);
  assert.equal(JSON.parse(applied[0])[0].sha, "new");
});

test("commit acknowledgement is tied to the successful submitted operation", async () => {
  for (const reply of [changed(), changed("hook rejected"), { Error: { message: "signing failed" } }]) {
    let callback: ((id: number, envelope: string) => void) | undefined;
    const acknowledged: string[] = [];
    const client = {
      async requestGit() { return reply; },
    } as unknown as ProtocolClient;
    const bridge: GitPanelBridge = {
      set_git_panel_ops(cb) { callback = cb; },
      git_panel_apply_changed_files() {},
      git_panel_set_error() {},
      git_panel_commit_succeeded(message) { acknowledged.push(message); },
    };
    new GitPanelService(client, bridge).install();
    callback!(1, JSON.stringify({ Commit: { message: "Keep this exact draft" } }));
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.deepEqual(acknowledged, "ChangedFiles" in reply && reply.ChangedFiles.error === null
      ? ["Keep this exact draft"] : []);
    callback!(2, JSON.stringify("ChangedFiles"));
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.ok(acknowledged.length <= 1, "status refresh cannot acknowledge a commit");
  }
});
