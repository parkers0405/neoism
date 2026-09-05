/**
 * Host-side axis lock for the two chrome surfaces which bypass the shared
 * TouchGesturePolicy (the horizontal tab strip and the vertical file tree).
 *
 * The important promotion rule is that `delta` is measured from touch-down on
 * the first committed frame.  Measuring from the previous ambiguous sample
 * throws away the intent threshold and produces the familiar mobile dead-zone
 * jump.  Once committed, deltas are incremental and therefore 1:1.
 */
export const TOUCH_SCROLL_INTENT_PX = 4;

export type TouchScrollAxis = "x" | "y";

export type TouchMomentumAxis = TouchScrollAxis | "dominant";

export interface TouchMomentumFrame {
  dx: number;
  dy: number;
  active: boolean;
}

interface VelocitySample {
  x: number;
  y: number;
  timeMs: number;
}

/**
 * Shared iOS-style release velocity + deceleration policy for every canvas
 * scroll surface. Drag pixels are never passed through this class: callers
 * apply those 1:1, record the finger position here, and only consume `step`
 * frames after lift.
 */
export class TouchMomentum {
  private samples: VelocitySample[] = [];
  private velocityX = 0;
  private velocityY = 0;
  private lastFrameMs = 0;
  private running = false;

  // A short window tracks the flick at the end of a gesture rather than the
  // average speed of a long drag. Recent segments carry more weight.
  private static readonly SAMPLE_WINDOW_MS = 110;
  // Safari often delivers the final motion sample well before touchend. A
  // 180ms grace still rejects a hold, while retaining a real sparse flick.
  private static readonly RELEASE_STALE_MS = 180;
  private static readonly MIN_LAUNCH_PX_S = 110;
  private static readonly STOP_PX_S = 72;
  private static readonly MAX_PX_S = 6000;
  private static readonly DECAY_TAU_S = 0.32;

  begin(x: number, y: number, timeMs: number): void {
    this.cancel();
    this.samples = [{ x, y, timeMs }];
  }

  sample(x: number, y: number, timeMs: number): void {
    const last = this.samples.at(-1);
    if (last && timeMs < last.timeMs) return;
    // A stationary touchend is not a new velocity segment. Recording it with
    // a fresh timestamp gives its zero-speed segment maximum recency weight
    // and "snubs" otherwise fast releases on WebKit.
    if (last && x === last.x && y === last.y) return;
    if (last && timeMs === last.timeMs) {
      this.samples[this.samples.length - 1] = { x, y, timeMs };
    } else {
      this.samples.push({ x, y, timeMs });
    }
    const cutoff = timeMs - TouchMomentum.SAMPLE_WINDOW_MS;
    while (this.samples.length > 2 && this.samples[1].timeMs < cutoff) {
      this.samples.shift();
    }
  }

  /** Record an event's historical/coalesced points in timestamp order. */
  sampleBatch(samples: ReadonlyArray<VelocitySample>): void {
    for (const sample of [...samples].sort((a, b) => a.timeMs - b.timeMs)) {
      this.sample(sample.x, sample.y, sample.timeMs);
    }
  }

  /** Launch from recent finger velocity. Returns false for a hold/slow lift. */
  release(timeMs: number, axis: TouchMomentumAxis = "dominant"): boolean {
    const last = this.samples.at(-1);
    if (!last || timeMs - last.timeMs > TouchMomentum.RELEASE_STALE_MS) {
      this.samples = [];
      return false;
    }
    const cutoff = timeMs - TouchMomentum.SAMPLE_WINDOW_MS;
    const segments: Array<{ vx: number; vy: number; weight: number }> = [];
    for (let i = 1; i < this.samples.length; i += 1) {
      const a = this.samples[i - 1];
      const b = this.samples[i];
      const dtMs = b.timeMs - a.timeMs;
      if (dtMs <= 0 || b.timeMs < cutoff) continue;
      // Quadratic recency weighting catches acceleration without letting a
      // single tiny final interval dominate noisy touch hardware.
      const recency = 1 - Math.min(1, (timeMs - b.timeMs) / TouchMomentum.SAMPLE_WINDOW_MS);
      const weight = dtMs * (0.2 + recency * recency * 0.8);
      segments.push({
        vx: ((b.x - a.x) * 1000) / dtMs,
        vy: ((b.y - a.y) * 1000) / dtMs,
        weight,
      });
    }
    const weight = segments.reduce((sum, segment) => sum + segment.weight, 0);
    if (weight <= 0) return false;
    let vx = segments.reduce((sum, segment) => sum + segment.vx * segment.weight, 0) / weight;
    let vy = segments.reduce((sum, segment) => sum + segment.vy * segment.weight, 0) / weight;
    if (axis === "x" || (axis === "dominant" && Math.abs(vx) >= Math.abs(vy))) vy = 0;
    if (axis === "y" || (axis === "dominant" && Math.abs(vy) > Math.abs(vx))) vx = 0;
    const speed = Math.hypot(vx, vy);
    if (speed < TouchMomentum.MIN_LAUNCH_PX_S) return false;
    const cap = Math.min(1, TouchMomentum.MAX_PX_S / speed);
    this.velocityX = vx * cap;
    this.velocityY = vy * cap;
    this.lastFrameMs = timeMs;
    this.running = true;
    this.samples = [];
    return true;
  }

  step(timeMs: number): TouchMomentumFrame {
    if (!this.running) return { dx: 0, dy: 0, active: false };
    const dt = Math.max(0, Math.min(0.05, (timeMs - this.lastFrameMs) / 1000));
    this.lastFrameMs = timeMs;
    if (dt <= 0) return { dx: 0, dy: 0, active: true };
    const decay = Math.exp(-dt / TouchMomentum.DECAY_TAU_S);
    // Analytic integration of exponential velocity keeps travel independent
    // of display refresh rate and avoids a stepped low-speed tail.
    const travelScale = TouchMomentum.DECAY_TAU_S * (1 - decay);
    const dx = this.velocityX * travelScale;
    const dy = this.velocityY * travelScale;
    this.velocityX *= decay;
    this.velocityY *= decay;
    if (Math.hypot(this.velocityX, this.velocityY) < TouchMomentum.STOP_PX_S) {
      this.cancel();
      return { dx, dy, active: false };
    }
    return { dx, dy, active: true };
  }

  /** Stop at a new touch or a hard content bound; true suppresses its click. */
  cancel(): boolean {
    const wasRunning = this.running;
    this.running = false;
    this.velocityX = 0;
    this.velocityY = 0;
    this.lastFrameMs = 0;
    return wasRunning;
  }

  isRunning(): boolean {
    return this.running;
  }
}

/** Normalize DOMHighResTimeStamp across Safari's monotonic and epoch forms. */
export function normalizedTouchEventTime(timeStamp: number, nowMs: number): number {
  if (!Number.isFinite(timeStamp) || timeStamp <= 0) return nowMs;
  if (Math.abs(timeStamp - nowMs) < 86_400_000) return timeStamp;
  const origin = typeof performance !== "undefined" && Number.isFinite(performance.timeOrigin)
    ? performance.timeOrigin
    : Date.now() - nowMs;
  const monotonic = timeStamp - origin;
  return Number.isFinite(monotonic) && monotonic >= 0 ? monotonic : nowMs;
}

export interface DirectTouchScrollUpdate {
  /** The gesture moved far enough that a later touchend must not click. */
  moved: boolean;
  /** The requested axis won and this surface now owns the gesture. */
  scrolling: boolean;
  /** Exact finger delta to apply on this frame. */
  delta: number;
}

export class DirectTouchScrollGesture {
  private lastPrimary: number;
  private state: "pending" | "scrolling" | "rejected" = "pending";

  constructor(
    private readonly axis: TouchScrollAxis,
    private readonly startX: number,
    private readonly startY: number,
    private readonly threshold = TOUCH_SCROLL_INTENT_PX,
  ) {
    this.lastPrimary = axis === "x" ? startX : startY;
  }

  update(x: number, y: number): DirectTouchScrollUpdate {
    const primary = this.axis === "x" ? x : y;
    const totalPrimary = primary - (this.axis === "x" ? this.startX : this.startY);
    const totalCross = (this.axis === "x" ? y - this.startY : x - this.startX);

    if (this.state === "pending") {
      if (Math.max(Math.abs(totalPrimary), Math.abs(totalCross)) <= this.threshold) {
        return { moved: false, scrolling: false, delta: 0 };
      }
      if (Math.abs(totalPrimary) < Math.abs(totalCross)) {
        this.state = "rejected";
        return { moved: true, scrolling: false, delta: 0 };
      }
      this.state = "scrolling";
      this.lastPrimary = primary;
      // Re-feed all displacement accumulated while intent was ambiguous.
      return { moved: true, scrolling: true, delta: totalPrimary };
    }

    if (this.state === "rejected") {
      return { moved: true, scrolling: false, delta: 0 };
    }

    const delta = primary - this.lastPrimary;
    this.lastPrimary = primary;
    return { moved: true, scrolling: true, delta };
  }

  didMove(): boolean {
    return this.state !== "pending";
  }

  isScrolling(): boolean {
    return this.state === "scrolling";
  }
}