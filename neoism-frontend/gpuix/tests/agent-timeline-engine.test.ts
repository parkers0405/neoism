import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";

// The original agent timeline engine (shared Rust pane + recorded renderer),
// driven exactly like AgentTimelineView drives it.
type Command = { kind: string; text?: string; x?: number; y?: number; width?: number; size?: number };
type Frame = { commands: Command[]; animating: boolean; copy?: string | null; link?: string | null };
const history = [
  { id: "u1", role: "User", kind: "User", text: "Show me the code" },
  { id: "a1", role: "Assistant", kind: "Assistant", text: "Here it is:\n\n```ts\nconst answer = 42;\n```\n\nAnd a [link](https://example.com) too." },
  { id: "t1", role: "Assistant", kind: "Tool", tool: "bash", status: "completed", title: "Run tests", detail: "all green" },
];
const engine = () => {
  const { id } = coreCall<{ id: number }>("agenttimeline.new", { sessionId: "s1", theme: "", scale: 1, fontFamily: "Cascadia Code NF", fontWeight: 400 });
  coreCall("agenttimeline.event", { id, message: { HistoryChunk: { session_id: "s1", messages: history, next_cursor: null } } });
  return id;
};
const texts = (frame: Frame) => frame.commands.filter((c) => c.kind === "text");
// Reveal/scroll animations run on wall time: step real frames.
const run = (id: number, ms: number) => { let frame = coreCall<Frame>("agenttimeline.frame", { id }); const end = Date.now() + ms; while (Date.now() < end) { Bun.sleepSync(8); frame = coreCall<Frame>("agenttimeline.frame", { id }); } return frame; };
const settle = (id: number) => { let frame = coreCall<Frame>("agenttimeline.layout", { id, width: 900, height: 700 }); for (let i = 0; i < 300 && frame.animating; i++) { Bun.sleepSync(8); frame = coreCall<Frame>("agenttimeline.frame", { id }); } return frame; };

test("renders the original timeline from daemon history", () => {
  const id = engine(), frame = settle(id);
  const painted = texts(frame).map((c) => c.text).join(" ");
  expect(painted).toContain("Show me the code");
  expect(painted).toContain("answer");
  // Completed tool rows in reopened history stay collapsed, as on desktop.
  coreCall("agenttimeline.dispose", { id });
});

test("code-card Copy click copies the code, links report their target", () => {
  const id = engine(), frame = settle(id);
  const copy = texts(frame).find((c) => c.text?.trim().toLowerCase() === "copy");
  expect(copy).toBeDefined();
  const click = (x: number, y: number) => { const down = coreCall<Frame>("agenttimeline.pointer", { id, kind: "down", x, y }); coreCall("agenttimeline.pointer", { id, kind: "up", x, y }); return down; };
  const copied = click(copy!.x! + 4, copy!.y! + (copy!.size || 12) / 2);
  expect(copied.copy).toContain("const answer = 42;");
  const link = texts(frame).find((c) => c.text === "link");
  expect(link).toBeDefined();
  expect(click(link!.x! + 4, link!.y! + (link!.size || 14) / 2).link).toBe("https://example.com");
  coreCall("agenttimeline.dispose", { id });
});

test("drag selection copies text on release, like the desktop", () => {
  const id = engine(), frame = settle(id);
  const line = texts(frame).find((c) => c.text === "Here")!;
  const y = line.y! + (line.size || 14) / 2;
  coreCall("agenttimeline.pointer", { id, kind: "down", x: line.x! + 1, y });
  coreCall("agenttimeline.pointer", { id, kind: "move", x: line.x! + 200, y, deferFrame: true });
  const up = coreCall<Frame>("agenttimeline.pointer", { id, kind: "up", x: line.x! + 200, y });
  expect(up.copy).toContain("Here it is");
  expect(coreCall<{ text: string | null }>("agenttimeline.copy", { id }).text).toBe(up.copy ?? null);
  coreCall("agenttimeline.dispose", { id });
});

test("streaming updates and scroll go through the shared pane", () => {
  const id = engine(); settle(id);
  coreCall("agenttimeline.event", { id, message: { MessageUpdated: { session_id: "s1", message: { id: "a2", role: "Assistant", kind: "Assistant", text: "Streaming tail token" } } } });
  let frame = run(id, 900);
  expect(texts(frame).map((c) => c.text).join("")).toContain("Streaming tail token");
  // Another conversation's events never reach this pane.
  coreCall("agenttimeline.event", { id, message: { MessageUpdated: { session_id: "other", message: { id: "x", role: "Assistant", kind: "Assistant", text: "Foreign" } } } });
  coreCall("agenttimeline.event", { id, message: { SessionIdle: { session_id: "s1" } } });
  frame = run(id, 300);
  expect(texts(frame).map((c) => c.text).join("")).not.toContain("Foreign");
  const before = JSON.stringify(frame.commands.map((c) => c.y));
  coreCall("agenttimeline.scroll", { id, deltaY: 120, precise: true });
  expect(JSON.stringify(run(id, 400).commands.map((c) => c.y))).not.toBe(before);
  coreCall("agenttimeline.dispose", { id });
}, 20000);
