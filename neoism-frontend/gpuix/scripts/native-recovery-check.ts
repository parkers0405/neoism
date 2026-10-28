import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { launchNative } from "../tests/support/native-launch";
import { createAgentDaemon } from "../tests/support/agent-daemon";
const directory = mkdtempSync(join(tmpdir(), "neoism-recovery-window-"));
const config = join(directory, "config.json"), recovery = join(directory, "session.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }));
const server = createAgentDaemon({ deferCreations: false });
server.files.set("recovery.ts", server.files.get("README.md")!);
const launch = () => launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config],
  cwd: process.cwd(), env: { ...process.env, NEOISM_GPUIX_STATE_PATH: recovery } });
const until = async (test: () => boolean | Promise<boolean>) => { const deadline = Date.now() + 10000; while (!await test()) { if (Date.now() > deadline) throw new Error("Recovery verification timed out"); await Bun.sleep(20); } };
let app = await launch(), stage = "open file";
try {
  await app.getByTestId("file:recovery.ts").waitFor({ timeoutMs: 15000 }); await app.getByTestId("file:recovery.ts").click();
  const editor = app.getByTestId("code-editor"); await editor.waitFor();
  stage = "edit and split";
  await editor.press("i"); await editor.press("X"); await editor.press("escape");
  await until(async () => await app.getByTestId("unsaved-buffer").count() > 0);
  await editor.press("ctrl-shift-r"); await until(async () => await app.getByTestId("terminal").count() === 1);
  if (await app.getByTestId("code-editor").count() !== 1) throw new Error("Split replaced or duplicated the code editor instead of creating the original terminal split");
  if (server.fileRequests.some((r) => r.tag === "WriteFile")) throw new Error("Editing or splitting saved the document unexpectedly");
  stage = "checkpoint"; await app.close();
  const snapshot = JSON.parse(readFileSync(recovery, "utf8"));
  if (!Object.values(snapshot.editors).some((e: any) => e.text?.startsWith("X# Neoism"))) throw new Error("Unsaved content missing from checkpoint");
  stage = "restart"; app = await launch();
  await until(async () => await app.getByTestId("code-editor").count() === 1 && await app.getByTestId("terminal").count() === 1);
  await app.getByTestId("code-editor").click();
  await until(async () => await app.getByTestId("unsaved-buffer").count() > 0);
  const panes = await app.getByTestId("pane-layout").getByType("div").all();
  const paneIds = panes.map((p) => p.testId).filter((id): id is string => !!id && /^pane:\d+$/.test(id));
  if (paneIds.length !== 2) throw new Error("Split layout did not survive restart");
  stage = "save recovered document";
  await app.getByTestId("code-editor").press("ctrl-s");
  await until(() => server.fileRequests.some((r) => r.tag === "WriteFile"));
  if (!server.files.get("recovery.ts")?.startsWith("X# Neoism")) throw new Error("Saving lost recovered content");
  await until(async () => await app.getByTestId("unsaved-buffer").count() === 0);
  console.log("Native editor edits, split views, unsaved recovery after window restart, and explicit save passed against the isolated daemon fixture.");
} catch (error) { console.error(`Native recovery check failed at ${stage}`); throw error; }
finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
