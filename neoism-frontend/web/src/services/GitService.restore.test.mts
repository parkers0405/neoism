import assert from "node:assert/strict";
import test from "node:test";
import { DaemonGitService, GitPanelService, type GitPanelBridge } from "./GitService";
import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { GitClientMessage, GitServerMessage } from "../workspace/types";

const changed: GitServerMessage = {
  ChangedFiles: { files: [], branch: "main", error: null },
};

test("restore sends the exact path and returns the refreshed snapshot", async () => {
  const sent: GitClientMessage[] = [];
  const client = {
    async requestGit(message: GitClientMessage) {
      sent.push(message);
      return changed;
    },
  } as unknown as ProtocolClient;
  const result = await new DaemonGitService(client).restore("literal* name.rs");
  assert.deepEqual(sent, [{ Restore: { path: "literal* name.rs" } }]);
  assert.deepEqual(result, changed.ChangedFiles);
});

test("panel dispatch forwards Restore and applies its refreshed file list", async () => {
  let callback: ((id: number, envelope: string) => void) | undefined;
  const sent: GitClientMessage[] = [];
  const applied: unknown[] = [];
  const client = {
    async requestGit(message: GitClientMessage) {
      sent.push(message);
      return changed;
    },
  } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    git_panel_prepare_restore() { return true; },
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_apply_changed_files(json) { applied.push(JSON.parse(json)); },
  };
  new GitPanelService(client, bridge).install();
  callback!(1, JSON.stringify({ Restore: { path: "file.rs" } }));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, [{ Restore: { path: "file.rs" } }, { Log: { max_count: 1 } }]);
  assert.deepEqual(applied, [changed.ChangedFiles]);
});
