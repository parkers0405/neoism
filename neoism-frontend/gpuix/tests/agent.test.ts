import { afterEach, expect, test } from "bun:test";
import { createAgentDaemon } from "./support/agent-daemon";
import { loadConfig } from "../src/config";
import { DaemonConnection } from "../src/daemon/client";
import { WorkspaceModel } from "../src/model";
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).reverse().forEach((close) => close()));
const until = async (predicate: () => boolean) => {
  const limit = Date.now() + 3000;
  while (!predicate()) { if (Date.now() > limit) throw new Error("Agent fixture did not converge"); await Bun.sleep(5); }
};
async function fixture() {
  const server = createAgentDaemon();
  const { requests, push, workspaces, threads, creations } = server;
  cleanups.push(server.stop);
  const config = { ...loadConfig(["--config", "/nonexistent/test-config"], {}), endpoint: server.endpoint, authToken: undefined };
  const client = new DaemonConnection(config), model = new WorkspaceModel(client, config);
  cleanups.push(() => { model.dispose(); client.stop(); });
  model.start(); client.start();
  await until(() => !model.state.loading && !model.state.catalogsLoading && model.state.providers.length > 0 && model.state.threads.some(thread=>thread.session_id==="one"));
  // Startup now selects the independent Welcome tab. These tests exercise
  // a viewed conversation, so open that thread explicitly before its state.
  model.openThread(threads[0]);
  await until(() => model.state.activeSurfaceId === "agent:one" && model.selection("one").thinking === "medium");
  return { model, requests, push, workspaces, threads, creations };
}

test("catalogs and live provider selections stay scoped to their conversations", async () => {
  const { model, requests, threads, push } = await fixture();
  expect(model.state.providers[0].models?.[0].id).toBe("base"); expect(model.state.agents).toHaveLength(2);
  model.setSelection("model", "openai/changed", "one");
  model.openThread(threads[1]); await until(() => model.selection("two").thinking === "medium");
  expect(model.selection("one").model).toBe("openai/changed"); expect(model.selection("two").model).toBe("other/second");
  model.setSelection("thinking", "high", "two");
  await until(() => requests.some((r) => r.tag === "UpdateSession" && r.body.session_id === "two"));
  expect(requests.find((r) => r.tag === "UpdateSession" && r.body.session_id === "two")?.body).toEqual({ session_id: "two", agent: "plan", model: { providerId: "other", id: "second", variant: "high" } });
  push({ ProviderState: { session_id: "one", agent: "build", model: "openai/canonical", thinking: "low" } });
  await until(() => model.selection("one").model === "openai/canonical"); expect(model.selection("two").thinking).toBe("high");
  push({ ContentDelta: { session_id: "one", message_id: "reason", text: "working", kind: "Reasoning" } });
  await until(() => !!model.state.history.one?.length); expect(model.state.history.one[0].kind).toBe("Reasoning");
  expect(requests.some((r) => r.tag === "ResumeStream" && r.body.session_id === "two")).toBe(true);
});
test("questions preserve session identities and failed answers remain retryable", async () => {
  const { model, requests, push } = await fixture();
  const request = { id: "question", sessionID: "spoofed", questions: [{ question: "Proceed?", options: ["Yes", "No"] }] };
  push({ QuestionAsked: { session_id: "one", request } }); push({ QuestionAsked: { session_id: "two", request } });
  await until(() => model.state.questions.length === 2);
  const question = model.state.questions.find((q) => q.sessionId === "one")!;
  expect(question.request.sessionID).toBe("one"); model.answerQuestion(question, [["Yes"]]);
  await until(() => requests.some((r) => r.tag === "AnswerQuestion"));
  expect(requests.find((r) => r.tag === "AnswerQuestion")?.body).toEqual({ request_id: "question", session_id: "one", answers: [["Yes"]] });
  expect(model.state.questions.find((q) => q.sessionId === "two")?.responding).toBe(false);
  await until(() => !!model.state.questions[0]?.error); expect(model.state.questions.find((q) => q.sessionId === "one")?.responding).toBe(false);
  expect(model.state.questions.find((q) => q.sessionId === "two")?.error).toBeUndefined();
  push({ QuestionRemoved: { session_id: "two", request_id: "question" } });
  await until(() => model.state.questions.length === 1); expect(model.state.questions[0].sessionId).toBe("one");
});
test("a delayed new conversation preserves the submitted workspace, options, and attachment bytes", async () => {
  const { model, requests, creations, workspaces } = await fixture();
  model.setSelection("agent", "plan"); model.setSelection("model", "openai/chosen"); model.setSelection("thinking", "high");
  const submission = model.submitNewPrompt("Look at this", { attachments: [{ kind: "image/png", path: "capture.png", bytes: [1, 2, 3] }], delivery: "queue" });
  await until(() => creations.length === 1);
  await model.activateWorkspace(workspaces[1]); creations.shift()!(); expect(await submission).toBe(true);
  await until(() => requests.some((r) => r.tag === "SubmitPrompt"));
  const prompt = requests.find((r) => r.tag === "SubmitPrompt")!.body;
  expect(model.state.activeWorkspace?.id).toBe("b"); expect(model.state.activeSurfaceId).not.toBe("agent:created-a");
  expect(prompt.session_id).toBe("created-a"); expect(prompt.mode).toBe("plan"); expect(prompt.model).toBe("openai/chosen"); expect(prompt.thinking).toBe("high");
  expect(prompt.attachments).toEqual([{ kind: "image/png", path: "capture.png", bytes: [1, 2, 3] }]); expect(prompt.delivery).toBe("queue");
});
