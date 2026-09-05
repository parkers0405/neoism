/** Stable tags emitted by the WASM bridge after classification through the
 * shared Rust Vim Ex parser. None of these actions can close a browser page. */
export type VimExHostPlan =
  | "write"
  | "close"
  | "close_force"
  | "write_close"
  | "close_all"
  | "close_all_force"
  | "write_all_close";

export type VimExHostAction =
  | { kind: "save" }
  | { kind: "close_buffer" }
  | { kind: "save_then_close" }
  | { kind: "close_workspace_buffers" }
  | { kind: "refuse_modified"; all: boolean }
  | { kind: "unavailable" };

export interface VimExHostContext {
  document: boolean;
  modified: boolean;
  workspaceModified: boolean;
}

/** Desktop buffer lifecycle semantics, separated from DOM/transport effects. */
export function resolveVimExHostAction(
  plan: VimExHostPlan,
  context: VimExHostContext,
): VimExHostAction {
  switch (plan) {
    case "write":
      return context.document ? { kind: "save" } : { kind: "unavailable" };
    case "write_close":
      return context.document
        ? { kind: "save_then_close" }
        : { kind: "unavailable" };
    case "close":
      return context.modified
        ? { kind: "refuse_modified", all: false }
        : { kind: "close_buffer" };
    case "close_force":
      return { kind: "close_buffer" };
    case "close_all":
      return context.workspaceModified
        ? { kind: "refuse_modified", all: true }
        : { kind: "close_workspace_buffers" };
    case "close_all_force":
      return { kind: "close_workspace_buffers" };
    case "write_all_close":
      // Desktop's native write-all is still intentionally incomplete. Do not
      // pretend that saving only the focused web document wrote every buffer.
      return { kind: "unavailable" };
  }
}

export interface PendingSaveClose {
  tabKey: string;
  bufferId: string | null;
}

/** Save-and-close ordering gate: close is released only by the matching
 * daemon `Saved` acknowledgement (or the matching host WriteFile callback). */
export class VimExSaveCloseGate {
  private pending: PendingSaveClose | null = null;

  arm(request: PendingSaveClose): void {
    this.pending = request;
  }

  cancel(tabKey?: string): void {
    if (tabKey === undefined || this.pending?.tabKey === tabKey) this.pending = null;
  }

  acknowledge(bufferId: string): string | null {
    if (!this.pending || this.pending.bufferId !== bufferId) return null;
    const tabKey = this.pending.tabKey;
    this.pending = null;
    return tabKey;
  }

  acknowledgeHostWrite(tabKey: string): string | null {
    if (!this.pending || this.pending.tabKey !== tabKey) return null;
    this.pending = null;
    return tabKey;
  }

  peek(): PendingSaveClose | null {
    return this.pending;
  }
}

/** Deterministic BufferTabs fallback after an acknowledged close. */
export function activeIndexAfterClose(length: number, removed: number): number {
  if (length <= 1) return 0;
  return Math.min(Math.max(0, removed), length - 2);
}