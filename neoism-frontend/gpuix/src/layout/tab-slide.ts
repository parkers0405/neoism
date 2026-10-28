import { AnimationFrameScheduler, ANIMATION_FRAME_MS, type Schedule } from "../animation/frame-scheduler";

interface Spring { position: number; velocity: number }
/** Exact update from shared/src/animation.rs::CriticallyDampedSpring. */
function advance(spring: Spring, dt: number, length: number) {
  if (length <= dt) { spring.position = spring.velocity = 0; return false; }
  if (!spring.position) return false;
  const omega = 4 / length, a = spring.position, b = a * omega + spring.velocity, c = Math.exp(-omega * dt);
  spring.position = (a + b * dt) * c;
  spring.velocity = c * (-a * omega - b * dt * omega + b);
  if (Math.abs(spring.position) < .01) { spring.position = spring.velocity = 0; return false; }
  return true;
}
export interface TabSlideOptions {
  reducedMotion: boolean; onChange: () => void; animationLength?: number;
  now?: () => number; schedule?: Schedule;
}
/** Logical slots never depend on the animated paint offset. On reorder, old
 * positions become spring deltas in the new slots (FLIP). Native relative
 * `left` offsets paint stable existing tab hosts without changing their flow. */
export class TabSlide<K extends string | number> {
  private slots = new Map<K, number>();
  private springs = new Map<K, Spring>();
  private drag?: { id: K; pointer: number; grab: number };
  private frame: AnimationFrameScheduler;
  private clock: () => number;
  private last = 0;
  private disposed = false;
  constructor(private options: TabSlideOptions) {
    this.clock = options.now || (() => performance.now());
    this.frame = new AnimationFrameScheduler(ANIMATION_FRAME_MS, this.clock, options.schedule);
  }
  logical(id: K) { return this.slots.get(id) || 0; }
  offset(id: K) {
    return this.drag?.id === id ? this.drag.pointer - this.drag.grab - this.logical(id) : this.springs.get(id)?.position || 0;
  }
  position(id: K) { return this.logical(id) + this.offset(id); }
  layout(entries: readonly { id: K; width: number }[]) {
    if (this.disposed) return;
    // Host text and tab widths arrive after mount. Measuring those widths (or
    // a renamed tab) is layout hydration, not a reorder entrance animation.
    // Compare only surviving IDs so opening/closing a tab does not replay one.
    const nextIds = new Set(entries.map(entry => entry.id));
    const previousOrder = [...this.slots.keys()].filter(id => nextIds.has(id));
    const nextOrder = entries.filter(entry => this.slots.has(entry.id)).map(entry => entry.id);
    const reordered = previousOrder.some((id, index) => id !== nextOrder[index]);
    let x = 0, changed = false;
    const next = new Map<K, number>();
    for (const entry of entries) {
      next.set(entry.id, x);
      const before = this.slots.get(entry.id), spring = this.springs.get(entry.id) || { position: 0, velocity: 0 };
      if (before !== undefined && before !== x) {
        // Preserve an existing slide's visible position across a measurement
        // correction, but never start a new slide for that correction alone.
        if (reordered || spring.position !== 0) spring.position += before - x;
        changed = true;
        if (this.options.reducedMotion) spring.position = spring.velocity = 0;
      }
      this.springs.set(entry.id, spring); x += Math.max(0, entry.width);
    }
    for (const id of this.springs.keys()) if (!next.has(id)) this.springs.delete(id);
    this.slots = next;
    if (changed) { this.options.onChange(); this.schedule(); }
  }
  begin(id: K, pointer: number, grabOffset = pointer - this.position(id)) {
    if (this.disposed || !this.slots.has(id)) return;
    this.drag = { id, pointer, grab: grabOffset }; this.options.onChange();
  }
  move(pointer: number) { if (this.drag) { this.drag.pointer = pointer; this.options.onChange(); } }
  end() {
    const drag = this.drag; if (!drag) return;
    const spring = this.springs.get(drag.id);
    if (spring) { spring.position = this.options.reducedMotion ? 0 : this.offset(drag.id); spring.velocity = 0; }
    this.drag = undefined; this.options.onChange(); this.schedule();
  }
  private schedule() {
    if (this.disposed || this.options.reducedMotion || ![...this.springs].some(([id, spring]) => id !== this.drag?.id && spring.position !== 0)) return;
    if (!this.frame.pending) this.last = this.clock();
    this.frame.request(() => this.tick(), this.clock());
  }
  private tick() {
    const now = this.clock(), dt = Math.min(.05, Math.max(0, (now - this.last) / 1000)); this.last = now;
    let moving = false;
    for (const [id, spring] of this.springs) if (id !== this.drag?.id) moving = advance(spring, dt, this.options.animationLength ?? .12) || moving;
    this.options.onChange();
    if (moving) this.frame.request(() => this.tick(), now);
  }
  dispose() { this.disposed = true; this.frame.dispose(); this.drag = undefined; this.springs.clear(); this.slots.clear(); }
}
