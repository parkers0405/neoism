import assert from "node:assert/strict";
import test from "node:test";
import { DaemonGitService, GitPanelService, type GitPanelBridge } from "./GitService";
import type { ProtocolClient } from "../workspace/ProtocolClient";
import type { GitClientMessage, GitServerMessage } from "../workspace/types";

test("baseline is requested on demand and forwarded without changing newlines", async () => {
  const sent: GitClientMessage[] = [];
  const base = { path: "src/file.rs", text: "HEAD\r\n\n" };
  const client = {
    async requestGit(message: GitClientMessage): Promise<GitServerMessage> {
      sent.push(message);
      return { BaseFile: base };
    },
  } as unknown as ProtocolClient;
  assert.deepEqual(await new DaemonGitService(client).readBase(base.path), base);
  let callback: ((id: number, envelope: string) => void) | undefined;
  const applied: unknown[] = [];
  const bridge: GitPanelBridge = {
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_apply_base_file(json) { applied.push(JSON.parse(json)); },
  };
  new GitPanelService(client, bridge).install();
  assert.equal(sent.length, 1, "install must not prefetch baselines");
  callback!(1, JSON.stringify({ ReadBase: { path: base.path } }));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, [{ ReadBase: { path: base.path } }, { ReadBase: { path: base.path } }]);
  assert.deepEqual(applied, [base]);
});
