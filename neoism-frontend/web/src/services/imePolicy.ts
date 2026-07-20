/**
 * IME composition decisions, mirrored 1:1 from the shared Rust
 * `neoism-ui::ime_state` module. Keeps the web frontend and the
 * desktop fork in lock step on every composition-driven choice:
 *
 *   - mode-locking during compose (drop real key events while the
 *     IME owns the keyboard)
 *   - commit-string routing (raw single keystroke vs bracketed
 *     paste for multi-char commits)
 *   - preedit cursor offset clamping
 *
 * If the shared Rust module changes one of these decisions, this
 * file is the single mirror that must change with it.
 *
 * See `neoism-frontend/shared/src/ime_state.rs` for the source of
 * truth and the unit tests that pin the behavior.
 */

/**
 * Threshold (in characters) above which an IME commit is forwarded
 * to the terminal via bracketed-paste rather than as raw keystrokes.
 * Matches `COMMIT_BRACKETED_PASTE_MIN_CHARS` in the Rust module.
 *
 * Single-character commits (the common case for Japanese / Chinese
 * typewriter input) go through the raw path so terminal modes that
 * care about per-key timing (vim insert mode, readline) see them as
 * individual events.
 */
export const COMMIT_BRACKETED_PASTE_MIN_CHARS = 2;

export interface CommitDispatch {
  /** The text to forward to the focused input surface. */
  text: string;
  /**
   * Whether the host should wrap the text in bracketed-paste markers
   * (`ESC [ 200 ~` … `ESC [ 201 ~`) when the active mode supports it.
   */
  useBracketedPaste: boolean;
}

/**
 * Pure classifier for an IME `Commit` event.
 *
 * Mirrors `commit_dispatch` in `ime_state.rs`. Single-char commits
 * stay as raw keystrokes so vim insert mode sees them as individual
 * inputs; multi-char commits use bracketed paste so terminal modes
 * with autocomplete / abbreviation expansion don't fire mid-string.
 *
 * `chars().count()` in Rust counts Unicode scalar values (code
 * points); we mirror that here with `Array.from(text).length` so a
 * single CJK code point still counts as one char regardless of UTF-8
 * byte length.
 */
export function commitDispatch(text: string): CommitDispatch {
  const charCount = Array.from(text).length;
  return {
    text,
    useBracketedPaste: charCount >= COMMIT_BRACKETED_PASTE_MIN_CHARS,
  };
}

/**
 * Pure classifier for key events while a preedit is in flight.
 *
 * While the IME is showing a preedit popup, every keystroke (Enter
 * to commit, Escape to cancel, arrows to navigate the candidate
 * list) belongs to the IME, not the underlying terminal
 * surface. The browser fires `keydown` alongside `compositionupdate`
 * with `KeyboardEvent.isComposing === true` (or `keyCode === 229` on
 * legacy hosts); the host must swallow them.
 *
 * Returns `true` when the host should swallow the key event.
 *
 * Mirrors `should_drop_keys_during_compose` in `ime_state.rs`.
 */
export function shouldDropKeysDuringCompose(hasPreedit: boolean): boolean {
  return hasPreedit;
}

/**
 * Returns `true` when the browser `keydown` event was fired by the
 * IME mid-composition and should be swallowed by the host. Combines
 * the standard `isComposing` flag (Chromium / Firefox / Safari) with
 * the `keyCode === 229` fallback some legacy / Edge paths emit.
 */
export function keyEventIsImeComposing(event: KeyboardEvent): boolean {
  if (event.isComposing) {
    return true;
  }
  // 229 is the legacy "IME is processing the key" code that pre-dates
  // `isComposing`. Some Linux IBus / fcitx + Chromium combos still
  // surface keystrokes that way for the final commit cycle.
  if (event.keyCode === 229) {
    return true;
  }
  return false;
}
