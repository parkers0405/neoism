import { test } from "node:test";
import assert from "node:assert/strict";

import {
  commitDispatch,
  keyEventIsImeComposing,
  shouldDropKeysDuringCompose,
} from "./imePolicy.ts";
import { installWasmInputPolicy } from "../terminal/createTerminal.ts";
import {
  loadWasmInputPolicyModule,
  skipReason,
} from "../presence/wasmPolicyTestSupport.mts";

// The decisions live in `neoism-frontend/shared/src/ime_state.rs` (its
// unit tests are canonical); this file pins the ADAPTER: the fallback
// path before the wasm bundle loads, the wasm-routed path after, and
// that the two agree.

const wasm = await loadWasmInputPolicyModule();
const imeSkip = skipReason(wasm, "ime_commit_dispatch");

function fakeKeyEvent(fields: { isComposing?: boolean; keyCode?: number }): KeyboardEvent {
  return {
    isComposing: fields.isComposing ?? false,
    keyCode: fields.keyCode ?? 0,
  } as KeyboardEvent;
}

// ── Fallback path (wasm not yet installed for this process) ─────────

test("fallback: commit dispatch keeps single chars raw, brackets multi", () => {
  assert.deepEqual(commitDispatch("a"), { text: "a", useBracketedPaste: false });
  // One CJK code point is one char regardless of UTF-8/UTF-16 length.
  assert.deepEqual(commitDispatch("あ"), { text: "あ", useBracketedPaste: false });
  assert.equal(commitDispatch("ab").useBracketedPaste, true);
  assert.equal(commitDispatch("こんにちは").useBracketedPaste, true);
  assert.equal(commitDispatch("").useBracketedPaste, false);
});

test("fallback: keys are dropped during compose", () => {
  assert.equal(shouldDropKeysDuringCompose(true), true);
  assert.equal(shouldDropKeysDuringCompose(false), false);
});

test("fallback: keydown composing detection covers flag and legacy 229", () => {
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({ isComposing: true })), true);
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({ keyCode: 229 })), true);
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({ keyCode: 65 })), false);
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({})), false);
});

// ── Wasm-routed path (shared Rust ime_state) ────────────────────────

test("wasm: adapter routes through shared ime_state and agrees with fallback", { skip: imeSkip }, () => {
  // Snapshot the fallback answers BEFORE installing the module.
  const fallbackAnswers = ["a", "あ", "ab", "こんにちは", ""].map((text) =>
    commitDispatch(text),
  );

  installWasmInputPolicy(wasm!);

  const wasmAnswers = ["a", "あ", "ab", "こんにちは", ""].map((text) =>
    commitDispatch(text),
  );
  assert.deepEqual(wasmAnswers, fallbackAnswers, "Rust and fallback must agree");

  assert.equal(shouldDropKeysDuringCompose(true), true);
  assert.equal(shouldDropKeysDuringCompose(false), false);
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({ isComposing: true })), true);
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({ keyCode: 229 })), true);
  assert.equal(keyEventIsImeComposing(fakeKeyEvent({ keyCode: 65 })), false);
});
