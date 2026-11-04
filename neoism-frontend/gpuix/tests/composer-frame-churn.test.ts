import { expect, test } from "bun:test";
import { fileURLToPath } from "node:url";

test("App animation does not resend unchanged native geometry", async () => {
  const child = Bun.spawn([process.execPath, "run", "tests/support/composer-frame-churn-check.tsx"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)), stdout: "pipe", stderr: "pipe",
  });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
  expect(JSON.parse(stdout)).toMatchObject({ frames: 20, identicalStyles: 0, creates: 0 });
}, 15_000);