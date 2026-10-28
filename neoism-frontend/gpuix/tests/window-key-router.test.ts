import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("actual Solid Root routes a post-close native window key once to the current App generation without a window", async () => {
  const child = Bun.spawn([process.execPath, "run", "tests/support/window-key-router-check.tsx"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)), stdout: "pipe", stderr: "pipe",
  });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
  expect(JSON.parse(stdout)).toEqual({ noWindow: true, removedTargetUsesWindowHandler: true, descendantAndWindowDispatchOnce: true,
    normalizedWindowEvent: true, replacementGenerationRegistered: true, independentLocalAgentDrafts: true, ordinaryInputUnhandled: true, nativeRootRetained: true, noDaemonWritesOrPrompts: true });
});
