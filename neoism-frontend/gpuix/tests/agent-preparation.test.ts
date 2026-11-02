import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";

// Native bridge only: drive the same cached layout and repeated paint frames as
// the retained host, without starting a browser/UI build or changing its budget.
test("a twelve-fence transcript finishes intrinsic preparation and stays finished", () => {
  const sessionId = "preparation-regression";
  const { id } = coreCall<{ id: number }>("agenttimeline.new", {
    sessionId, canvasPaint: true, chunkedPaint: true,
  });
  try {
    const text = Array.from({ length: 12 }, (_, block) => "```rust\n" +
      Array.from({ length: 1000 }, (_, line) => `let value_${block}_${line} = ${line}; // block ${block}, line ${line}`).join("\n") +
      "\n```\n\n").join("");
    coreCall("agenttimeline.event", { id, message: { HistoryChunk: {
      session_id: sessionId,
      messages: [{ id: "answer", role: "Assistant", kind: "Assistant", text }], next_cursor: null,
    } } });
    let frame = coreCall<{ preparing: boolean }>("agenttimeline.layout", { id, width: 1100, height: 900 });
    // Preparation has a 1.5ms wall-clock budget per frame, not a fixed line
    // count. Cold macOS font measurement needs more frames than Linux; 300
    // frames can expire while it is still making legitimate forward progress.
    // Keep a finite deadline so cache eviction/restart loops still fail, and
    // a work bound allowing one line of progress per frame plus finalization.
    const deadline = performance.now() + 30_000;
    for (let i = 0; i < 12_050 && frame.preparing && performance.now() < deadline; i++) {
      frame = coreCall<{ preparing: boolean }>("agenttimeline.frame", { id });
    }
    expect(frame.preparing).toBe(false);
    for (let i = 0; i < 3; i++) {
      expect(coreCall<{ preparing: boolean }>("agenttimeline.frame", { id }).preparing).toBe(false);
    }
  } finally {
    coreCall("agenttimeline.dispose", { id });
  }
}, 35000);