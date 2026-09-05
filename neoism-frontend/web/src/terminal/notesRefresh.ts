export interface NotesSnapshotEntry {
  path: string;
  is_dir: boolean;
  icon?: string;
}

export interface NotesDirectoryEntry {
  name: string;
  is_dir: boolean;
  icon?: string;
}

export interface NotesRefreshAttempt<Adapter extends object> {
  readonly generation: number;
  readonly adapter: Adapter;
  readonly vault: string | null;
  readonly workspace?: string | null;
  readonly connectionGeneration?: number;
}

export interface NotesSyncIdentity<Adapter extends object> {
  readonly adapter: Adapter;
  readonly vault: string | null;
  readonly workspace: string | null;
  readonly connectionGeneration: number;
}

export function normalizeNotesVaultRoot(vault: string | null | undefined): string | null {
  if (!vault) return null;
  const normalized = vault.length > 1 ? vault.replace(/\/+$/, "") : vault;
  return normalized || "/";
}

export function notesChangedTouchesActiveVault(
  changedRoot: string | null | undefined,
  activeVault: string | null | undefined,
): boolean {
  const root = normalizeNotesVaultRoot(changedRoot);
  return root !== null && root === normalizeNotesVaultRoot(activeVault);
}

export interface NotesVaultSink {
  setNotesVaultRoot(vault: string | null): void;
}

/** Cache the resolution and always replay it to a currently mounted panel. */
export function synchronizeNotesVault(
  vault: string | null | undefined,
  panel: NotesVaultSink | null,
): string | null {
  const normalized = normalizeNotesVaultRoot(vault);
  panel?.setNotesVaultRoot(normalized);
  return normalized;
}

/** Newest-wins token, also bound to the exact adapter and normalized vault. */
export class NotesRefreshCoordinator<Adapter extends object> {
  private generation = 0;
  private desired: NotesSyncIdentity<Adapter> | null = null;
  private dirty = false;
  private inFlight = false;
  private failures = 0;

  begin(adapter: Adapter, vault: string | null | undefined): NotesRefreshAttempt<Adapter> {
    return {
      generation: ++this.generation,
      adapter,
      vault: normalizeNotesVaultRoot(vault),
    };
  }

  invalidate(): void {
    this.generation += 1;
    this.dirty = true;
    this.inFlight = false;
  }

  isCurrent(
    attempt: NotesRefreshAttempt<Adapter>,
    adapter: Adapter | null,
    vault: string | null | undefined,
  ): boolean {
    return attempt.generation === this.generation
      && attempt.adapter === adapter
      && attempt.vault === normalizeNotesVaultRoot(vault);
  }

  /** Install/replay desired host state. Dirtiness survives failed requests;
   * identity includes the socket generation so reconnects cannot reuse an old
   * success. `force` supports same-root replay and explicit panel opens. */
  ensure(
    adapter: Adapter,
    vault: string | null | undefined,
    workspace: string | null | undefined,
    connectionGeneration: number,
    force = false,
  ): void {
    const next: NotesSyncIdentity<Adapter> = {
      adapter,
      vault: normalizeNotesVaultRoot(vault),
      workspace: workspace ?? null,
      connectionGeneration,
    };
    const changed = !this.desired
      || this.desired.adapter !== next.adapter
      || this.desired.vault !== next.vault
      || this.desired.workspace !== next.workspace
      || this.desired.connectionGeneration !== next.connectionGeneration;
    if (changed || force) {
      this.desired = next;
      this.dirty = true;
      this.generation += 1;
      this.failures = 0;
    }
  }

  beginDesired(): NotesRefreshAttempt<Adapter> | null {
    if (!this.desired || !this.dirty || this.inFlight) return null;
    this.inFlight = true;
    return { generation: this.generation, ...this.desired };
  }

  finish(attempt: NotesRefreshAttempt<Adapter>, success: boolean): boolean {
    this.inFlight = false;
    const current = !!this.desired
      && attempt.generation === this.generation
      && attempt.adapter === this.desired.adapter
      && attempt.vault === this.desired.vault
      && attempt.workspace === this.desired.workspace
      && attempt.connectionGeneration === this.desired.connectionGeneration;
    if (!current) return false;
    if (success) {
      this.dirty = false;
      this.failures = 0;
    } else {
      this.dirty = true;
      this.failures = Math.min(this.failures + 1, 8);
    }
    return true;
  }

  needsRefresh(): boolean {
    return this.dirty && !this.inFlight;
  }

  retryDelayMs(): number {
    return Math.min(5_000, 250 * 2 ** Math.max(0, this.failures - 1));
  }
}

/** Build one all-or-nothing recursive snapshot; child failures propagate. */
export async function collectNotesSnapshot(
  vault: string,
  listDirectory: (relativeDir: string) => Promise<readonly NotesDirectoryEntry[]>,
  maxDepth = 6,
  maxEntries = 800,
): Promise<NotesSnapshotEntry[]> {
  const base = normalizeNotesVaultRoot(vault);
  if (!base) return [];
  const snapshot: NotesSnapshotEntry[] = [];

  const walk = async (dir: string, depth: number): Promise<void> => {
    if (depth > maxDepth || snapshot.length >= maxEntries) return;
    const listing = await listDirectory(dir);
    for (const entry of listing) {
      if (snapshot.length >= maxEntries) return;
      if (entry.name.startsWith(".")) continue;
      const rel = dir ? `${dir}/${entry.name}` : entry.name;
      snapshot.push({
        path: base === "/" ? `/${rel}` : `${base}/${rel}`,
        is_dir: entry.is_dir,
        icon: entry.icon,
      });
      if (entry.is_dir) await walk(rel, depth + 1);
    }
  };

  await walk("", 0);
  return snapshot;
}

/** Retry from a new accumulator; never reuse a failed walk's partial prefix. */
export async function collectNotesSnapshotWithRetry(
  vault: string,
  listDirectory: (relativeDir: string) => Promise<readonly NotesDirectoryEntry[]>,
  beforeRetry: () => Promise<void>,
): Promise<NotesSnapshotEntry[]> {
  try {
    return await collectNotesSnapshot(vault, listDirectory);
  } catch {
    await beforeRetry();
    return collectNotesSnapshot(vault, listDirectory);
  }
}