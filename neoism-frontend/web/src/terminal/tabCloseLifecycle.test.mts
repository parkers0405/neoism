import { test } from "node:test";
import assert from "node:assert/strict";

import {
  TabCloseLifecycle,
  activeIndexAfterClose,
  confirmDirtyClose,
} from "./tabCloseLifecycle.ts";

test("close then stale ListEditorSurfaces cannot resurrect an editor or markdown path", () => {
  for (const key of ["file:src/main.rs", "file:README.md"]) {
    const lifecycle = new TabCloseLifecycle();
    lifecycle.setGeneration(4);
    assert.equal(lifecycle.beginClose(key, ["surface", "workspace"]), true);
    assert.equal(lifecycle.blocks(key), true); // stale list still contains it
    lifecycle.acknowledgeMissing("surface", new Set());
    assert.equal(lifecycle.blocks(key), true); // workspace can still rehydrate it
    lifecycle.acknowledgeMissing("workspace", new Set());
    assert.equal(lifecycle.blocks(key), false);
  }
});

test("close during reconnect survives generation change until authorities converge", () => {
  const lifecycle = new TabCloseLifecycle();
  lifecycle.setGeneration(8);
  lifecycle.beginClose("terminal:pty-1", ["pty", "session", "workspace"]);
  lifecycle.setGeneration(9);
  assert.equal(lifecycle.blocks("terminal:pty-1"), true);
  lifecycle.acknowledge("terminal:pty-1", "pty");
  lifecycle.acknowledgeMissing("session", new Set());
  lifecycle.acknowledgeMissing("workspace", new Set());
  assert.equal(lifecycle.blocks("terminal:pty-1"), false);
});

test("duplicate close is idempotent and explicit reopen supersedes the tombstone", () => {
  const lifecycle = new TabCloseLifecycle();
  assert.equal(lifecycle.beginClose("agent:7", ["surface", "workspace"]), true);
  assert.equal(lifecycle.beginClose("agent:7", ["surface", "workspace"]), false);
  lifecycle.explicitOpen("agent:7");
  assert.equal(lifecycle.blocks("agent:7"), false);
});

test("dirty cancel keeps the tab and dirty confirm permits the same close path", () => {
  assert.equal(confirmDirtyClose(true, () => false), false);
  assert.equal(confirmDirtyClose(true, () => true), true);
  assert.equal(confirmDirtyClose(false, () => false), true);
});

test("closing the active tab selects a stable adjacent fallback", () => {
  assert.equal(activeIndexAfterClose(4, 2, 2), 2);
  assert.equal(activeIndexAfterClose(4, 3, 3), 2);
  assert.equal(activeIndexAfterClose(4, 2, 0), 1);
  assert.equal(activeIndexAfterClose(1, 0, 0), 0);
});

test("terminal, agent, and editor tombstones wait on distinct authorities", () => {
  const lifecycle = new TabCloseLifecycle();
  lifecycle.beginClose("terminal:t", ["pty", "session", "workspace"]);
  lifecycle.beginClose("agent:2", ["surface", "workspace"]);
  lifecycle.beginClose("file:a.rs", ["surface", "workspace"]);
  lifecycle.acknowledge("terminal:t", "pty");
  lifecycle.acknowledge("agent:2", "surface");
  lifecycle.acknowledge("file:a.rs", "surface");
  assert.deepEqual(new Set(lifecycle.pendingKeys()), new Set([
    "terminal:t",
    "agent:2",
    "file:a.rs",
  ]));
});