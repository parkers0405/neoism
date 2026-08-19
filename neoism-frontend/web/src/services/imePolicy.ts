/**
 * IME composition decisions, routed through the SHARED RUST policy
 * (`neoism-ui::ime_state`) via the wasm exports in
 * `wasm/src/rendered/input_policy.rs` (`ime_commit_dispatch`,
 * `ime_should_drop_keys_during_compose`, `ime_key_event_is_composing`).
 *
 * This file is a thin adapter, not a mirror: it looks up the loaded
 * wasm module per call (the bundle loads asynchronously after the
 * panel constructs) and only carries the minimal inline fallbacks
 * needed so typing never breaks in the pre-load window or on a stale
 * served bundle. The Rust module is the single source of truth; the
 * fallback expressions are pinned to it by
 * `neoism-frontend/shared/src/ime_state.rs`'s unit tests.
 */

import { wasmInputPolicy } from "../terminal/createTerminal";

/**
 * Threshold (in characters) above which an IME commit is forwarded
 * to the terminal via bracketed-paste rather than as raw keystrokes.
 * Matches `COMMIT_BRACKETED_PASTE_MIN_CHARS` in the Rust module —
 * kept exported for hosts that only need the constant.
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
 * Classify an IME `Commit` event: single-char commits stay raw
 * keystrokes (vim insert mode sees individual inputs); multi-char
 * commits use bracketed paste. Decision comes from the shared Rust
 * `commit_dispatch`; the fallback mirrors its `chars().count()`
 * semantics (`Array.from` counts code points, not UTF-16 units).
 */
export function commitDispatch(text: string): CommitDispatch {
  const dispatch = wasmInputPolicy()?.ime_commit_dispatch?.(text);
  if (dispatch) {
    return { text: dispatch.text, useBracketedPaste: dispatch.useBracketedPaste };
  }
  // Pre-wasm / stale-bundle fallback (source of truth: ime_state.rs).
  return {
    text,
    useBracketedPaste: Array.from(text).length >= COMMIT_BRACKETED_PASTE_MIN_CHARS,
  };
}

/**
 * Mode-locking during compose: while the IME shows a preedit popup,
 * every keystroke (Enter to commit, Escape to cancel, arrows for the
 * candidate list) belongs to the IME and must be swallowed by the
 * host. Routed through the shared `should_drop_keys_during_compose`.
 */
export function shouldDropKeysDuringCompose(hasPreedit: boolean): boolean {
  const decided =
    wasmInputPolicy()?.ime_should_drop_keys_during_compose?.(hasPreedit);
  // Pre-wasm / stale-bundle fallback (source of truth: ime_state.rs).
  return decided ?? hasPreedit;
}

/**
 * Returns `true` when the browser `keydown` event was fired by the
 * IME mid-composition and should be swallowed. Combines the standard
 * `isComposing` flag with the legacy `keyCode === 229` path some
 * IBus/fcitx + Chromium combos still emit — the classification lives
 * in the shared `key_event_is_ime_composing`; only the DOM field
 * extraction happens here.
 */
export function keyEventIsImeComposing(event: KeyboardEvent): boolean {
  const decided = wasmInputPolicy()?.ime_key_event_is_composing?.(
    event.isComposing === true,
    event.keyCode | 0,
  );
  // Pre-wasm / stale-bundle fallback (source of truth: ime_state.rs).
  return decided ?? (event.isComposing === true || event.keyCode === 229);
}
