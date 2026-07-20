export type MobileKeyboardContext =
  | "code"
  | "text"
  | "url"
  | "search"
  /** Multi-line buffer editing (markdown / editor): Enter is a newline,
   *  so the iOS return key must say "return", not "send". */
  | "editor";

export interface MobileKeyboardInsets {
  /** Pixels obscured at the bottom of the layout viewport by the
   *  on-screen keyboard (browser-reported or estimated). 0 when the
   *  soft keyboard is closed. */
  bottom: number;
  /** True iff the soft keyboard appears to be open right now. */
  keyboardOpen: boolean;
}

export interface MobileKeyboardOptions {
  mount: HTMLElement;
  /** Forwards captured input bytes back to the terminal. */
  onBytes: (bytes: Uint8Array) => void;
  /** Fired whenever the visualViewport inset / keyboard-open state
   *  shifts — typically wired to `TerminalPanel.handleResize` so the
   *  cell grid contracts above the soft keyboard. */
  onInsetsChanged?: (insets: MobileKeyboardInsets) => void;
  /** Optional element whose scroll-into-view behaviour mirrors the
   *  current caret. Falls back to the mount element when omitted. */
  scrollAnchor?: HTMLElement;
}

/** Two-bit modifier latch — set when the user taps Ctrl/Shift/Alt/Meta
 *  on the soft toolbar. Cleared after the next non-modifier key emits
 *  unless the same modifier was double-tapped (sticky). */
interface ModifierLatch {
  active: boolean;
  sticky: boolean;
}

/**
 * Soft keyboard polish for browser-based terminals.
 *
 * Responsibilities:
 *   1. Surface a hidden contenteditable capture so iOS Safari / Android
 *      Chrome pop their on-screen keyboard when the user taps the
 *      terminal. Browser-native composition flows through `beforeinput`.
 *   2. Track the `visualViewport` so the host can resize the cell grid
 *      to the keyboard-adjusted layout viewport. Falls back to a
 *      `window.resize`-only path on browsers without
 *      `window.visualViewport` (older WebKit).
 *   3. Optionally render a one-row toolbar with the modifier + arrow
 *      keys that physical keyboards have but soft keyboards don't,
 *      so users can fire Ctrl-C / Esc / arrow nav without long-pressing
 *      glyph picks.
 *   4. Scroll the caret row back into view when the keyboard opens.
 *   5. Honour `navigator.virtualKeyboard` overlay-content mode where
 *      available so the browser stops scrolling the layout for us and
 *      we own the inset math.
 *
 * Composition target is `inputmode="text"` by default but switches to
 * `none` once the in-page toolbar is showing — this avoids stacking two
 * keyboards on iPadOS. Autocorrect / autocapitalize / spellcheck are
 * disabled because code-like input is the dominant use case; the
 * `setContext("text")` setter relaxes them when the host knows the
 * user is editing prose (e.g., a chat composer).
 */
export class MobileKeyboard {
  private readonly capture: HTMLDivElement;
  private toolbar: HTMLDivElement | null = null;
  private toolbarVisible = false;
  private readonly beforeInputHandler: (event: InputEvent) => void;
  private readonly keydownHandler: (event: KeyboardEvent) => void;
  private readonly focusHandler: () => void;
  private readonly viewportHandler: () => void;
  private readonly windowResizeHandler: () => void;
  private currentInsets: MobileKeyboardInsets = { bottom: 0, keyboardOpen: false };
  private currentContext: MobileKeyboardContext = "code";
  private modifiers: Record<"ctrl" | "shift" | "alt" | "meta", ModifierLatch> = {
    ctrl: { active: false, sticky: false },
    shift: { active: false, sticky: false },
    alt: { active: false, sticky: false },
    meta: { active: false, sticky: false },
  };
  /** True once the on-screen keyboard is being driven by the browser.
   *  Used to suppress nuisance toolbar pop-ins on hover/click sequences
   *  that aren't actually focused. */
  private captureFocused = false;

  constructor(private readonly options: MobileKeyboardOptions) {
    this.capture = document.createElement("div");
    this.capture.className = "mobile-keyboard-capture";
    this.capture.contentEditable = "true";
    this.applyContextAttributes();
    this.capture.setAttribute("aria-label", "Terminal input");
    this.capture.setAttribute("data-context", this.currentContext);

    this.beforeInputHandler = (event) => this.handleBeforeInput(event);
    this.keydownHandler = (event) => this.handleKeyDown(event);
    this.focusHandler = () => {
      this.captureFocused = true;
      this.scrollAnchorIntoView();
    };
    this.capture.addEventListener("beforeinput", this.beforeInputHandler);
    this.capture.addEventListener("keydown", this.keydownHandler);
    this.capture.addEventListener("focus", this.focusHandler);
    this.capture.addEventListener("blur", () => {
      this.captureFocused = false;
    });

    options.mount.appendChild(this.capture);

    // Opt into VirtualKeyboard overlay-content so the layout viewport
    // doesn't scroll when the keyboard opens; we own the inset math via
    // visualViewport instead. Best-effort: only Chromium ships this.
    const virtualKeyboard = navigatorVirtualKeyboard();
    if (virtualKeyboard) {
      try {
        virtualKeyboard.overlaysContent = true;
      } catch {
        /* Defensive: spec allows setter to throw on unsupported builds. */
      }
    }

    this.viewportHandler = () => this.recomputeInsets();
    this.windowResizeHandler = () => this.recomputeInsets();
    const viewport = window.visualViewport;
    if (viewport) {
      viewport.addEventListener("resize", this.viewportHandler);
      viewport.addEventListener("scroll", this.viewportHandler);
    }
    window.addEventListener("resize", this.windowResizeHandler);

    // Initial publish — even with no keyboard, downstream code should
    // see the resting inset (0, closed) so it can stop suppressing layout.
    queueMicrotask(() => this.recomputeInsets());
  }

  focus(): void {
    this.capture.focus({ preventScroll: true });
  }

  blur(): void {
    this.captureFocused = false;
    this.capture.blur();
  }

  dispose(): void {
    this.capture.removeEventListener("beforeinput", this.beforeInputHandler);
    this.capture.removeEventListener("keydown", this.keydownHandler);
    this.capture.removeEventListener("focus", this.focusHandler);
    const viewport = window.visualViewport;
    if (viewport) {
      viewport.removeEventListener("resize", this.viewportHandler);
      viewport.removeEventListener("scroll", this.viewportHandler);
    }
    window.removeEventListener("resize", this.windowResizeHandler);
    this.hideToolbar();
    this.capture.remove();
  }

  /** Current keyboard-adjusted insets. Useful for tests + as the
   *  authoritative source the host can re-read after a layout pass. */
  insets(): MobileKeyboardInsets {
    return { ...this.currentInsets };
  }

  /** True iff a soft keyboard is currently open. */
  isKeyboardOpen(): boolean {
    return this.currentInsets.keyboardOpen;
  }

  /** Toggle the in-page modifier / arrow toolbar. Callers can wire this
   *  to a Cmd-comma shortcut or a chrome button; auto-detect tries to
   *  do the right thing for tap-only sessions. */
  setToolbarVisible(visible: boolean): void {
    if (visible === this.toolbarVisible) return;
    if (visible) this.showToolbar();
    else this.hideToolbar();
  }

  /** Tells the keyboard what kind of input the focused pane wants:
   *  `"code"` (default) disables autocorrect / autocapitalize and
   *  forces `inputmode="text"`; `"text"` relaxes them for prose;
   *  `"url"` / `"search"` pick the matching `inputmode` hint so iOS
   *  shows a `.com` / search keyboard. */
  setContext(context: MobileKeyboardContext): void {
    if (context === this.currentContext) return;
    this.currentContext = context;
    this.applyContextAttributes();
    this.capture.setAttribute("data-context", context);
  }

  // ------------------------------------------------------------------
  // Internals
  // ------------------------------------------------------------------

  private applyContextAttributes(): void {
    const codeLike =
      this.currentContext === "code" ||
      this.currentContext === "editor" ||
      this.currentContext === "url" ||
      this.currentContext === "search";
    this.capture.setAttribute("autocapitalize", codeLike ? "off" : "sentences");
    this.capture.setAttribute("autocorrect", codeLike ? "off" : "on");
    this.capture.setAttribute("spellcheck", codeLike ? "false" : "true");
    this.capture.setAttribute(
      "inputmode",
      this.toolbarVisible
        ? "none"
        : this.currentContext === "url"
          ? "url"
          : this.currentContext === "search"
            ? "search"
            : "text",
    );
    // Hint Enter as "go" / "send" on one-line surfaces; buffer editing
    // (markdown / editor) inserts newlines, so it keeps the plain
    // return key.
    this.capture.setAttribute(
      "enterkeyhint",
      this.currentContext === "search"
        ? "search"
        : this.currentContext === "editor"
          ? "enter"
          : "send",
    );
  }

  private handleBeforeInput(event: InputEvent): void {
    event.preventDefault();
    if (event.inputType === "insertText" && event.data) {
      this.emitTextRespectingModifiers(event.data);
    } else if (
      event.inputType === "insertParagraph" ||
      event.inputType === "insertLineBreak"
    ) {
      this.options.onBytes(Uint8Array.of(0x0d));
      this.clearTransientModifiers();
    } else if (
      event.inputType === "deleteContentBackward" ||
      event.inputType === "deleteByCut"
    ) {
      this.options.onBytes(Uint8Array.of(0x7f));
      this.clearTransientModifiers();
    }
    // Always reset the buffer so the contenteditable never accumulates.
    this.capture.textContent = "";
  }

  private handleKeyDown(event: KeyboardEvent): void {
    // Some mobile keyboards bypass beforeinput for navigation keys.
    if (event.key === "Enter") {
      event.preventDefault();
      this.options.onBytes(Uint8Array.of(0x0d));
      this.clearTransientModifiers();
    } else if (event.key === "Backspace") {
      event.preventDefault();
      this.options.onBytes(Uint8Array.of(0x7f));
      this.clearTransientModifiers();
    }
  }

  private emitTextRespectingModifiers(data: string): void {
    if (this.modifiers.ctrl.active && data.length === 1) {
      const lower = data.toLowerCase().charCodeAt(0);
      if (lower >= 97 && lower <= 122) {
        this.options.onBytes(Uint8Array.of(lower - 96));
        this.clearTransientModifiers();
        return;
      }
    }
    this.options.onBytes(new TextEncoder().encode(data));
    this.clearTransientModifiers();
  }

  private clearTransientModifiers(): void {
    for (const key of Object.keys(this.modifiers) as Array<
      keyof typeof this.modifiers
    >) {
      const latch = this.modifiers[key];
      if (latch.active && !latch.sticky) {
        latch.active = false;
        this.syncModifierButton(key, false);
      }
    }
  }

  private syncModifierButton(
    key: keyof typeof this.modifiers,
    active: boolean,
  ): void {
    if (!this.toolbar) return;
    const btn = this.toolbar.querySelector<HTMLButtonElement>(
      `button[data-modifier='${key}']`,
    );
    if (!btn) return;
    btn.classList.toggle("is-active", active);
    btn.classList.toggle("is-sticky", this.modifiers[key].sticky);
  }

  private recomputeInsets(): void {
    const viewport = window.visualViewport;
    let bottom = 0;
    if (viewport) {
      // `innerHeight` is the layout viewport (above keyboard not
      // subtracted on Android Chrome); `visualViewport.height` shrinks
      // when the soft keyboard opens. Their delta minus any pageTop
      // offset is the keyboard inset in CSS pixels. Clamp to >= 0 in
      // case the viewport is taller than the layout (e.g., during a
      // pinch-zoom unzoom transition).
      bottom = Math.max(
        0,
        Math.round(window.innerHeight - (viewport.height + viewport.offsetTop)),
      );
    }
    // Tolerate a 4px slop — browsers report fractional pixels on URL-bar
    // hide/show that aren't a real keyboard event.
    const open = bottom > 4;
    const previous = this.currentInsets;
    if (previous.bottom === bottom && previous.keyboardOpen === open) {
      return;
    }
    this.currentInsets = { bottom, keyboardOpen: open };
    // CSS custom prop so stylesheets can offset chrome without JS.
    document.documentElement.style.setProperty(
      "--mobile-keyboard-inset-bottom",
      `${bottom}px`,
    );
    document.documentElement.classList.toggle(
      "mobile-keyboard-open",
      open,
    );
    try {
      this.options.onInsetsChanged?.(this.insets());
    } catch (err) {
      if (typeof console !== "undefined") {
        console.warn("[mobile-keyboard] onInsetsChanged threw", err);
      }
    }
    if (open && this.captureFocused) {
      this.scrollAnchorIntoView();
    }
  }

  private scrollAnchorIntoView(): void {
    // The terminal owns keyboard avoidance by resizing against
    // visualViewport. Asking the browser to scroll the canvas as well
    // causes the whole grid to jump on iOS/Android address-bar changes.
  }

  private showToolbar(): void {
    if (this.toolbar) return;
    const bar = document.createElement("div");
    bar.className = "mobile-keyboard-toolbar";
    bar.setAttribute("role", "toolbar");
    bar.setAttribute("aria-label", "Terminal modifiers");
    this.buildToolbarRow(bar, [
      this.makeModifierButton("Esc", "esc"),
      this.makeModifierButton("Tab", "tab"),
      this.makeModifierButton("Ctrl", "ctrl"),
      this.makeModifierButton("Shift", "shift"),
      this.makeModifierButton("Alt", "alt"),
      this.makeModifierButton("Cmd", "meta"),
      this.makeArrowButton("←", "ArrowLeft"),
      this.makeArrowButton("↓", "ArrowDown"),
      this.makeArrowButton("↑", "ArrowUp"),
      this.makeArrowButton("→", "ArrowRight"),
    ]);
    this.options.mount.appendChild(bar);
    this.toolbar = bar;
    this.toolbarVisible = true;
    this.applyContextAttributes();
  }

  private hideToolbar(): void {
    if (!this.toolbar) {
      this.toolbarVisible = false;
      return;
    }
    this.toolbar.remove();
    this.toolbar = null;
    this.toolbarVisible = false;
    this.applyContextAttributes();
  }

  private buildToolbarRow(host: HTMLDivElement, buttons: HTMLButtonElement[]): void {
    for (const btn of buttons) host.appendChild(btn);
  }

  private makeModifierButton(
    label: string,
    kind: "esc" | "tab" | "ctrl" | "shift" | "alt" | "meta",
  ): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "mobile-keyboard-key";
    btn.textContent = label;
    btn.setAttribute("data-key-kind", kind);
    if (kind === "ctrl" || kind === "shift" || kind === "alt" || kind === "meta") {
      btn.setAttribute("data-modifier", kind);
    }
    btn.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      // Re-focus the capture so the next character lands in the terminal
      // path, not the toolbar host.
      this.capture.focus({ preventScroll: true });
    });
    btn.addEventListener("click", (event) => {
      event.preventDefault();
      this.handleToolbarPress(kind, event.detail >= 2);
    });
    return btn;
  }

  private makeArrowButton(label: string, key: string): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "mobile-keyboard-key mobile-keyboard-key-arrow";
    btn.textContent = label;
    btn.setAttribute("data-key-kind", "arrow");
    btn.setAttribute("data-arrow", key);
    btn.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      this.capture.focus({ preventScroll: true });
    });
    btn.addEventListener("click", (event) => {
      event.preventDefault();
      this.emitNamedKey(key);
    });
    return btn;
  }

  private handleToolbarPress(
    kind: "esc" | "tab" | "ctrl" | "shift" | "alt" | "meta",
    doubleTap: boolean,
  ): void {
    if (kind === "esc") {
      this.options.onBytes(Uint8Array.of(0x1b));
      this.clearTransientModifiers();
      return;
    }
    if (kind === "tab") {
      this.options.onBytes(Uint8Array.of(0x09));
      this.clearTransientModifiers();
      return;
    }
    const latch = this.modifiers[kind];
    if (doubleTap) {
      latch.sticky = !latch.sticky;
      latch.active = latch.sticky;
    } else {
      latch.active = !latch.active;
      latch.sticky = false;
    }
    this.syncModifierButton(kind, latch.active);
  }

  private emitNamedKey(key: string): void {
    switch (key) {
      case "ArrowUp":
        this.options.onBytes(new TextEncoder().encode("\x1b[A"));
        break;
      case "ArrowDown":
        this.options.onBytes(new TextEncoder().encode("\x1b[B"));
        break;
      case "ArrowRight":
        this.options.onBytes(new TextEncoder().encode("\x1b[C"));
        break;
      case "ArrowLeft":
        this.options.onBytes(new TextEncoder().encode("\x1b[D"));
        break;
      default:
        break;
    }
    this.clearTransientModifiers();
  }
}

/** Narrow accessor for the Chromium-only `navigator.virtualKeyboard`
 *  surface — kept in a helper so the type cast is local to one place. */
function navigatorVirtualKeyboard():
  | { overlaysContent: boolean }
  | null {
  const nav = navigator as Navigator & {
    virtualKeyboard?: { overlaysContent: boolean };
  };
  return nav.virtualKeyboard ?? null;
}
