import { dlopen, FFIType, ptr, CString, type Pointer } from "bun:ffi";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

interface Profile { start(): number; end(name: string, started: number, volume?: number): void }
const quiet: Profile = { start: () => 0, end: () => {} };
let binding: ReturnType<typeof open> | undefined;
function open() {
  const name = process.platform === "win32" ? "neoism_gpuix_core.dll" : process.platform === "darwin" ? "libneoism_gpuix_core.dylib" : "libneoism_gpuix_core.so";
  const paths = [process.env.NEOISM_GPUIX_CORE, join(dirname(process.execPath), "web/native", name), join(dirname(process.execPath), "../Resources/web/native", name), join(dirname(process.execPath), name), fileURLToPath(new URL(`./gpuix/artifacts/${name}`, import.meta.url))].filter((path): path is string => !!path);
  const path = paths.find(existsSync);
  if (!path) throw new Error("Neoism's native engine is not built. Run npm run build:core in neoism-frontend/gpuix.");
  return dlopen(path, {
    neoism_core_call: { args: [FFIType.ptr, FFIType.u64], returns: FFIType.ptr },
    neoism_core_free: { args: [FFIType.ptr], returns: FFIType.void },
  });
}

/** Shared ABI transport only. Both frontends call the same Rust engine; no
 * renderer/window modules or GUI JSX types are loaded by a terminal host. */
export function callNativeCore<T = unknown>(op: string, args: Record<string, unknown> = {}, profile = quiet): T {
  const started = profile.start();
  try {
    binding ??= open();
    const encodeStart = profile.start();
    const bytes = new TextEncoder().encode(JSON.stringify({ op, ...args }));
    profile.end("core.encode", encodeStart, bytes.byteLength);
    const nativeStart = profile.start();
    const result = binding.symbols.neoism_core_call(ptr(bytes), bytes.byteLength) as Pointer;
    profile.end(`native.${op}`, nativeStart);
    if (!result) throw new Error("Native engine returned no response");
    try {
      const decodeStart = profile.start(), json = new CString(result).toString();
      profile.end("core.readResponse", decodeStart, json.length);
      const parseStart = profile.start();
      const value: { ok?: T; error?: string } = JSON.parse(json);
      profile.end("core.parseResponse", parseStart);
      profile.end("core.decode", decodeStart, json.length);
      if (value.error) throw new Error(value.error);
      return value.ok as T;
    } finally { binding.symbols.neoism_core_free(result); }
  } finally { profile.end(`core.${op}`, started); }
}

export interface NativeDocumentFrame {
  lines: Array<{ line: number; continuation: boolean; runs: Array<{ text: string; token: string; selected: boolean; severity?: string | null }> }>;
  cursor: { line: number; col: number; shape: "Block" | "Beam"; visible: boolean };
  mode: "Normal" | "Insert" | "Visual";
  dirty: boolean; revision: number; totalLines: number; pendingKeys: string; pendingEmpty: boolean;
  textX: number; scrollY: number; highlightAnimating?: boolean;
  hostAction?: "save" | "save-close" | "close" | "split-terminal";
  clipboard?: string | null;
  selection: { start: number[]; end: number[] } | null;
}
export class CoreEditor {
  readonly id: number;
  frame: NativeDocumentFrame;
  constructor(path: string, text: string, vim: boolean) {
    this.id = callNativeCore<{ id: number }>("editor.new", { path, text, vim, compact: false }).id;
    this.frame = callNativeCore("editor.frame", { id: this.id });
  }
  call(operation: string, args: Record<string, unknown> = {}) { return this.frame = callNativeCore(`editor.${operation}`, { id: this.id, ...args }); }
  text() { return callNativeCore<{ text: string }>("editor.text", { id: this.id }).text; }
  snapshot() { return callNativeCore<Record<string, unknown>>("editor.snapshot", { id: this.id }); }
  dispose() { callNativeCore("dispose", { id: this.id }); }
}