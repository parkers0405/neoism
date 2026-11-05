import { AnimationFrameScheduler, ANIMATION_FRAME_MS, type Schedule } from "../animation/frame-scheduler";
import { easeOutCubic } from "../design/animation";

// Sugarloaf shared/panels/file_tree/{mod,render}.rs. This is a root-swap
// sweep, NOT an entry-insertion animation or a fade of the whole sidebar.
export const TREE_ROOT_REVEAL_MS = 320;
export const TREE_ROOT_STAGGER_MS = 12;
export interface TreeRevealIdentity { key?: string }
export interface TreeRevealInput {
  /** Workspace id + tree kind + declared root; never the terminal cwd. */
  key: string;
  ready: boolean;
  total: number;
  height: number;
  rowHeight: number;
  top: number;
  offset: number;
  reducedMotion: boolean;
  /** Mounted panel/workspace visibility, not keyboard focus. OS occlusion is
   * handled by the native presentation scheduler (no JS observation API). */
  active: boolean;
}

/** Pure paint state: never touches native selection, expansion, scroll or focus.
 * Identity can outlive a component generation; scheduler ownership cannot. */
export class TreeRootReveal {
  private readonly frames: AnimationFrameScheduler;
  private input?: TreeRevealInput;
  private key?: string;
  private armed = false;
  private started?: number;
  private elapsed = 0;
  private disposed = false;
  constructor(private readonly identity: TreeRevealIdentity, private readonly changed: () => void,
    private readonly now = () => performance.now(), schedule?: Schedule) {
    this.frames = new AnimationFrameScheduler(ANIMATION_FRAME_MS, now, schedule);
  }
  get pending() { return this.frames.pending; }
  update(input: TreeRevealInput) {
    if (this.disposed) return;
    this.input = input;
    if (this.key !== input.key) {
      this.key = input.key;
      this.frames.cancel(); this.started = undefined; this.elapsed = 0; this.armed = this.identity.key !== input.key;
      this.changed();
    }
    // Do not consume a cached activation while its incoming generation is
    // hidden. If superseded before promotion, the next generation must still
    // be able to reveal this activation. Config-only remounts retain identity.
    if (input.ready && input.active) this.identity.key = input.key;
    if (!input.active || input.reducedMotion) {
      this.finish(); return;
    }
    if (this.armed && input.ready && (input.total === 0 || input.height > 0 && input.rowHeight > 0)) {
      this.armed = false;
      if (input.total > 0 && input.height > 0 && input.rowHeight > 0) this.started = this.now();
    }
    this.advance();
  }
  private finish() {
    const repaint = this.started !== undefined;
    this.frames.cancel(); this.started = undefined; this.armed = false;
    if (repaint) this.changed();
  }
  private advance = () => {
    if (this.disposed || this.started === undefined || !this.input) return;
    const input = this.input;
    this.elapsed = Math.max(0, this.now() - this.started);
    const visible = input.rowHeight > 0 ? Math.ceil(input.height / input.rowHeight) : 0;
    if (!input.active || input.reducedMotion || visible <= 0 || this.elapsed >= TREE_ROOT_REVEAL_MS + visible * TREE_ROOT_STAGGER_MS) {
      this.finish(); return;
    }
    this.changed();
    this.frames.request(this.advance);
  };
  row(index: number, scale = 1) {
    const input = this.input;
    if (this.started === undefined || !input) return { opacity: 1, offsetX: 0 };
    // Rank only intersecting rows: overscan must not consume stagger beats.
    const first = Math.max(0, Math.floor(input.top - input.offset / input.rowHeight));
    const rank = Math.max(0, index - first);
    const t = Math.max(0, Math.min(1, (this.elapsed - rank * TREE_ROOT_STAGGER_MS) / TREE_ROOT_REVEAL_MS));
    const opacity = easeOutCubic(t);
    // The old code says "from the left" but actually adds a POSITIVE offset.
    return { opacity, offsetX: (1 - opacity) * 12 * scale };
  }
  dispose() { this.disposed = true; this.frames.dispose(); this.started = undefined; this.armed = false; }
}