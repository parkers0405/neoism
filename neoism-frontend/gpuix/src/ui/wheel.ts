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
