/** Lifetime guard for gestures which start in a painted tab strip.
 *
 * Activation can replace the content surface between down and up. Routing
 * later phases against that new surface would deliver half a gesture to
 * content which was not under the original press.
 */
export type TabGestureSource = "workspace-tabs" | "pane-tabs";

export class TabGestureOwnership {
  private readonly owners = new Map<number, TabGestureSource>();

  claim(id: number, source: TabGestureSource): void {
    this.owners.set(id, source);
  }

  source(id: number): TabGestureSource | null {
    return this.owners.get(id) ?? null;
  }

  owns(id: number): boolean {
    return this.owners.has(id);
  }

  release(id: number): TabGestureSource | null {
    const source = this.source(id);
    this.owners.delete(id);
    return source;
  }

  clear(): void {
    this.owners.clear();
  }
}