import { launchNative as launch } from "../tests/support/native-launch";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createAgentDaemon } from "../tests/support/agent-daemon";
const until = async (predicate: () => boolean) => {
  const limit = Date.now() + 10000;
  while (!predicate()) { if (Date.now() > limit) throw new Error("Native agent scenario timed out"); await Bun.sleep(10); }
};
const server = createAgentDaemon({ questionsOnSwitch: true, deferCreations: false });
const directory = mkdtempSync(join(tmpdir(), "neoism-gpuix-agent-"));
const path = join(directory, "capture.png"), bytes = [137, 80, 78, 71, 13, 10, 26, 10];
writeFileSync(path, Uint8Array.from(bytes));
const config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } }));
const app = await launch({ command: process.execPath, args: ["--preload", "./tests/support/path-dialog.ts", "src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config],
  cwd: process.cwd(), env: { ...process.env, XDG_STATE_HOME: join(directory, "state"), NEOISM_TEST_PATHS: JSON.stringify([path]) } });
let stage = "initial question";
try {
  await app.getByTestId("agent-question").waitFor({ timeoutMs: 15000 });
  await app.getByTestId("picker-option:Blue").click();
  stage = "second question";
  await app.getByText("(2/2) When?").waitFor();
  const answer = app.getByTestId("inline-picker-input"); await answer.fill("Tomorrow"); await answer.press("enter");
  stage = "answer retry";
  await app.getByText("Try again").waitFor(); await answer.press("enter");
  await until(() => server.requests.filter((r) => r.tag === "AnswerQuestion").length === 2);
  const replies = server.requests.filter((r) => r.tag === "AnswerQuestion");
  if (JSON.stringify(replies[0].body.answers) !== JSON.stringify([["Blue"], ["Tomorrow"]]) || JSON.stringify(replies[1].body.answers) !== JSON.stringify(replies[0].body.answers)) throw new Error("Question answers were lost during retry");
  const limit = Date.now() + 10000;
  while (await app.getByTestId("agent-question").count()) { if (Date.now() > limit) throw new Error("Answered question did not close"); await Bun.sleep(10); }
  stage = "permission";
  server.push({ ToolUseRequest: { session_id: "one", request_id: "native-permission", tool: "read", title: "Read file" } });
  await app.getByTestId("agent-picker:permission").waitFor(); await app.getByTestId("picker-option:Always").click();
  await until(() => server.requests.some((r) => r.tag === "ApproveTool"));
  if (server.requests.find((r) => r.tag === "ApproveTool")?.body.decision !== "Always") throw new Error("Permission choice did not reach daemon");
  stage = "agent selector";
  await app.getByTestId("select-agent").click(); await app.getByTestId("agent-picker:agent").waitFor();
  await app.getByTestId("picker-option:plan").waitFor(); await app.getByTestId("picker-option:plan").click();
  stage = "model selector";
  await app.getByTestId("select-model").click(); await app.getByTestId("agent-picker:model").waitFor();
  await app.getByTestId("picker-option:openai/advanced").waitFor(); await app.getByTestId("picker-option:openai/advanced").click();
  stage = "reasoning selector";
  await app.getByTestId("select-thinking").click(); await app.getByTestId("agent-picker:thinking").waitFor(); await app.getByTestId("picker-option:high").click();
  stage = "attachments";
  const composer = app.getByTestId("composer"); await composer.fill("Review"); await app.getByTestId("attach-files").click();
  await app.getByText("capture.png").waitFor(); await composer.press("enter");
  await until(() => server.requests.some((r) => r.tag === "SubmitPrompt"));
  const prompt = server.requests.find((r) => r.tag === "SubmitPrompt")!.body;
  const attached = prompt.attachments as Array<{ kind: string; path: string; bytes: number[] }>;
  if (prompt.mode !== "plan" || prompt.model !== "openai/advanced" || prompt.thinking !== "high") throw new Error("Composer selections were not submitted");
  if (attached?.[0]?.kind !== "image/png" || attached[0].path !== "capture.png" || JSON.stringify(attached[0].bytes) !== JSON.stringify(bytes)) throw new Error("Attachment metadata or bytes changed");
  stage = "queued submission";
  await composer.fill("Next"); await composer.press("alt-enter");
  await until(() => server.requests.filter((r) => r.tag === "SubmitPrompt").length >= 2); await Bun.sleep(100);
  const prompts = server.requests.filter((r) => r.tag === "SubmitPrompt");
  if (prompts.length !== 2 || prompts[1].body.delivery !== "queue") throw new Error("Alt+Enter did not submit exactly one queued prompt");
  server.push({ QueueUpdate: { session_id: "one", count: 2 } });
  await composer.fill("/queue clear"); await composer.press("enter"); await until(() => server.requests.some((r) => r.tag === "ClearQueue"));
  console.log("Native agent controls, question retry, permissions, attachments, queued submission, and queue clearing passed against the isolated daemon fixture.");
} catch (error) { console.error(`Native agent check failed at ${stage}; requests: ${server.requests.map((r) => r.tag).join(", ")}`); console.error(JSON.stringify(await app.getByTestId("agent-question").all())); throw error; } finally { await app.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }
