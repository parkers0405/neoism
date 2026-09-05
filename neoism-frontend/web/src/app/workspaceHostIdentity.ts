import type { WorkspaceSummary } from "../workspace/types";

export interface DaemonHostIdentity {
  hostId: string | null;
  /** Address is intentionally retained only as context for callers/UI. */
  url?: string | null;
}

/**
 * Classify a host from the connected daemon's tree using machine identity,
 * never URL spelling. A missing connected id means an older daemon: entries
 * from that daemon's own tree are conservatively treated as own so existing
 * Notes remain available instead of being replaced by a null linked vault.
 */
export function classifyWorkspaceHost(
  connected: DaemonHostIdentity,
  candidate: DaemonHostIdentity,
): "own" | "foreign" {
  if (connected.hostId === null) return "own";
  return connected.hostId === candidate.hostId ? "own" : "foreign";
}

/** Foreign workspaces may expose only an explicitly linked vault. */
export function notesVaultForWorkspace(
  workspace: WorkspaceSummary | undefined,
  connected: DaemonHostIdentity,
  candidateUrl?: string | null,
): string | null {
  if (!workspace) return null;
  const kind = classifyWorkspaceHost(connected, {
    hostId: workspace.host_id,
    url: candidateUrl,
  });
  return kind === "foreign"
    ? (workspace.linked_vault_dir ?? null)
    : (workspace.notes_vault_dir ?? workspace.linked_vault_dir ?? null);
}