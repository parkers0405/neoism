interface FocusRenderer {
  getFocusedElementId?: () => number | null;
  getFocusedElementIdSnapshot?: () => number | null;
  focusElement?: (id: number) => void;
}
interface Snapshot { owner: number | null | undefined; nextRead: number }
const snapshots = new WeakMap<object, Snapshot>();

/** Focus queries synchronously wait for the native UI thread. Share the
 * fallback observation across mounted views; actual focus/blur events and
 * explicit focus requests invalidate it immediately. */
export function focusedNativeElement(renderer: FocusRenderer, clock = () => performance.now()) {
  let state = snapshots.get(renderer);
  if (!state) { state = { owner: undefined, nextRead: -Infinity }; snapshots.set(renderer, state); }
  if (clock() >= state.nextRead) {
    // Right after a focus event, ask the UI thread (it may not have drawn yet).
    // Steady-state polls read the lock-free snapshot from the latest draw: the
    // synchronous query parks JavaScript behind an in-progress paint.
    state.owner = state.nextRead === -Infinity || !renderer.getFocusedElementIdSnapshot
      ? renderer.getFocusedElementId?.() : renderer.getFocusedElementIdSnapshot();
    state.nextRead = clock() + 50;
  }
  return state.owner;
}

export function invalidateNativeFocus(renderer: object) {
  const state = snapshots.get(renderer);
  if (state) state.nextRead = -Infinity;
}

export function focusNativeElement(renderer: FocusRenderer, id: number) {
  invalidateNativeFocus(renderer);
  renderer.focusElement?.(id);
}
