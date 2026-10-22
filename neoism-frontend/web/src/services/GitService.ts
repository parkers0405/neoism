// JS-side bridge for `neoism-ui::services::GitService`.
//
// Mirrors the Rust trait surface but speaks the richer wire protocol
// from `neoism-protocol/src/git.rs` (status entries, diff hunks, log
// commits). The Rust `GitService::status` returns a coarse
// `GitStatus { branch, dirty }`; the daemon currently sends a list of
// entries — the wasm chrome can derive the coarse summary on its side.
//
// Pass 2 (write parity): also hosts `GitPanelService`, the daemon
// marshaller behind the wasm `DaemonGitDiffIo` provider. The shared
// git side panel fires serialized `GitClientMessage` envelopes through
// the bridge's `set_git_panel_ops` callback; this service forwards
// them over the WebSocket and pushes the replies back through the
// bridge's `git_panel_*` entry points — the web twin of the desktop's
// `NativeGitDiffIo` shell-outs.

import { presenceBufferIdForPath } from "../presence/presence";
import type { ProtocolClient } from "../workspace/ProtocolClient";
import type {
  GitCommitDetail,
  CommitSummary,
  DiffHunk,
  GitChangeBlock,
  GitClientMessage,
  GitFileChange,
  GitFileDiff,
  GitServerMessage,
  GitStatusEntry,
} from "../workspace/types";

export type { CommitSummary, DiffHunk, GitFileChange, GitFileDiff, GitStatusEntry };

export interface GitService {
  status(): Promise<GitStatusEntry[]>;
  diff(path?: string | null): Promise<DiffHunk[]>;
  log(maxCount?: number | null): Promise<CommitSummary[]>;
  readCommit(revision: string): Promise<GitCommitDetail>;
}

function expectVariant<K extends string>(
  reply: GitServerMessage,
  tag: K,
): Extract<GitServerMessage, Record<K, unknown>> {
  if (tag in reply) {
    return reply as Extract<GitServerMessage, Record<K, unknown>>;
  }
  if ("Error" in reply) {
    throw new Error(`git service error: ${reply.Error.message}`);
  }
  throw new Error(`unexpected reply variant for ${tag}`);
}

export class DaemonGitService implements GitService {
  constructor(private readonly client: ProtocolClient) {}

  async status(): Promise<GitStatusEntry[]> {
    const msg: GitClientMessage = "Status";
    const reply = await this.client.requestGit(msg);
    return expectVariant(reply, "Status").Status.entries;
  }

  async diff(path: string | null = null): Promise<DiffHunk[]> {
    const msg: GitClientMessage = { Diff: { path } };
    const reply = await this.client.requestGit(msg);
    return expectVariant(reply, "Diff").Diff.hunks;
  }

  async log(maxCount: number | null = null): Promise<CommitSummary[]> {
    const msg: GitClientMessage = { Log: { max_count: maxCount } };
    const reply = await this.client.requestGit(msg);
    return expectVariant(reply, "Log").Log.commits;
  }

  async readCommit(revision: string): Promise<GitCommitDetail> {
    const reply = await this.client.requestGit({ ReadCommit: { revision } });
    return expectVariant(reply, "CommitDetail").CommitDetail.detail;
  }

  // ── Write-parity verbs (Pass 2) ────────────────────────────────────
  // Each mutation replies with a refreshed `ChangedFiles` (desktop
  // parity: mutate, then re-collect), so callers get the post-op list
  // — with real staged bits — in one round trip.

  async readBase(path: string) {
    const reply = await this.client.requestGit({ ReadBase: { path } });
    return expectVariant(reply, "BaseFile").BaseFile;
  }

  async changedFiles(): Promise<
    Extract<GitServerMessage, { ChangedFiles: unknown }>["ChangedFiles"]
  > {
    const reply = await this.client.requestGit("ChangedFiles");
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async stage(path: string) {
    const reply = await this.client.requestGit({ Stage: { path } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async applyBlock(path: string, block: GitChangeBlock, restore = false) {
    const reply = await this.client.requestGit({ ApplyBlock: { path, block, restore } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async unstage(path: string) {
    const reply = await this.client.requestGit({ Unstage: { path } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  /** Caller must confirm discarding unstaged changes before invoking. */
  async restore(path: string) {
    const reply = await this.client.requestGit({ Restore: { path } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async commit(message: string) {
    const reply = await this.client.requestGit({ Commit: { message } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async publish() {
    const reply = await this.client.requestGit("Publish");
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async branches(): Promise<string[]> {
    const reply = await this.client.requestGit("Branches");
    return expectVariant(reply, "Branches").Branches.branches;
  }

  async checkout(branch: string) {
    const reply = await this.client.requestGit({ Checkout: { branch } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async diffFiles(paths: string[]): Promise<GitFileDiff[]> {
    const reply = await this.client.requestGit({ DiffFiles: { paths } });
    return expectVariant(reply, "FileDiffs").FileDiffs.diffs;
  }
}

// ── Git side-panel daemon marshaller ─────────────────────────────────

/**
 * Raw wasm-bridge surface the panel marshaller drives. These are the
 * snake_case `#[wasm_bindgen]` methods on `ChromeBridge`
 * (`wasm/src/rendered/panels.rs`); the camelCase spellings are the
 * adapter passthroughs (`createTerminal.ts`) when present. All
 * optional — a stub adapter (wasm failed to boot) just leaves the
 * panel read-only.
 */
export interface GitPanelBridge {
  /** Host identity guard: null means this adapter has been retired. */
  gitPanelContext?(): string | null;
  prepare_git_stage_block?(path: string, blockJson: string): boolean;
  prepareGitStageBlock?(path: string, blockJson: string): boolean;
  git_panel_restore_identity?(path: string): string;
  gitPanelRestoreIdentity?(path: string): string;
  git_panel_block_restored?(identityJson: string, bufferId: string): boolean;
  gitPanelBlockRestored?(identityJson: string, bufferId: string): boolean;
  git_panel_prepare_stage?(path: string): boolean;
  gitPanelPrepareStage?(path: string): boolean;
  git_panel_prepare_restore?(path: string): boolean;
  gitPanelPrepareRestore?(path: string): boolean;
  git_panel_set_latest_commit?(json: string): void;
  gitPanelSetLatestCommit?(json: string): void;
  git_panel_apply_commit_detail?(json: string): void;
  gitPanelApplyCommitDetail?(json: string): void;
  git_panel_apply_base_file?(replyJson: string): void;
  gitPanelApplyBaseFile?(replyJson: string): void;
  set_git_panel_ops?(cb: (reqId: number, envelopeJson: string) => void): void;
  setGitPanelOps?(cb: (reqId: number, envelopeJson: string) => void): void;
  git_panel_apply_changed_files?(replyJson: string): void;
  gitPanelApplyChangedFiles?(replyJson: string): void;
  git_panel_set_branches?(branchesJson: string): void;
  gitPanelSetBranches?(branchesJson: string): void;
  git_panel_apply_file_diffs?(diffsJson: string): void;
  gitPanelApplyFileDiffs?(diffsJson: string): void;
  git_panel_set_error?(message: string): void;
  gitPanelSetError?(message: string): void;
  git_panel_commit_succeeded?(message: string): void;
  gitPanelCommitSucceeded?(message: string): void;
}

/**
 * Resolve the raw bridge behind whatever object `onBridgeReady`
 * delivered. `TerminalPanel` hands out the `ChromeAdapter` wrapper,
 * whose `inner` field is the actual wasm `ChromeBridge`; if the
 * adapter itself ever grows camelCase passthroughs those win. Probing
 * both keeps this working without touching the adapter today.
 */
function bridgeSurfaces(bridge: unknown): GitPanelBridge[] {
  const out: GitPanelBridge[] = [];
  if (bridge && typeof bridge === "object") {
    out.push(bridge as GitPanelBridge);
    const inner = (bridge as { inner?: unknown }).inner;
    if (inner && typeof inner === "object") {
      out.push(inner as GitPanelBridge);
    }
  }
  return out;
}

/**
 * Installs the `set_git_panel_ops` callback on the wasm bridge and
 * services the envelopes the shared git panel's `DaemonGitDiffIo`
 * provider fires: forward each `GitClientMessage` to the daemon,
 * apply the reply back into the panel, and follow every refreshed
 * file list with a `DiffFiles` fetch so the diff card carries
 * desktop-parity (`git diff HEAD`) patch text.
 */
export class GitPanelService {
  private surfaces: GitPanelBridge[] = [];
  /** Drops stale diff-fetch results when refreshes overlap. */
  private diffFetchSeq = 0;
  private latestCommitSeq = 0;

  constructor(
    private readonly client: ProtocolClient,
    private readonly bridge: unknown,
  ) {}

  /** Idempotent; safe to call again after a bridge swap. */
  install(): void {
    this.surfaces = bridgeSurfaces(this.bridge);
    const setOps = this.surfaces
      .map((s) => {
        const set = s.setGitPanelOps ?? s.set_git_panel_ops;
        return set ? set.bind(s) : null;
      })
      .find((set) => set !== null);
    setOps?.((_reqId, envelopeJson) => {
      void this.handleEnvelope(envelopeJson);
    });
  }

  /** Invoke the first of `names` found on any bridge surface (adapter
   *  camelCase preferred, raw snake_case fallback). */
  private call(names: Array<keyof GitPanelBridge>, ...args: string[]): void {
    for (const surface of this.surfaces) {
      for (const name of names) {
        const method = surface[name] as ((...a: string[]) => void) | undefined;
        if (method) {
          method.apply(surface, args);
          return;
        }
      }
    }
  }

  private prepareMutation(names: Array<keyof GitPanelBridge>, path: string): boolean {
    for (const surface of this.surfaces) {
      for (const name of names) {
        const method = surface[name] as ((path: string) => boolean) | undefined;
        if (method) return method.call(surface, path) === true;
      }
    }
    this.call(["gitPanelSetError", "git_panel_set_error"], "Mutation unavailable: reload to connect the canonical editor safety gate.");
    return false;
  }

  private prepareBlock(path: string, block: GitChangeBlock, restore: boolean): boolean {
    if (restore) return this.prepareMutation(["gitPanelPrepareRestore", "git_panel_prepare_restore"], path);
    for (const surface of this.surfaces) {
      const prepare = surface.prepareGitStageBlock ?? surface.prepare_git_stage_block;
      if (prepare) return prepare.call(surface, path, JSON.stringify(block)) === true;
    }
    this.call(["gitPanelSetError", "git_panel_set_error"], "Block mutation unavailable: reload to connect the canonical editor safety gate.");
    return false;
  }

  private async handleEnvelope(envelopeJson: string): Promise<void> {
    let message: GitClientMessage;
    try {
      message = JSON.parse(envelopeJson) as GitClientMessage;
    } catch {
      return;
    }
    const contextSurface = this.surfaces.find(s => s.gitPanelContext);
    const context = contextSurface ? () => contextSurface.gitPanelContext?.() : undefined;
    const scope = context?.();
    // Keep this owner/root/connection guard for the entire refresh chain;
    // follow-up sequence numbers alone cannot detect a workspace switch.
    const generation = this.client.getGeneration?.();
    const current = () => scope !== null && (!context || context() === scope)
      && this.client.getGeneration?.() === generation;
    try {
      // Providers invoke this callback while ChromeBridge is mutably borrowed.
      // Unwind that stack before calling any wasm preflight (no re-entrant borrow).
      await Promise.resolve();
      if (!current()) return;
      if (typeof message === "object") {
        if ("ApplyBlock" in message && !this.prepareBlock(message.ApplyBlock.path, message.ApplyBlock.block, message.ApplyBlock.restore)) return;
        if ("Stage" in message && !this.prepareMutation(["gitPanelPrepareStage", "git_panel_prepare_stage"], message.Stage.path)) return;
        if ("Restore" in message && !this.prepareMutation(["gitPanelPrepareRestore", "git_panel_prepare_restore"], message.Restore.path)) return;
      }
      const restore = typeof message === "object" && "ApplyBlock" in message && message.ApplyBlock.restore ? message.ApplyBlock : null;
      const reloadSurface = restore ? this.surfaces.find(s => (s.gitPanelRestoreIdentity ?? s.git_panel_restore_identity) && (s.gitPanelBlockRestored ?? s.git_panel_block_restored)) : undefined;
      if (restore && !reloadSurface) throw new Error("Restore unavailable: reload to connect canonical source reconciliation.");
      const identityJson = restore && reloadSurface
        ? (reloadSurface.gitPanelRestoreIdentity ?? reloadSurface.git_panel_restore_identity)!.call(reloadSurface, restore.path) : "null";
      const identity = JSON.parse(identityJson) as {root: string; absolute: string; revision: string; text: string} | null;
      const reply = scope === undefined ? await this.client.requestGit(message) : await this.client.requestGit(message, scope);
      if (!current()) return;
      if (restore && identity && reloadSurface && "ChangedFiles" in reply && reply.ChangedFiles.error === null) {
        // The daemon has already folded actual disk bytes into its canonical
        // document. Ask the retained replica for an authoritative delta; never
        // reconstruct CRLF/EOF from display rows or publish a disk replacement.
        (reloadSurface.gitPanelBlockRestored ?? reloadSurface.git_panel_block_restored)!.call(reloadSurface, identityJson, presenceBufferIdForPath(identity.absolute, identity.root));
      }
      this.applyReply(reply, scope, current);
      if (typeof message === "object" && "Commit" in message
          && "ChangedFiles" in reply && reply.ChangedFiles.error === null) {
        this.call(
          ["gitPanelCommitSucceeded", "git_panel_commit_succeeded"],
          message.Commit.message,
        );
      }
    } catch (err) {
      if (!current()) return;
      this.call(
        ["gitPanelSetError", "git_panel_set_error"],
        err instanceof Error ? err.message : String(err),
      );
    }
  }

  private applyReply(reply: GitServerMessage, scope: string | null | undefined, current: () => boolean): void {
    if (!current()) return;
    if ("Log" in reply) {
      this.call(["gitPanelSetLatestCommit", "git_panel_set_latest_commit"], JSON.stringify(reply.Log.commits));
      return;
    }
    if ("CommitDetail" in reply) {
      this.call(["gitPanelApplyCommitDetail", "git_panel_apply_commit_detail"], JSON.stringify(reply.CommitDetail.detail));
      return;
    }
    if ("BaseFile" in reply) {
      this.call(["gitPanelApplyBaseFile", "git_panel_apply_base_file"], JSON.stringify(reply.BaseFile));
      return;
    }
    if ("ChangedFiles" in reply) {
      this.call(
        ["gitPanelApplyChangedFiles", "git_panel_apply_changed_files"],
        JSON.stringify(reply.ChangedFiles),
      );
      if (!current()) return;
      void this.refreshDiffs(reply.ChangedFiles.files.map((f) => f.path), scope, current);
      const latestSeq = ++this.latestCommitSeq;
      void this.client.requestGit({ Log: { max_count: 1 } }, scope).then(log => {
        if (current() && latestSeq === this.latestCommitSeq && "Log" in log) this.applyReply(log, scope, current);
      }).catch(() => {});
      return;
    }
    if ("Branches" in reply) {
      this.call(
        ["gitPanelSetBranches", "git_panel_set_branches"],
        JSON.stringify(reply.Branches.branches),
      );
      return;
    }
    if ("Error" in reply) {
      this.call(
        ["gitPanelSetError", "git_panel_set_error"],
        reply.Error.message,
      );
    }
  }

  /** Fetch per-file `git diff HEAD` patches for the refreshed list and
   *  push them into the panel's diff cache. A newer refresh supersedes
   *  an in-flight fetch. */
  private async refreshDiffs(paths: string[], scope: string | null | undefined, current: () => boolean): Promise<void> {
    if (!current()) return;
    const seq = ++this.diffFetchSeq;
    if (paths.length === 0) return;
    let reply: GitServerMessage;
    try {
      reply = await this.client.requestGit({ DiffFiles: { paths } }, scope);
    } catch {
      return;
    }
    if (!current() || seq !== this.diffFetchSeq) return;
    if ("FileDiffs" in reply) {
      this.call(
        ["gitPanelApplyFileDiffs", "git_panel_apply_file_diffs"],
        JSON.stringify(reply.FileDiffs.diffs),
      );
    }
  }
}
