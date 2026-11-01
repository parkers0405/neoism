/** GPUI events and pane geometry use logical pixels. Precise trackpads
 * already supply those pixels; mouse wheels supply notches. */
export interface WheelEvent { deltaX?: number; deltaY?: number; precise?: boolean; scaleFactor?: number }

/** One terminal cell per notch, in the same units as the pane. Applying
 * display scale here again makes wheels jump farther on HiDPI monitors. */
export function wheelCellPixels(_event: WheelEvent, fontSize: number) { return Math.round(fontSize * 1.3); }

export function wheelPixels(event: WheelEvent, fontSize: number) {
  const cell = event.precise ? 1 : wheelCellPixels(event, fontSize);
  return { x: (event.deltaX || 0) * cell, y: (event.deltaY || 0) * cell };
}

/** More travel per trackpad gesture in long conversations and diffs. Keep
 * fractional deltas and event cadence so native inertia/animation is preserved.
 * Sideways gestures and Shift-scroll retain their original axis and distance. */
export function readingScrollY(event: WheelEvent & { modifiers?: { shift?: boolean } }) {
  const y = event.deltaY || 0;
  return event.precise && !event.modifiers?.shift && Math.abs(event.deltaX || 0) <= Math.abs(y) ? y * 2 : y;
}
