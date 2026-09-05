export type MobileEditingSurface =
  | "terminal"
  | "editor"
  | "agent"
  | "markdown"
  | "overlay";

export type MobileTextFieldFamily =
  | "command-palette"
  | "finder"
  | "settings"
  | "file-browser"
  | "universal-modal"
  | "agent-question"
  | "agent-session-search"
  | "extensions"
  | "agent-composer"
  | "editor"
  | "markdown"
  | "terminal";

export interface MobileTextFieldHit {
  family: MobileTextFieldFamily;
  overlay: boolean;
}

export type SplashAction =
  | "change-directory"
  | "open-file-tree"
  | "open-notes"
  | "open-agent"
  | "search"
  | "open-command-palette"
  | "new-terminal";

export interface SplashKeyboardIntent {
  family: "command-palette" | "finder";
  anticipatedOverlay: true;
}

/** Splash is an input boundary. Only rows that immediately open a typing
 * overlay request the keyboard; every other row and every unpainted gap is
 * inert rather than falling through to terminal type-anywhere behavior. */
export function splashKeyboardIntent(
  splashActive: boolean,
  action: SplashAction | null,
): SplashKeyboardIntent | null {
  if (!splashActive || action === null) return null;
  if (action === "search") return { family: "finder", anticipatedOverlay: true };
  if (action === "change-directory" || action === "open-command-palette") {
    return { family: "command-palette", anticipatedOverlay: true };
  }
  return null;
}

export interface VisualViewportSample {
  width: number;
  height: number;
  offsetTop: number;
  scale: number;
}

/**
 * Convert a VisualViewport observation into the unzoomed CSS-pixel height
 * consumed by keyboard-inset policy.
 *
 * `VisualViewport.width/height` shrink for browser pinch zoom while the layout
 * viewport and `devicePixelRatio` do not.  Multiplying the visual extent by
 * `scale` removes that pinch transform.  `offsetTop` is already expressed in
 * layout CSS pixels and must not be scaled.  The visual width is deliberately
 * not a layout-width source: canvas/chrome geometry remains the layout
 * viewport's 1 CSS px coordinate space during keyboard animation and pinch.
 */
export function keyboardViewportObservation(
  layoutHeight: number,
  viewport: VisualViewportSample,
): { visualHeight: number; offsetTop: number } {
  const height = Number.isFinite(viewport.height) ? Math.max(0, viewport.height) : 0;
  const scale = Number.isFinite(viewport.scale) && viewport.scale > 0
    ? viewport.scale
    : 1;
  const offsetTop = Number.isFinite(viewport.offsetTop)
    ? Math.max(0, viewport.offsetTop)
    : 0;
  return {
    visualHeight: Math.min(Math.max(1, layoutHeight), height * scale),
    offsetTop,
  };
}

/** One-CSS-pixel mobile layout contract, independent of render DPR. */
export function mobileViewportLayout(
  layoutWidth: number,
  layoutHeight: number,
  keyboardInset: number,
  chromeScale: number,
): {
  layoutWidth: number;
  renderHeight: number;
  editableBottom: number;
  statusBottom: number;
  chromeScale: number;
} {
  const width = Math.max(1, Math.floor(layoutWidth));
  const height = Math.max(1, Math.floor(layoutHeight));
  const inset = Math.max(0, Math.min(height, keyboardInset));
  return {
    layoutWidth: width,
    renderHeight: height,
    editableBottom: height - inset,
    statusBottom: height,
    chromeScale,
  };
}

export function mobileDirectInsertFallback(
  coarsePointer: boolean,
  maxTouchPoints: number,
): boolean {
  return coarsePointer || maxTouchPoints > 0;
}

/** Keep the render/status surface physical-height; only editable content is
 * carved above the keyboard by the Rust chrome layout. */
export function mobileKeyboardLayout(
  layoutHeight: number,
  keyboardInset: number,
): { renderHeight: number; editableBottom: number; statusBottom: number } {
  const layout = mobileViewportLayout(1, layoutHeight, keyboardInset, 1);
  return {
    renderHeight: layout.renderHeight,
    editableBottom: layout.editableBottom,
    statusBottom: layout.statusBottom,
  };
}

/** Must be evaluated and acted on synchronously during touchstart on iOS. */
export function focusCaptureOnTouchStart(
  surface: MobileEditingSurface,
  tapWantsKeyboard: boolean,
): boolean {
  // Terminal scrollback is overwhelmingly a pan target. Its capture is
  // focused synchronously from touchend only after the gesture resolves as a
  // tap. The Agent composer retains its proven synchronous-focus workaround.
  // Canvas editors and overlays resolve tap-vs-pan first and focus from the
  // still-trusted touchend turn. That is the only reliable way to prevent the
  // first keyboard animation frame from peeking above a finger scroll on iOS.
  return tapWantsKeyboard && surface === "agent";
}

export type TouchKeyboardIntent = "none" | "provisional" | "deferred-tap";
export type TouchKeyboardResolution = "none" | "commit" | "cancel" | "focus-on-end";
export type TouchKeyboardFocusPhase = "idle" | "provisional" | "committed";
export type TouchKeyboardFocusEvent =
  | "touchstart-provisional"
  | "touchend-tap"
  | "viewport-resize"
  | "redraw"
  | "scroll-promotion"
  | "touchcancel"
  | "overlay-takeover"
  | "blur";

/**
 * Lifecycle of the DOM capture focus acquired from an iOS touchstart.
 * Viewport animation and rendering are observations, not reasons to revoke a
 * clean tap's focus. Gesture/ownership changes are the only cancellation
 * events. Kept pure so the exact Safari event ordering is deterministic in
 * tests.
 */
export function nextTouchKeyboardFocusPhase(
  phase: TouchKeyboardFocusPhase,
  event: TouchKeyboardFocusEvent,
): TouchKeyboardFocusPhase {
  switch (event) {
    case "touchstart-provisional":
      return "provisional";
    case "touchend-tap":
      return phase === "provisional" ? "committed" : phase;
    case "viewport-resize":
    case "redraw":
      return phase;
    case "scroll-promotion":
    case "touchcancel":
    case "overlay-takeover":
    case "blur":
      return "idle";
  }
}

/** Ignore a release-coordinate miss caused by keyboard reflow, but not a new
 * modal/panel that took ownership during the tap. */
export function preserveCommittedTouchFocus(
  committing: boolean,
  overlayWasActiveAtTouchStart: boolean,
  overlayIsActiveAfterTap: boolean,
  anticipatedOverlay = false,
): boolean {
  return committing && !(
    overlayIsActiveAfterTap && !overlayWasActiveAtTouchStart && !anticipatedOverlay
  );
}

export function touchKeyboardIntent(
  surface: MobileEditingSurface,
  tapWantsKeyboard: boolean,
  captureAlreadyFocused: boolean,
): TouchKeyboardIntent {
  if (!tapWantsKeyboard || captureAlreadyFocused) return "none";
  return surface === "agent" ? "provisional" : "deferred-tap";
}

/** Resolve the sole text target allowed to request the keyboard for a touch.
 * An overlay is an ownership boundary: a miss cannot fall through to the
 * Agent composer/editor painted underneath it. */
export function mobileTextFieldIntent(
  surface: Exclude<MobileEditingSurface, "overlay">,
  overlayActive: boolean,
  overlayField: MobileTextFieldFamily | null,
  surfaceWantsKeyboard: boolean,
): MobileTextFieldHit | null {
  if (overlayActive) {
    return overlayField ? { family: overlayField, overlay: true } : null;
  }
  if (overlayField) return { family: overlayField, overlay: true };
  if (!surfaceWantsKeyboard) return null;
  const family: MobileTextFieldFamily = surface === "agent"
    ? "agent-composer"
    : surface;
  return { family, overlay: false };
}

export function resolveTouchKeyboardIntent(
  intent: TouchKeyboardIntent,
  outcome: "tap" | "scroll" | "cancel",
): TouchKeyboardResolution {
  if (intent === "provisional") return outcome === "tap" ? "commit" : "cancel";
  if (intent === "deferred-tap" && outcome === "tap") return "focus-on-end";
  return "none";
}

/** Decode one MobileKeyboard byte commit into browser-style editor keys. */
export function mobileDirectInputKeys(bytes: Uint8Array): string[] {
  if (bytes.length === 1) {
    switch (bytes[0]) {
      case 0x09:
        return ["Tab"];
      case 0x0a:
      case 0x0d:
        return ["Enter"];
      case 0x1b:
        return ["Escape"];
      case 0x08:
      case 0x7f:
        return ["Backspace"];
    }
  }
  const text = new TextDecoder().decode(bytes);
  const named = new Map([
    ["\x1b[A", "ArrowUp"], ["\x1bOA", "ArrowUp"],
    ["\x1b[B", "ArrowDown"], ["\x1bOB", "ArrowDown"],
    ["\x1b[C", "ArrowRight"], ["\x1bOC", "ArrowRight"],
    ["\x1b[D", "ArrowLeft"], ["\x1bOD", "ArrowLeft"],
  ]).get(text);
  if (named) return [named];
  return Array.from(text, (ch) => ch === "\n" || ch === "\r" ? "Enter" : ch);
}