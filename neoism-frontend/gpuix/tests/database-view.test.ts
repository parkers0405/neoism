import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("native database grid virtualizes, distinguishes DEFAULT/NULL/text, and confirms deletion", async () => {
  const child = Bun.spawn([process.execPath, "run", "tests/support/database-view-check.tsx"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)), env: { ...process.env, NODE_ENV: "test" }, stdout: "pipe", stderr: "pipe",
  });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
  expect(stdout).toContain("Database workbench behavior verified");
});