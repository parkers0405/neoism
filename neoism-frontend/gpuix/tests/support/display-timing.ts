/** JS-observed timing from native callbacks/viewport publication. These samples
 * do not expose the compositor's presentation timestamps. */
export class IntervalSamples {
  private previous?: number;
  private values: number[] = [];
  sample(now = performance.now()) {
    if (this.previous !== undefined) this.values.push(now - this.previous);
    this.previous = now;
  }
  reset() { this.previous = undefined; this.values = []; }
  summary() {
    const values = [...this.values].sort((a, b) => a - b);
    const percentile = (p: number) => values.length ? values[Math.min(values.length - 1, Math.floor(values.length * p))] : null;
    return { samples: values.length, medianMs: percentile(.5), p95Ms: percentile(.95), p99Ms: percentile(.99), maxMs: values.at(-1) ?? null };
  }
}
interface WakeRenderer { setFrameWakeCallback?: (callback?: (() => void) | null) => boolean }
/** Install before the display clock so every observed wake comes through the
 * actual native API rather than a timer invented by the benchmark. */
export function observeNativeWake(renderer: WakeRenderer) {
  const intervals = new IntervalSamples(), setWake = renderer.setFrameWakeCallback?.bind(renderer);
  if (setWake) renderer.setFrameWakeCallback = callback => setWake(callback ? () => { intervals.sample(); callback(); } : null);
  return intervals;
}
