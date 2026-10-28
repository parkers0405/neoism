export const ANIMATION_FRAME_MS = 1000 / 240;

/** Schedule only requested animation work; finishing an animation leaves no
 * repeating timer behind. Native presentation can coalesce these updates. */
export function animationDelay(startedAt: number, intervalMs: number, now = performance.now()) {
  return Math.max(0, intervalMs - Math.max(0, now - startedAt));
}

export type Schedule = (callback: () => void, delay: number, kind?: "frame" | "deadline") => () => void;
const scheduleTimeout: Schedule = (callback, delay) => {
  const timer = setTimeout(callback, delay);
  return () => clearTimeout(timer);
};
let presentationSchedule: Schedule | undefined;
export function usePresentationScheduler(schedule: Schedule) {
  const previous = presentationSchedule; presentationSchedule = schedule;
  return () => { if (presentationSchedule === schedule) presentationSchedule = previous; };
}
const scheduleFrame: Schedule = (callback, delay, kind) => (presentationSchedule || scheduleTimeout)(callback, delay, kind);

export class AnimationFrameScheduler {
  private cancelTimer?: () => void;
  private disposed = false;
  constructor(readonly intervalMs: number, private readonly now = () => performance.now(), private readonly schedule: Schedule = scheduleFrame) {}
  get pending() { return this.cancelTimer !== undefined; }
  request(callback: () => void, startedAt = this.now(), intervalMs = this.intervalMs) {
    if (this.disposed || this.pending) return;
    this.cancelTimer = this.schedule(() => {
      this.cancelTimer = undefined;
      if (!this.disposed) callback();
    }, animationDelay(startedAt, intervalMs, this.now()), intervalMs > this.intervalMs ? "deadline" : "frame");
  }
  cancel() { this.cancelTimer?.(); this.cancelTimer = undefined; }
  dispose() { this.disposed = true; this.cancel(); }
}
