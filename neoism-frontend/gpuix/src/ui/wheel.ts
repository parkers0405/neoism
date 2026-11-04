/** GPUI events and pane geometry use logical pixels. Precise trackpads
 * already supply those pixels; mouse wheels supply notches. */
export interface WheelEvent { deltaX?: number; deltaY?: number; precise?: boolean; scaleFactor?: number; modifiers?: { shift?: boolean } }

export const TOUCHPAD_SCROLL_GAIN = 2;

/** Match readingScrollY for sideways/Shift gestures, before unit conversion.
 * Scale both components together so downstream dominance tests keep routing
 * the same axis. Vertical gestures and mouse-wheel notches are untouched. */
export function horizontalScrollEvent<T extends WheelEvent>(event: T): T {
  const x = event.deltaX || 0, y = event.deltaY || 0;
  if (!event.precise || (!event.modifiers?.shift && Math.abs(x) <= Math.abs(y))) return event;
  return { ...event, deltaX: x * TOUCHPAD_SCROLL_GAIN, deltaY: y * TOUCHPAD_SCROLL_GAIN };
}

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
export function readingScrollY(event: WheelEvent) {
  const y = event.deltaY || 0;
  return event.precise && !event.modifiers?.shift && Math.abs(event.deltaX || 0) <= Math.abs(y) ? y * TOUCHPAD_SCROLL_GAIN : y;
}
