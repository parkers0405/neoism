// Exact cubic used by shared/src/animation.rs. x(t) = t for this Bezier.
export const easeOutCubicBezier: [number, number, number, number] = [1 / 3, 1, 2 / 3, 1];
export const easeOutCubic = (t: number) => 1 + (Math.max(0, Math.min(1, t)) - 1) ** 3;
export function revealProgress(elapsedMs: number, row: number, durationMs: number, staggerMs: number) {
  return easeOutCubic((elapsedMs - row * staggerMs) / durationMs);
}
