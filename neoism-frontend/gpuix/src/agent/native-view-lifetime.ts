/** Cache eviction is not view unmount: close notifications can run inside a
 * Solid transaction. Stop work now, but keep native paint resources alive until
 * the last displaying view has cleaned up. Never look up a replacement by ID. */
export class NativeViewLifetime {
  closed = false;
  disposed = false;
  private views = new Set<() => void>();
  constructor(private stop: () => void, private destroy: () => void) {}
  attach(cancel: () => void): () => void {
    this.views.add(cancel);
    if (this.closed) cancel();
    return () => { this.views.delete(cancel); this.finish(); };
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.stop();
    for (const cancel of this.views) cancel();
    this.finish();
  }
  private finish() {
    if (!this.closed || this.disposed || this.views.size) return;
    this.disposed = true;
    this.destroy();
  }
}