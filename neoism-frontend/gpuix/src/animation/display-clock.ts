import { usePresentationScheduler, type Schedule } from "./frame-scheduler";
import {batch} from "solid-js";
import {profileStart,profileEnd} from "../profiling";

export interface DisplayRenderer {
  setFrameWakeCallback?: (callback?: (() => void) | null) => boolean;
  requestFrameWake?: () => boolean;
}
export interface DisplayClock { readonly active: boolean; dispose(): void }
const installed = new WeakMap<object, DisplayClock>();

/** Multiplex the native runtime's wake callback with requested animation work.
 * GPUI owns the platform/display frame; JavaScript never invents its cadence.
 * No request remains after animations settle or their owners unmount. */
export function installDisplayClock(renderer: DisplayRenderer, onError: (error: unknown) => void = error => { throw error; }, options:{lookahead?:boolean}={}): DisplayClock {
  const existing = installed.get(renderer); if (existing) return existing;
  const setWake = renderer.setFrameWakeCallback?.bind(renderer);
  if (!setWake || !renderer.requestFrameWake) return { active: false, dispose() {} };
  const callbacks = new Map<number, () => void>(), delayed = new Set<ReturnType<typeof setTimeout>>();
  let next = 0, active = false, queued = false, requested = false, disposed = false;
  let detachSchedule: (() => void) | undefined;
  const arm = () => {
    if (!active || requested || disposed) return;
    requested = true;
    let accepted:boolean;
    try { accepted = renderer.requestFrameWake!(); }
    catch (error) { requested = false; onError(error); return; }
    if (!accepted) {
      requested = false; callbacks.clear();
      onError(new Error("The native display frame request was rejected"));
    }
  };
  const request = () => {
    if (!active || queued || requested || !callbacks.size) return;
    queued = true;
    // Solid flushes mutations and glyph uploads in microtasks. Submit the
    // frame request after those commands, including updates from every owner
    // sharing this display callback, have reached the native UI queue.
    queueMicrotask(() => queueMicrotask(() => {
      queued = false;
      if (!active || requested || !callbacks.size) return;
      arm();
    }));
  };
  const schedule: Schedule = (callback, delay, kind) => {
    const id = ++next;
    let timer: ReturnType<typeof setTimeout> | undefined, cancelled = false;
    const enqueue = () => { if (timer !== undefined) delayed.delete(timer); if (!cancelled && active) { callbacks.set(id, callback); request(); } };
    // Real deadlines (e.g. a hover probe) may sleep; the animation interval
    // itself is replaced by the next native display frame, at any refresh rate.
    if (kind === "deadline" && delay > 0) { timer = setTimeout(enqueue, delay); delayed.add(timer); }
    else enqueue();
    return () => { cancelled = true; callbacks.delete(id); if (timer !== undefined) { clearTimeout(timer); delayed.delete(timer); } };
  };
  const clear = () => {
    active = false; queued = false; requested = false;
    callbacks.clear(); for (const timer of delayed) clearTimeout(timer); delayed.clear();
    detachSchedule?.(); detachSchedule = undefined;
  };
  renderer.setFrameWakeCallback = callback => {
    if (disposed) return false;
    clear();
    if (!callback) return setWake(null);
    const accepted = setWake(() => {
      if (!active || disposed) return;
      requested = false;
      const batch = [...callbacks.values()]; callbacks.clear();
      // Reserve the following platform opportunity before preparing this frame.
      // Waiting for Solid/native mutation flushes can register too late while
      // the UI thread is already drawing. This wake does not invalidate paint.
      // At the end of an animation one empty callback drains the reservation;
      // it neither advances owners nor arms another wake, so idle still parks.
      if(options.lookahead!==false&&batch.length)arm();
      const started=profileStart();
      // All owners share a presentation. Flush dependent Solid computations
      // once, after every owner's state has advanced to that frame.
      runFrameBatch(batch,onError);
      profileEnd("animation.batch",started);
      callback();
    });
    if (accepted) { active = true; detachSchedule = usePresentationScheduler(schedule); }
    return accepted;
  };
  const clock: DisplayClock = { get active() { return active; }, dispose() {
    if (disposed) return; disposed = true; clear(); setWake(null);
  } };
  installed.set(renderer, clock); return clock;
}

export function runFrameBatch(callbacks:readonly (()=>void)[],onError:(error:unknown)=>void){
  batch(()=>{for(const frame of callbacks){try{frame();}catch(error){onError(error);}}});
}
