// Native bridge only: excludes uploads, GPUI draws and physical presentation.
// Exercise both sides of the former eight-job preparation-cache limit.
import { coreCall } from "../src/native-core";

type Frame = { preparing: boolean };
for (const fences of [1, 8, 9, 12]) {
  const { id } = coreCall<{ id: number }>("agenttimeline.new", {
    sessionId: "preparation-benchmark", canvasPaint: true, chunkedPaint: true,
  });
  try {
    const text = Array.from({ length: fences }, (_, block) => "```rust\n" +
      Array.from({ length: 1000 }, (_, line) => `let value_${block}_${line} = ${line}; // block ${block}, line ${line}`).join("\n") +
      "\n```\n\n").join("");
    coreCall("agenttimeline.event", { id, message: { HistoryChunk: {
      session_id: "preparation-benchmark",
      messages: [{ id: "answer", role: "Assistant", kind: "Assistant", text }], next_cursor: null,
    } } });
    const started = performance.now();
    let frame = coreCall<Frame>("agenttimeline.layout", { id, width: 1100, height: 900 });
    const initialMs = performance.now() - started;
    const samples: number[] = [];
    for (let i = 0; i < 300 && frame.preparing; i++) {
      const before = performance.now();
      frame = coreCall<Frame>("agenttimeline.frame", { id });
      samples.push(performance.now() - before);
    }
    samples.sort((a, b) => a - b);
    console.log(JSON.stringify({ scope: "native-bridge-only", fences, initialMs,
      preparationFrames: samples.length, complete: !frame.preparing,
      p50Ms: samples[Math.floor(samples.length * .5)] ?? 0,
      p95Ms: samples[Math.floor(samples.length * .95)] ?? 0, maxMs: samples.at(-1) ?? 0,
    }));
    if (frame.preparing) process.exitCode = 1;
  } finally {
    coreCall("agenttimeline.dispose", { id });
  }
}