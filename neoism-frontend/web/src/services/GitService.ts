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

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type {
  CommitSummary,
  DiffHunk,
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

  // ── Write-parity verbs (Pass 2) ────────────────────────────────────
  // Each mutation replies with a refreshed `ChangedFiles` (desktop
  // parity: mutate, then re-collect), so callers get the post-op list
  // — with real staged bits — in one round trip.

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

  async unstage(path: string) {
    const reply = await this.client.requestGit({ Unstage: { path } });
    return expectVariant(reply, "ChangedFiles").ChangedFiles;
  }

  async commit(message: string) {
    const reply = await this.client.requestGit({ Commit: { message } });
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

  private async handleEnvelope(envelopeJson: string): Promise<void> {
    let message: GitClientMessage;
    try {
      message = JSON.parse(envelopeJson) as GitClientMessage;
    } catch {
      return;
    }
    try {
      const reply = await this.client.requestGit(message);
      this.applyReply(reply);
    } catch (err) {
      this.call(
        ["gitPanelSetError", "git_panel_set_error"],
        err instanceof Error ? err.message : String(err),
      );
    }
  }

  private applyReply(reply: GitServerMessage): void {
    if ("ChangedFiles" in reply) {
      this.call(
        ["gitPanelApplyChangedFiles", "git_panel_apply_changed_files"],
        JSON.stringify(reply.ChangedFiles),
      );
      void this.refreshDiffs(reply.ChangedFiles.files.map((f) => f.path));
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
  private async refreshDiffs(paths: string[]): Promise<void> {
    if (paths.length === 0) return;
    const seq = ++this.diffFetchSeq;
    let reply: GitServerMessage;
    try {
      reply = await this.client.requestGit({ DiffFiles: { paths } });
    } catch {
      return;
    }
    if (seq !== this.diffFetchSeq) return;
    if ("FileDiffs" in reply) {
      this.call(
        ["gitPanelApplyFileDiffs", "git_panel_apply_file_diffs"],
        JSON.stringify(reply.FileDiffs.diffs),
      );
    }
  }
}
