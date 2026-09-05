export interface EditorTouchTapAdapter {
  editorPointerDown?: (
    x: number,
    y: number,
    shift: boolean,
    ctrl: boolean,
    clickCount: number,
  ) => boolean;
  editorPointerUp?: () => boolean;
}

/**
 * Deliver a resolved one-finger tap to the native editor pointer pair.
 * Gesture classification happens before this function is called, so drags
 * never enter this path. The release is deliberately immediate: touch has no
 * hover/button lifetime after the policy emits its end-tap action.
 */
export function routeEditorTouchTap(
  adapter: EditorTouchTapAdapter | null,
  editorActive: boolean,
  x: number,
  y: number,
  onHandled: () => void,
): boolean {
  if (!editorActive || !adapter?.editorPointerDown) return false;
  const handled = adapter.editorPointerDown(x, y, false, false, 1) === true;
  adapter.editorPointerUp?.();
  if (handled) onHandled();
  return handled;
}