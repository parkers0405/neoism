import { coreCall, NativeEditor, type EditorFrame, type EditorSnapshot } from "../src/native-core";
import { TerminalBuffer } from "../src/terminal/buffer";
import { palettes } from "../src/design/generated";

// Native bridge CPU work only: no daemon, user files, or native window. This
// excludes Solid mutations, GPUI layout/paint, uploads and compositor presentation.
const source = Array.from({ length: 5000 }, (_, i) =>
  `pub fn function_${i}(value: usize) -> String { format!("sample item ${i}: {}", value) }`).join("\n");
const samples = 40, requestedIntervalMs = 4;
async function measure(run: (i: number) => void, paced = false) {
  const times: number[] = [], began = performance.now();
  for (let i = 0; i < samples; i++) {
    const start = performance.now(); run(i); times.push(performance.now() - start);
    if (paced && i + 1 < samples) await Bun.sleep(Math.max(0, requestedIntervalMs - (performance.now() - start)));
  }
  times.sort((a, b) => a - b);
  return { averageMs: times.reduce((a, b) => a + b) / samples, p95Ms: times[Math.floor(samples * .95)],
    samples, elapsedMs: performance.now() - began, ...(paced ? { requestedIntervalMs } : {}) };
}
for (const compact of [false, true]) {
  const editor = compact ? new NativeEditor("scroll-performance.rs", source, true) : undefined;
  const id = editor?.id ?? coreCall<{ id: number }>("editor.new", { path: "scroll-performance.rs", text: source, vim: true }).id;
  const call = (op: string, args: Record<string, unknown> = {}): EditorFrame => editor ? editor.call(op, args) : coreCall(`editor.${op}`, { id, ...args });
  try {
    call("layout", { width: 1200, height: 800, cellWidth: 9, rowHeight: 22 });
    {
      const snapshot = coreCall<EditorSnapshot>("editor.snapshot", { id }); snapshot.scrollY = 0;
      call("restore", { snapshot });
      // Finish any initial caret movement before the explicit idle sample.
      await Bun.sleep(350); call("frame");
      const idle = await measure(() => call("frame"));
      console.log(JSON.stringify({ scope: "native-bridge-only", engine: "editor", phase: "idle-frame", encoding: compact ? "compact" : "raw", ...idle }));
      const before = call("frame"); let after = before, wheelInputs = 0;
      const scrolling = await measure(i => {
        // CodePane::scroll_pixels negates wheel delta. +90 at the top clamps
        // to zero; negative input is required to exercise downward scrolling.
        if (i % 4 === 0) { wheelInputs++; after = call("scroll", { delta: -90 }); }
        else after = call("frame");
      }, true);
      const movedPixels = after.scrollY - before.scrollY;
      if (!(movedPixels > 0) || after.lines[0]?.line === before.lines[0]?.line)
        throw new Error(`Editor scroll benchmark did not move through the document: ${before.scrollY}→${after.scrollY}`);
      const wire = coreCall("editor.frame", { id });
      console.log(JSON.stringify({ scope: "native-bridge-only", engine: "editor", phase: "paced-scroll", encoding: compact ? "compact" : "raw",
        ...scrolling, wheelInputs, startScrollY: before.scrollY, endScrollY: after.scrollY, movedPixels,
        firstVisibleLine: before.lines[0]?.line, lastVisibleLine: after.lines[0]?.line, wireBytes: JSON.stringify(wire).length }));
    }
  } finally { if (editor) editor.dispose(); else coreCall("dispose", { id }); }
}
for (const encoding of ["cells", "runs"] as const) {
  const terminal = new TerminalBuffer(palettes.pastel_dark, 140, 42, encoding);
  try {
    await terminal.write(new TextEncoder().encode(Array.from({ length: 1500 }, (_, i) => `line ${i} colored \x1b[32mvalue\x1b[0m\r\n`).join("")));
    const idle = await measure(() => terminal.frame());
    console.log(JSON.stringify({ scope: "native-bridge-only", engine: "terminal", phase: "idle-frame", encoding, ...idle }));
    const before = terminal.frame(), firstText = before.lines[0]?.map(run => run.text).join("");
    const scrolling = await measure(() => { terminal.scrollPixels(40, 22); terminal.frame(); }, true);
    const after = terminal.frame(), lastText = after.lines[0]?.map(run => run.text).join("");
    if (firstText === lastText) throw new Error("Terminal scroll benchmark did not move through scrollback");
    console.log(JSON.stringify({ scope: "native-bridge-only", engine: "terminal", phase: "paced-scroll", encoding, ...scrolling,
      requestedScrollPixels: samples * 40, firstVisibleText: firstText, lastVisibleText: lastText,
      startSubrowOffsetPixels: before.offset, endSubrowOffsetPixels: after.offset }));
  } finally { terminal.dispose(); }
}
