import { test } from "node:test";
import assert from "node:assert/strict";

import { resolveDaemonTarget } from "./daemonTarget.ts";

test("query daemon wins and auto-connects", () => {
  const target = resolveDaemonTarget({
    protocol: "http:",
    host: "127.0.0.1:5173",
    port: "5173",
    search: "?daemon=ws://127.0.0.1:43111/session&token=secret",
  });
  assert.equal(target.url, "ws://127.0.0.1:43111/session");
  assert.equal(target.token, "secret");
  assert.equal(target.autoConnect, true);
  assert.equal(target.fromQuery, true);
});

test("same-origin daemon page uses /session", () => {
  const target = resolveDaemonTarget({
    protocol: "http:",
    host: "127.0.0.1:43111",
    port: "43111",
    search: "",
  });
  assert.equal(target.url, "ws://127.0.0.1:43111/session");
  assert.equal(target.autoConnect, true);
  assert.equal(target.fromQuery, false);
});

test("vite without query stays on the injected or head default", () => {
  const head = resolveDaemonTarget({
    protocol: "http:",
    host: "127.0.0.1:5173",
    port: "5173",
    search: "",
  });
  assert.equal(head.url, "ws://127.0.0.1:7878/session");
  assert.equal(head.autoConnect, false);

  const injected = resolveDaemonTarget(
    {
      protocol: "http:",
      host: "127.0.0.1:5173",
      port: "5173",
      search: "",
    },
    "ws://127.0.0.1:43111/session",
  );
  assert.equal(injected.url, "ws://127.0.0.1:43111/session");
  assert.equal(injected.autoConnect, false);
});
