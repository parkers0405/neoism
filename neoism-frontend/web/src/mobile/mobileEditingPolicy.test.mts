import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  focusCaptureOnTouchStart,
  keyboardViewportObservation,
  mobileDirectInsertFallback,
  mobileDirectInputKeys,
  mobileKeyboardLayout,
  mobileTextFieldIntent,
  mobileViewportLayout,
  nextTouchKeyboardFocusPhase,
  preserveCommittedTouchFocus,
  touchKeyboardIntent,
  resolveTouchKeyboardIntent,
  splashKeyboardIntent,
} from "./mobileEditingPolicy.ts";
import { computeSizeContract } from "../terminal/sizeContract.ts";

test("mobile direct insert selects touch web but not desktop web", () => {
  assert.equal(mobileDirectInsertFallback(true, 0), true);
  assert.equal(mobileDirectInsertFallback(false, 5), true);
  assert.equal(mobileDirectInsertFallback(false, 0), false);
});

test("scroll promotion cancels provisional focus while resolved taps focus", () => {
  assert.equal(resolveTouchKeyboardIntent("provisional", "tap"), "commit");
  assert.equal(resolveTouchKeyboardIntent("provisional", "scroll"), "cancel");
  assert.equal(resolveTouchKeyboardIntent("provisional", "cancel"), "cancel");
  assert.equal(resolveTouchKeyboardIntent("deferred-tap", "tap"), "focus-on-end");
  assert.equal(resolveTouchKeyboardIntent("deferred-tap", "scroll"), "none");
});

test("terminal direct drag crosses intent and cannot resolve as an input tap", () => {
  const intent = touchKeyboardIntent("terminal", true, false);
  assert.equal(intent, "deferred-tap");
  assert.equal(resolveTouchKeyboardIntent(intent, "scroll"), "none");
});

test("keyboard inset excludes the physical-bottom status band", () => {
  assert.deepEqual(mobileKeyboardLayout(844, 301), {
    renderHeight: 844,
    editableBottom: 543,
    statusBottom: 844,
  });
});

test("iPhone CSS viewport and DPR stay stable before and after keyboard", () => {
  const before = mobileViewportLayout(390, 844, 0, 1);
  const after = mobileViewportLayout(390, 844, 301, 1);
  const beforeRender = computeSizeContract(before.layoutWidth, before.renderHeight, 3, 4096);
  const afterRender = computeSizeContract(after.layoutWidth, after.renderHeight, 3, 4096);

  assert.equal(before.layoutWidth, 390);
  assert.equal(after.layoutWidth, before.layoutWidth);
  assert.equal(after.chromeScale, before.chromeScale);
  assert.equal(after.renderHeight, before.renderHeight);
  assert.equal(after.editableBottom, 543);
  assert.equal(after.statusBottom, 844);
  assert.deepEqual(afterRender, beforeRender);
  assert.equal(beforeRender.scale, 3);
  assert.equal(beforeRender.physicalWidth, 1170);
  assert.equal(beforeRender.physicalHeight, 2532);
});

test("browser visualViewport scale is normalized only for keyboard detection", () => {
  const pinched = keyboardViewportObservation(844, {
    width: 195,
    height: 422,
    offsetTop: 0,
    scale: 2,
  });
  const keyboardWhilePinched = keyboardViewportObservation(844, {
    width: 195,
    height: 271.5,
    offsetTop: 0,
    scale: 2,
  });
  assert.deepEqual(pinched, { visualHeight: 844, offsetTop: 0 });
  assert.deepEqual(keyboardWhilePinched, { visualHeight: 543, offsetTop: 0 });

  const before = mobileViewportLayout(390, 844, 0, 1);
  const after = mobileViewportLayout(
    390,
    844,
    844 - keyboardWhilePinched.visualHeight,
    1,
  );
  assert.equal(after.layoutWidth, before.layoutWidth);
  assert.equal(after.chromeScale, before.chromeScale);
  assert.equal(after.editableBottom, 543);
  // Even if a browser reports a transient visualViewport scale, neither the
  // canvas CSS rect nor its DPR/backing-store contract consumes that scale.
  assert.deepEqual(
    computeSizeContract(after.layoutWidth, after.renderHeight, 3, 4096),
    computeSizeContract(before.layoutWidth, before.renderHeight, 3, 4096),
  );
});

test("document viewport contract disables browser page scaling", () => {
  const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
  const viewport = html.match(/<meta\s+name="viewport"\s+content="([^"]+)"/s)?.[1] ?? "";
  assert.match(viewport, /initial-scale=1/);
  assert.match(viewport, /maximum-scale=1/);
  assert.match(viewport, /user-scalable=no/);
});

test("initial agent composer routes capture focus on touchstart", () => {
  assert.equal(focusCaptureOnTouchStart("agent", true), true);
  assert.equal(focusCaptureOnTouchStart("agent", false), false);
});

test("touch focus is provisional only for Agent and deferred elsewhere", () => {
  assert.equal(touchKeyboardIntent("markdown", true, false), "deferred-tap");
  assert.equal(touchKeyboardIntent("editor", true, false), "deferred-tap");
  assert.equal(touchKeyboardIntent("agent", true, false), "provisional");
  assert.equal(touchKeyboardIntent("overlay", true, false), "deferred-tap");
  assert.equal(touchKeyboardIntent("terminal", true, false), "deferred-tap");
  assert.equal(focusCaptureOnTouchStart("terminal", true), false);
  assert.equal(touchKeyboardIntent("terminal", false, false), "none");
  assert.equal(touchKeyboardIntent("markdown", true, true), "none");
});

test("cold editor and markdown taps focus on touchend while drags never focus", () => {
  for (const surface of ["editor", "markdown"] as const) {
    assert.equal(focusCaptureOnTouchStart(surface, true), false);
    const intent = touchKeyboardIntent(surface, true, false);
    assert.equal(resolveTouchKeyboardIntent(intent, "tap"), "focus-on-end");
    assert.equal(resolveTouchKeyboardIntent(intent, "scroll"), "none");
    assert.equal(resolveTouchKeyboardIntent(intent, "cancel"), "none");
  }
  assert.equal(focusCaptureOnTouchStart("agent", true), true);
});

test("mobile overlay fields defer focus until a clean tap", () => {
  const families = [
    "command-palette",
    "finder",
    "settings",
    "file-browser",
    "universal-modal",
    "agent-question",
    "extensions",
  ] as const;
  for (const family of families) {
    const hit = mobileTextFieldIntent("agent", true, family, true);
    assert.deepEqual(hit, { family, overlay: true });
    const intent = touchKeyboardIntent("overlay", hit !== null, false);
    assert.equal(focusCaptureOnTouchStart("overlay", true), false, family);
    assert.equal(resolveTouchKeyboardIntent(intent, "tap"), "focus-on-end", family);
    assert.equal(resolveTouchKeyboardIntent(intent, "scroll"), "none", family);
  }
});

test("all splash rows and background have explicit keyboard intent", () => {
  const expected = new Map([
    ["change-directory", "command-palette"],
    ["open-file-tree", null],
    ["open-notes", null],
    ["open-agent", null],
    ["search", "finder"],
    ["open-command-palette", "command-palette"],
    ["new-terminal", null],
  ] as const);
  for (const [action, family] of expected) {
    assert.equal(splashKeyboardIntent(true, action)?.family ?? null, family, action);
  }
  assert.equal(splashKeyboardIntent(true, null), null, "background/gap/below rows");
  assert.equal(splashKeyboardIntent(false, "search"), null, "stale rect while inactive");
});

test("anticipated splash overlay keeps clean-tap focus but drags remain inert", () => {
  const hit = splashKeyboardIntent(true, "search");
  const intent = touchKeyboardIntent("overlay", hit !== null, false);
  assert.equal(resolveTouchKeyboardIntent(intent, "tap"), "focus-on-end");
  assert.equal(resolveTouchKeyboardIntent(intent, "scroll"), "none");
  assert.equal(preserveCommittedTouchFocus(true, false, true, true), true);
  assert.equal(preserveCommittedTouchFocus(true, false, true, false), false);
});

test("overlay rows do not fall through to the Agent composer", () => {
  assert.equal(mobileTextFieldIntent("agent", true, null, true), null);
  assert.deepEqual(mobileTextFieldIntent("agent", false, null, true), {
    family: "agent-composer",
    overlay: false,
  });
  assert.equal(mobileTextFieldIntent("agent", false, null, false), null);
});

test("session search first tap resolves to one deferred overlay focus", () => {
  const hit = mobileTextFieldIntent(
    "agent",
    true,
    "agent-session-search",
    true,
  );
  assert.deepEqual(hit, {
    family: "agent-session-search",
    overlay: true,
  });
  const intent = touchKeyboardIntent("overlay", hit !== null, false);
  assert.equal(focusCaptureOnTouchStart("overlay", true), false);
  assert.equal(resolveTouchKeyboardIntent(intent, "tap"), "focus-on-end");
  assert.equal(preserveCommittedTouchFocus(true, true, true), true);
});

test("session search scroll cancellation never focuses or falls through", () => {
  const hit = mobileTextFieldIntent(
    "agent",
    true,
    "agent-session-search",
    true,
  );
  const intent = touchKeyboardIntent("overlay", hit !== null, false);
  assert.equal(resolveTouchKeyboardIntent(intent, "scroll"), "none");
  assert.equal(resolveTouchKeyboardIntent(intent, "cancel"), "none");
  assert.equal(mobileTextFieldIntent("agent", true, null, true), null);
});

test("session search typing route preserves every direct input commit", () => {
  const bytes = [
    new TextEncoder().encode("n"),
    new TextEncoder().encode("otes"),
    Uint8Array.of(0x7f),
  ];
  assert.deepEqual(bytes.flatMap(mobileDirectInputKeys), [
    "n", "o", "t", "e", "s", "Backspace",
  ]);
  assert.equal(
    mobileTextFieldIntent("agent", true, "agent-session-search", true)?.family,
    "agent-session-search",
  );
});

test("direct IME commits preserve every repeated Enter and navigation key", () => {
  assert.deepEqual(
    Array.from({ length: 24 }, () => Uint8Array.of(0x0d))
      .flatMap(mobileDirectInputKeys),
    Array(24).fill("Enter"),
  );
  assert.deepEqual(mobileDirectInputKeys(new TextEncoder().encode("a\nb")), [
    "a", "Enter", "b",
  ]);
  assert.deepEqual(mobileDirectInputKeys(new TextEncoder().encode("\x1b[B")), [
    "ArrowDown",
  ]);
  assert.deepEqual(mobileDirectInputKeys(Uint8Array.of(0x7f)), ["Backspace"]);
});

test("cold Agent composer tap survives touchend, viewport resize, and redraw", () => {
  let phase = nextTouchKeyboardFocusPhase("idle", "touchstart-provisional");
  assert.equal(phase, "provisional");
  phase = nextTouchKeyboardFocusPhase(phase, "viewport-resize");
  assert.equal(phase, "provisional");
  phase = nextTouchKeyboardFocusPhase(phase, "touchend-tap");
  assert.equal(phase, "committed");
  phase = nextTouchKeyboardFocusPhase(phase, "viewport-resize");
  phase = nextTouchKeyboardFocusPhase(phase, "redraw");
  assert.equal(phase, "committed");
});

test("scroll promotion, cancellation, and overlay takeover revoke provisional focus", () => {
  for (const event of [
    "scroll-promotion",
    "touchcancel",
    "overlay-takeover",
  ] as const) {
    const provisional = nextTouchKeyboardFocusPhase("idle", "touchstart-provisional");
    assert.equal(nextTouchKeyboardFocusPhase(provisional, event), "idle");
  }
});

test("terminal scroll never creates a provisional keyboard focus", () => {
  const intent = touchKeyboardIntent("terminal", true, false);
  assert.equal(intent, "deferred-tap");
  assert.equal(resolveTouchKeyboardIntent(intent, "scroll"), "none");
  assert.equal(nextTouchKeyboardFocusPhase("idle", "viewport-resize"), "idle");
  assert.equal(nextTouchKeyboardFocusPhase("idle", "redraw"), "idle");
});

test("committed focus ignores a stale post-resize miss but yields to overlay takeover", () => {
  // The Agent composer moved between touchstart and touchend. Geometry is no
  // longer under the release point, but no owner appeared, so focus survives.
  assert.equal(preserveCommittedTouchFocus(true, false, false), true);
  // A newly-opened non-text overlay is an ownership change, not reflow.
  assert.equal(preserveCommittedTouchFocus(true, false, true), false);
  // Text entry that began inside an already-open overlay remains owned.
  assert.equal(preserveCommittedTouchFocus(true, true, true), true);
  assert.equal(preserveCommittedTouchFocus(false, false, false), false);
});