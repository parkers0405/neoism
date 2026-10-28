// Mock the OS path dialog at the platform boundary. Button handling, file reads,
// MIME classification, attachment tokens, and daemon submission remain real.
import type { GpuixRenderer } from "@gpuix/native";
const key = Symbol.for("neoism/gpuix/native-host");
const globals = globalThis as typeof globalThis & { [key]?: { renderer: GpuixRenderer } };
const timer = setInterval(() => {
  const host = globals[key];
  if (!host) return;
  host.renderer.promptForPaths = async () => JSON.parse(process.env.NEOISM_TEST_PATHS || "[]");
  clearInterval(timer);
}, 5);
