import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("repeated font changes keep a complete native root, document state and current-page focus", async () => {
  // Bun's test runner resolves Solid's SSR graph. Run the production preload
  // in a child process to exercise GPUIX's actual reactive host mutation queue.
  const child = Bun.spawn([process.execPath, "run", "tests/support/app-generation-check.tsx"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)), stdout: "pipe", stderr: "pipe",
  });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
  const result = JSON.parse(stdout);
  expect(result).toMatchObject({ swaps: 6, attachments: 1, rootRetained: true, emptyTransactions: 0 });
  expect(result.restoredFocus).toEqual(Array(7).fill("dirty-file"));
});
