import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
const server = createAgentDaemon({ deferCreations: false });
const directory = mkdtempSync(join(tmpdir(), "neoism-timeline-"));
const config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }));
const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config], cwd: process.cwd(), env: { ...process.env, XDG_STATE_HOME: join(directory,"state") } });
const until = async (predicate: () => Promise<boolean>) => { const end = Date.now() + 10000; while (!await predicate()) { if (Date.now() > end) throw new Error("Timeline did not converge"); await Bun.sleep(20); } };
let stage = "load";
try {
  await app.getByTestId("conversation").waitFor({ timeoutMs: 15000 });
  server.push({ HistoryChunk: { session_id: "one", messages: [
    ...Array.from({ length: 250 }, (_, i) => ({ id: `prose-${i}`, role: "Assistant", kind: "Assistant", text: `Message ${i}\n\nNative transcript row.` })),
    { id: "author", role: "User", kind: "User", author: "Parker", text: "Review the diff" },
    { id: "code", role: "Assistant", kind: "Assistant", text: "```ts\nconst result = 1;\n```" },
    { id: "edit", role: "Tool", kind: "Tool", tool: "edit", status: "completed", title: "Edit src/lib.rs", detail: JSON.stringify({ neoismToolDetail: "edit", tool: "edit", input: {}, metadata: { acpDiffs: [{ path: "src/lib.rs", oldText: "old\n", newText: "new\n" }] } }) },
  ] } });
  await app.getByTestId("tool:edit").waitFor({ timeoutMs: 10000 });
  stage = "expand"; await app.getByTestId("tool:edit").click();
  await app.getByTestId("tool-diff:edit:0").waitFor({ timeoutMs: 10000 });
  stage = "diff text"; await app.getByText("+new").waitFor();
  stage = "code control"; await app.getByTestId("copy-code:code:0").waitFor();
  const mounted = (await app.getByType("div").all()).filter((node) => node.testId?.startsWith("timeline-row:")).length;
  if (!(mounted > 0 && mounted < 50)) throw new Error(`Timeline did not virtualize its 253 messages (${mounted} mounted)`);
  const timeline = app.getByTestId("conversation");
  stage = "home"; await timeline.press("home");
  await app.getByTestId("message:prose-0").waitFor();
  await app.getByTestId("timeline-follow").waitFor();
  stage = "end"; await timeline.press("end");
  await app.getByTestId("tool:edit").waitFor();
  // Repeated status/text updates preserve the expanded native tool identity.
  stage = "live update"; server.push({ MessageUpdated: { session_id: "one", message: { id: "edit", role: "Tool", kind: "Tool", tool: "edit", status: "completed", title: "Edit src/lib.rs", detail: "updated output" } } });
  await app.getByTestId("copy-code:edit:tool").waitFor();
  console.log(`Native timeline: ${mounted}/253 mounted rows, expansion, canonical diff, code controls, home/end, and stable live row updates passed.`);
} catch (error) { console.error("Timeline failed at", stage); console.error(JSON.stringify((await app.getByType("div").all()).filter((node) => node.testId?.startsWith("timeline-row:")).map((node) => ({id:node.testId,bounds:node.bounds})))); throw error; } finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
