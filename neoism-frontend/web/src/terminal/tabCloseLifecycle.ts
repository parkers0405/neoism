export type CloseAuthority = "workspace" | "surface" | "session" | "pty";

interface Tombstone {
  generation: number;
  waitingFor: Set<CloseAuthority>;
}

/**
 * Orders tab close against daemon inventory replies.
 *
 * A close is local-first, but its identity remains tombstoned until every
 * authority which can advertise it has acknowledged removal. Reconnects keep
 * outstanding tombstones; an explicit open is the only operation allowed to
 * supersede one before the acknowledgements arrive.
 */
export class TabCloseLifecycle {
  private generation = 0;
  private readonly tombstones = new Map<string, Tombstone>();

  setGeneration(generation: number): void {
    this.generation = generation;
  }

  beginClose(key: string, authorities: Iterable<CloseAuthority>): boolean {
    if (this.tombstones.has(key)) return false;
    this.tombstones.set(key, {
      generation: this.generation,
      waitingFor: new Set(authorities),
    });
    return true;
  }

  explicitOpen(key: string): void {
    this.tombstones.delete(key);
  }

  blocks(key: string): boolean {
    return this.tombstones.has(key);
  }

  acknowledge(key: string, authority: CloseAuthority): void {
    const tombstone = this.tombstones.get(key);
    if (!tombstone) return;
    tombstone.waitingFor.delete(authority);
    if (tombstone.waitingFor.size === 0) this.tombstones.delete(key);
  }

  acknowledgeMissing(authority: CloseAuthority, presentKeys: ReadonlySet<string>): void {
    for (const [key, tombstone] of this.tombstones) {
      if (!presentKeys.has(key)) this.acknowledge(key, authority);
      // A present key from any generation is deliberately ignored. It can be
      // a reply queued before close or a reconnect snapshot produced before
      // the idempotent close was resent.
      void tombstone;
    }
  }

  pendingKeys(): string[] {
    return [...this.tombstones.keys()];
  }
}

export function confirmDirtyClose(modified: boolean, confirmDiscard: () => boolean): boolean {
  return !modified || confirmDiscard();
}

export function activeIndexAfterClose(length: number, active: number, removed: number): number {
  const nextLength = Math.max(0, length - 1);
  if (nextLength === 0) return 0;
  if (removed === active) return Math.min(removed, nextLength - 1);
  return removed < active ? Math.max(0, active - 1) : Math.min(active, nextLength - 1);
}