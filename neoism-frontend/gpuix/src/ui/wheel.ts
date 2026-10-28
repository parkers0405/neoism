/** Wheel events carry pixels for precise trackpads and notches for mouse
 * wheels. Desktop (`Screen::scroll`) sizes a code/markdown notch as one
 * terminal cell in physical pixels, so it travels the same distance here. */
export interface WheelEvent { deltaX?: number; deltaY?: number; precise?: boolean; scaleFactor?: number }

/** Physical-pixel cell height the desktop multiplies wheel notches by. */
export function wheelCellPixels(event: WheelEvent, fontSize: number) { return Math.round(fontSize * 1.3) * (event.scaleFactor || 1); }

export function wheelPixels(event: WheelEvent, fontSize: number) {
  const cell = event.precise ? 1 : wheelCellPixels(event, fontSize);
  return { x: (event.deltaX || 0) * cell, y: (event.deltaY || 0) * cell };
}
