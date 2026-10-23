import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";
import { GitPanelService, type GitPanelBridge } from "../services/GitService.ts";
import type { ProtocolClient } from "../workspace/ProtocolClient.ts";
import type { PullRequestDetail, GitServerMessage } from "../workspace/types.ts";

const detail: PullRequestDetail = {
  summary: { number: 42, title: "Selection", url: "https://github.com/example/repo/pull/42", author: "ada", state: "OPEN", draft: false, base_branch: "main", head_branch: "selection", base_sha: "a".repeat(40), head_sha: "b".repeat(40), review_decision: "", checks_state: "SUCCESS" },
  body: "Description", files: [], diffs: [], conversation: [], threads: [], checks: [], truncated: false,
};

test("PR, creation, commit and local-change routes are distinct and reopening preserves drafts", () => {
  const panel: any = Object.create(TerminalPanel.prototype);
  const installed: unknown[] = [];
  panel.bufferTabs = [{ kind: "file", title: "draft", path: "/repo/input.rs", modified: true }];
  panel.nextGitDiffRouteId = 1;
  panel.assignActiveTabToFocusedEditorPane = () => {};
  panel.replayBufferTabs = () => {};
  panel.activateCurrentTabContents = () => {};
  panel.scheduleDraw = () => {};
  panel.wasmAdapter = { setGitPullRequestView: (...args: unknown[]) => installed.push(args), createGitPullRequestView: (...args: unknown[]) => installed.push(args) };
  panel.openGitPullRequestTab({ root: "/repo", detail });
  const pr = panel.bufferTabs[1];
  assert.equal(pr.path, undefined);
  panel.openGitDiffTab("/repo", "input.rs");
  panel.openGitPullRequestTab({ root: "/repo", create: { branch: "selection", head: "b".repeat(40), default_branch: "main" } });
  assert.equal(panel.bufferTabs.length, 4);
  assert.equal(new Set(panel.bufferTabs.slice(1).map((tab: any) => panel.tabLifecycleKey(tab))).size, 3);
  panel.openGitPullRequestTab({ root: "/repo", detail });
  assert.equal(panel.activeTabIndex, 1);
  assert.equal(installed.length, 3, "reopening refreshes through the draft-preserving shared setter");
  assert.equal(panel.bufferTabs[0].modified, true);
  panel.openGitPullRequestTab({ root: "/other", detail });
  assert.equal(panel.bufferTabs.length, 5);
});

test("inbox is a singleton distinct from creation and individual PR pages", () => {
  const panel: any = Object.create(TerminalPanel.prototype);
  const opened: unknown[] = [];
  panel.bufferTabs = [];
  panel.nextGitDiffRouteId = 1;
  panel.assignActiveTabToFocusedEditorPane = () => {};
  panel.replayBufferTabs = () => {};
  panel.activateCurrentTabContents = () => {};
  panel.scheduleDraw = () => {};
  panel.wasmAdapter = { openGitPullRequestInbox: (...args: unknown[]) => opened.push(args) };
  panel.openGitPullRequestTab({ root: "/repo", inbox: true });
  const inbox = panel.bufferTabs[0];
  assert.equal(inbox.title, "Pull Requests");
  assert.equal(inbox.path, undefined, "inbox never owns a canonical source path");
  panel.openGitPullRequestTab({ root: "/repo", create: { branch: "topic", head: "sha", default_branch: "main" } });
  panel.openGitPullRequestTab({ root: "/repo", detail });
  panel.openGitDiffTab("/repo", "input.rs");
  assert.equal(new Set(panel.bufferTabs.map((tab: any) => panel.tabLifecycleKey(tab))).size, 4);
  panel.openGitPullRequestTab({ root: "/repo", inbox: true });
  assert.equal(panel.bufferTabs.length, 4);
  assert.equal(panel.activeTabIndex, 0);
  assert.deepEqual(opened, [[inbox.diffRouteId, "/repo"], [inbox.diffRouteId, "/repo"]]);
  panel.openGitPullRequestTab({ root: "/other", inbox: true });
  assert.equal(panel.bufferTabs.length, 5);
});

test("PR replies use their own bridge and bypass local save-stage gates", async () => {
  let callback!: (id: number, json: string) => void;
  const applied: unknown[] = [];
  const client = { async requestGit(): Promise<GitServerMessage> { return { PullRequest: { reply: { Detail: { detail } } } }; } } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_apply_pull_request_reply(json) { applied.push(JSON.parse(json)); },
    gitPanelPrepareStage() { throw new Error("PR review must not stage source"); },
    gitPanelPrepareRestore() { throw new Error("PR review must not restore source"); },
  };
  new GitPanelService(client, bridge).install();
  callback(1, JSON.stringify({ PullRequest: { request: { Read: { number: 42 } } } }));
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.deepEqual(applied, [{ Detail: { detail } }]);
});

test("PR transport errors preserve a typed retry state and never poison local Git status", async () => {
  let callback!: (id: number, json: string) => void;
  const applied: any[] = [];
  const client = { async requestGit() { throw new Error("connection unavailable"); } } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    set_git_panel_ops(cb) { callback = cb; },
    git_panel_apply_pull_request_reply(json) { applied.push(JSON.parse(json)); },
    git_panel_set_error() { throw new Error("PR error leaked into local status"); },
  };
  new GitPanelService(client, bridge).install();
  callback(1, JSON.stringify({ PullRequest: { request: { Read: { number: 42 } } } }));
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(applied[0].Error.kind, "Unavailable");
});

test("declining checkout confirmation never sends a hosted mutation", async () => {
  let callback!: (id: number, json: string) => void;
  let sent = 0;
  const applied: any[] = [];
  const client = { async requestGit() { sent++; throw new Error("must not run"); } } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = {
    set_git_panel_ops(cb) { callback = cb; },
    gitPanelPreparePullRequest() { return false; },
    git_panel_apply_pull_request_reply(json) { applied.push(JSON.parse(json)); },
  };
  new GitPanelService(client, bridge).install();
  callback(1, JSON.stringify({ PullRequest: { request: { Checkout: { number: 42, expected_head: "b".repeat(40), new_worktree: false } } } }));
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(sent, 0);
  assert.equal(applied[0].Error.kind, "Conflict");
});

test("a PR reply cannot repaint a different workspace or connection generation", async () => {
  let callback!: (id: number, json: string) => void;
  let finish!: (reply: GitServerMessage) => void;
  let scope = "/repo";
  let generation = 1;
  const applied: string[] = [];
  const client = { getGeneration() { return generation; }, requestGit() { return new Promise<GitServerMessage>(resolve => { finish = resolve; }); } } as unknown as ProtocolClient;
  const bridge: GitPanelBridge = { gitPanelContext() { return scope; }, set_git_panel_ops(cb) { callback = cb; }, git_panel_apply_pull_request_reply(json) { applied.push(json); } };
  new GitPanelService(client, bridge).install();
  callback(1, JSON.stringify({ PullRequest: { request: { Read: { number: 42 } } } }));
  await new Promise<void>(resolve => setImmediate(resolve));
  scope = "/other"; generation++;
  finish({ PullRequest: { reply: { Detail: { detail } } } });
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.deepEqual(applied, []);
});
