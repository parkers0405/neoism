/**
 * Touch gesture decisions, routed through the SHARED RUST policy
 * (`neoism-ui::touch_policy`) via the wasm `TouchGesturePolicy` class
 * exported from `wasm/src/rendered/input_policy.rs`.
 *
 * This file is a thin adapter, not a mirror: the tap / drag-select /
 * scroll / pinch / two-finger-pan / long-press state machine runs in
 * Rust (the exact code the desktop fork runs), and this class only
 * translates DOM touch samples in and `{ kind: ... }` actions out.
 * There is deliberately NO TypeScript fallback state machine — before
 * the wasm bundle loads nothing is rendered so touches are meaningless,
 * and a served bundle predating the exports gets a one-shot console
 * warning telling the developer to rebuild (`npm run build:wasm`).
 *
 * See `neoism-frontend/shared/src/touch_policy.rs` for the source of
 * truth and the unit tests that pin the behaviour.
 */

import {
  wasmInputPolicy,
  type WasmInputPolicyModule,
  type WasmTouchGesturePolicyInstance,
} from "../terminal/createTerminal";

/** Pixel motion above which a tap becomes a drag. Mirror of the Rust
 *  `MAX_TAP_DISTANCE` constant for hosts that only need the number
 *  (e.g. the mobile tab-strip pan slop). */
export const MAX_TAP_DISTANCE = 5;

/** Editor-area motion budget before a tap becomes scroll (mirror of
 *  `EDITOR_SCROLL_TAP_DISTANCE`). */
export const EDITOR_SCROLL_TAP_DISTANCE = 16;

/** Wall-clock millis a finger must hold before a long-press fires
 *  (mirror of `LONG_PRESS_MS`). */
export const LONG_PRESS_MS = 500;

/** Pixel pan budget before two-finger pan commits to scroll (mirror
 *  of `TWO_FINGER_PAN_THRESHOLD`). */
export const TWO_FINGER_PAN_THRESHOLD = 6;

/** Pixel distance-change before pinch commits to zoom (mirror of
 *  `PINCH_COMMIT_THRESHOLD`). */
export const PINCH_COMMIT_THRESHOLD = 18;

/** Coarse classification of the zone a touch started in. */
export type TouchZone = "terminal-body" | "chrome-panel" | "editor-area";

/** POD touch sample fed into the policy. */
export interface TouchSample {
  /** Stable per-finger id (Touch.identifier). */
  id: number;
  /** Canvas-local logical-pixel x. */
  x: number;
  /** Canvas-local logical-pixel y. */
  y: number;
  /** Wall-clock millis (performance.now() is fine). */
  timeMs: number;
}

/** Window-local layout for coordinate clamping. */
export interface TouchLayoutSize {
  width: number;
  height: number;
}

/** Plan returned to the caller; serialized 1:1 from the Rust
 *  `TouchAction` (see `WireTouchAction` in `input_policy.rs`). */
export type TouchAction =
  | { kind: "none" }
  | { kind: "start-simulated-left-click"; x: number; y: number }
  | { kind: "scroll"; dx: number; dy: number; x: number; y: number }
  | { kind: "update-mouse-position"; x: number; y: number }
  | { kind: "change-font-size"; direction: "increase" | "decrease" }
  | { kind: "end-simulated-left-click"; x: number; y: number }
  | { kind: "end-select" }
  | { kind: "end-scroll" }
  | { kind: "promote-tap-to-scroll" }
  | { kind: "open-context-menu"; x: number; y: number }
  | { kind: "two-finger-scroll"; dx: number; dy: number }
  | { kind: "suppress-native-gesture" };

const NONE: TouchAction = { kind: "none" };

/** Narrow a wasm-returned action object onto the `TouchAction` union.
 *  The Rust serializer is the only producer, so a `kind` string is the
 *  whole contract; anything malformed degrades to `none`. */
function asTouchAction(raw: unknown): TouchAction {
  if (raw && typeof raw === "object" && typeof (raw as { kind?: unknown }).kind === "string") {
    return raw as TouchAction;
  }
  return NONE;
}

let warnedMissingExports = false;
function warnMissingExports(): void {
  if (warnedMissingExports) return;
  warnedMissingExports = true;
  if (typeof console !== "undefined") {
    console.warn(
      "[neoism] served wasm bundle predates the shared touch-policy exports; " +
        "touch gestures are disabled until the bundle is rebuilt " +
        "(npm run build:wasm).",
    );
  }
}

/**
 * Stateful gesture classifier. Hold one instance per
 * canvas/`TerminalPanel`; feed `start`/`move`/`end` from the DOM
 * `touchstart` / `touchmove` / `touchend` listeners and run
 * `tickLongPress` from the existing RAF/interval loop.
 *
 * All decisions run in `neoism-frontend/shared/src/touch_policy.rs`;
 * see its tests for the canonical behaviour.
 */
export class TouchPolicy {
  private wasm: WasmTouchGesturePolicyInstance | null = null;

  /** Test seam: a fake input-policy module, or a getter for one (so
   *  tests can model the bundle arriving late). Production resolves
   *  the live wasm module per gesture. */
  constructor(
    private readonly bindings?:
      | WasmInputPolicyModule
      | (() => WasmInputPolicyModule | null)
      | null,
  ) {}

  private module(): WasmInputPolicyModule | null {
    if (typeof this.bindings === "function") return this.bindings();
    return this.bindings ?? wasmInputPolicy();
  }

  /** The Rust classifier instance, created lazily on the first
   *  gesture after the wasm bundle finishes loading. */
  private classifier(): WasmTouchGesturePolicyInstance | null {
    if (this.wasm) return this.wasm;
    const mod = this.module();
    if (!mod) return null; // Bundle still loading — nothing rendered yet.
    const Klass = mod.TouchGesturePolicy;
    if (!Klass) {
      warnMissingExports();
      return null;
    }
    this.wasm = new Klass();
    return this.wasm;
  }

  /** Reset to the idle state; call when the canvas loses focus or a
   *  host-owned pan (mobile tab strip / file tree) takes the gesture. */
  reset(): void {
    this.wasm?.reset();
  }

  /** True when at least one finger is currently active. */
  isActive(): boolean {
    return this.wasm?.is_active() ?? false;
  }

  /**
   * Decide whether the platform's back/forward swipe-from-edge
   * should be eaten for a touch starting in `zone`. Routed through
   * the shared `should_suppress_swipe_back`.
   */
  static shouldSuppressSwipeBack(zone: TouchZone): boolean {
    const decided = wasmInputPolicy()?.touch_should_suppress_swipe_back?.(zone);
    // Pre-wasm / stale-bundle fallback (source of truth: touch_policy.rs).
    return decided ?? zone === "editor-area";
  }

  /** Feed a `touchstart` sample with its zone hint. */
  start(sample: TouchSample, zone: TouchZone): TouchAction {
    const classifier = this.classifier();
    if (!classifier) return NONE;
    return asTouchAction(
      classifier.start(sample.id, sample.x, sample.y, sample.timeMs, zone),
    );
  }

  /** Feed a `touchmove` sample. Promotion actions
   *  (`start-simulated-left-click` / `promote-tap-to-scroll`) require
   *  the caller to re-feed the same sample, mirroring the desktop
   *  fork's recursive `on_touch_motion` pattern. */
  move(sample: TouchSample, layout: TouchLayoutSize): TouchAction {
    if (!this.wasm) return NONE;
    return asTouchAction(
      this.wasm.move(
        sample.id,
        sample.x,
        sample.y,
        sample.timeMs,
        layout.width,
        layout.height,
      ),
    );
  }

  /** Drive on a RAF / interval loop with `nowMs = performance.now()`. */
  tickLongPress(nowMs: number, layout: TouchLayoutSize): TouchAction {
    if (!this.wasm) return NONE;
    return asTouchAction(
      this.wasm.tick_long_press(nowMs, layout.width, layout.height),
    );
  }

  /** Feed a `touchend` / `touchcancel` sample. */
  end(sample: TouchSample, layout: TouchLayoutSize): TouchAction {
    if (!this.wasm) return NONE;
    return asTouchAction(
      this.wasm.end(
        sample.id,
        sample.x,
        sample.y,
        sample.timeMs,
        layout.width,
        layout.height,
      ),
    );
  }
}
