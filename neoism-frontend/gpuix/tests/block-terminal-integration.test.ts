import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("enabled block terminal uses the native engine through submission, capture, remount and direct input", async () => {
  const child = Bun.spawn([process.execPath, "run", "tests/support/block-terminal-integration-check.tsx"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)), stdout: "pipe", stderr: "pipe",
  });
  const [status, stdout, stderr] = await Promise.all([
    child.exited, new Response(child.stdout).text(), new Response(child.stderr).text(),
  ]);
  expect({ status, stderr, output: status === 0 ? undefined : stdout }).toEqual({ status: 0, stderr: "", output: undefined });
  expect(JSON.parse(stdout)).toEqual({
    noWindow: true, promptComposer: true, exactSubmission: true, ansiCapture: true,
    retainedAcrossMounts: true, replayDeduplicated: true, reuseWithoutExecution: true,
    alternateScreen: true, safePickers: true, nativeCaretGhost: true, inputPositions: true,
  });
}, 15000);