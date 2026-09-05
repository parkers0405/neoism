import assert from "node:assert/strict";
import test from "node:test";
import {
  ExactSyntheticEchoFilter,
  normalizeTerminalDirectory,
  resolveTerminalDirectoryTarget,
  terminalDirectoryPlan,
} from "./terminalDirectory.ts";

const literal = (path: string) => new TextEncoder().encode(`literal:${path}`);
const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

test("relative cd sequences use the optimistic cwd from the captured terminal", () => {
  const first = terminalDirectoryPlan("/a/b", "..", false, "zsh", literal);
  assert.equal(first.optimisticCwd, "/a");
  const second = terminalDirectoryPlan(first.optimisticCwd!, "child", false, "zsh", literal);
  assert.equal(second.optimisticCwd, "/a/child");
});

test("bare HOME, tilde children, OLDPWD, and quoted-space operands stay safe", () => {
  assert.equal(decode(terminalDirectoryPlan("/cwd", "", false, "zsh", literal).payload), "cd\n");
  assert.equal(decode(terminalDirectoryPlan("/cwd", "-", false, "zsh", literal).payload), "cd -\n");
  assert.equal(
    decode(terminalDirectoryPlan("/cwd", "~/x/y", false, "zsh", literal).payload),
    `cd -- "$HOME"/'x/y'\n`,
  );
  const spaces = terminalDirectoryPlan("/cwd", "a b;$(bad)", false, "zsh", literal);
  assert.equal(spaces.optimisticCwd, "/cwd/a b;$(bad)");
  assert.equal(decode(spaces.payload), "literal:/cwd/a b;$(bad)");
});

test("absolute completion selection uses its separate literal target", () => {
  const plan = terminalDirectoryPlan("/ignored", "/selected/path", true, "zsh", literal);
  assert.equal(plan.optimisticCwd, "/selected/path");
  assert.equal(decode(plan.payload), "literal:/selected/path");
  assert.equal(normalizeTerminalDirectory("/a/b", "../../x"), "/x");
});

test("synthetic palette echo suppression is exact and fragmentation-safe", () => {
  const filter = new ExactSyntheticEchoFilter();
  filter.expect(new TextEncoder().encode("cd -- '/tmp/a b'\n"));
  assert.equal(decode(filter.filter(new TextEncoder().encode("cd -- '/tmp"))), "");
  assert.equal(
    decode(filter.filter(new TextEncoder().encode("/a b'\r\nPROMPT"))),
    "PROMPT",
  );
  filter.expect(new TextEncoder().encode("cd /expected\n"));
  assert.equal(decode(filter.filter(new TextEncoder().encode("user input\r\n"))), "user input\r\n");
  filter.expect(new TextEncoder().encode("cd /tmp\n"));
  assert.equal(decode(filter.filter(new TextEncoder().encode("cd /tmp\nprompt"))), "prompt");
});

test("selected Home row keeps symbolic HOME semantics", () => {
  const plan = terminalDirectoryPlan("/cwd", "~", true, "zsh", literal);
  assert.equal(decode(plan.payload), "cd\n");
  assert.equal(plan.optimisticCwd, null);
});

test("global directory target uses focused association then MRU and requests creation when empty", () => {
  const live = new Set(["focused", "active", "recent", "tab"]);
  const resolve = (associated: string | null, active: string | null, recent: string | null) =>
    resolveTerminalDirectoryTarget({
      associated,
      active,
      recent,
      tabSessions: ["tab"],
      isLive: (id) => live.has(id),
    });
  assert.equal(resolve("focused", "active", "recent"), "focused");
  live.delete("focused");
  assert.equal(resolve("focused", "active", "recent"), "active");
  live.delete("active");
  assert.equal(resolve("focused", "active", "recent"), "recent");
  live.clear();
  assert.equal(resolve("focused", "active", "recent"), null);
});