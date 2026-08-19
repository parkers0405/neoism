// Node-side loader for the REAL wasm bundle, so the input-policy
// adapter tests (`RemotePresenceStore`, `imePolicy`, `touchPolicy`)
// can exercise the actual shared-Rust policy instead of a scripted
// fake. Lives in `src/presence/` beside its primary consumer.
//
// Resolution order:
//   1. `NEOISM_WASM_PKG_DIR` — point at any wasm-pack output dir
//      (e.g. a scratch `--dev` build) without touching `src/wasm`.
//   2. The checked-in `src/wasm` bundle.
//
// The bundle is instantiated with `initSync` on raw bytes (no fetch,
// no DOM). When the resolved bundle predates the input-policy exports
// the loader still returns the module — callers skip per-export via
// the optional members of `WasmInputPolicyModule`. Returns `null`
// only when no bundle can be loaded at all.

import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import type { WasmInputPolicyModule } from "../terminal/createTerminal.ts";

interface WasmGlueModule extends WasmInputPolicyModule {
  initSync(args: { module: BufferSource }): unknown;
}

let cached: WasmInputPolicyModule | null | undefined;

export async function loadWasmInputPolicyModule(): Promise<WasmInputPolicyModule | null> {
  if (cached !== undefined) return cached;
  const dir = process.env.NEOISM_WASM_PKG_DIR
    ? path.resolve(process.env.NEOISM_WASM_PKG_DIR)
    : path.resolve(
        path.dirname(fileURLToPath(import.meta.url)),
        "../wasm",
      );
  try {
    const glue = (await import(
      pathToFileURL(path.join(dir, "neoism_terminal_wasm.js")).href
    )) as WasmGlueModule;
    const bytes = await readFile(path.join(dir, "neoism_terminal_wasm_bg.wasm"));
    glue.initSync({ module: bytes });
    cached = glue;
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn(
      `[test] wasm bundle not loadable from ${dir} (${String(err)}); ` +
        "wasm-backed policy tests will be skipped",
    );
    cached = null;
  }
  return cached;
}

/** Human-readable skip reason for tests that need one export. */
export function skipReason(
  mod: WasmInputPolicyModule | null,
  exportName: keyof WasmInputPolicyModule,
): string | false {
  if (!mod) return "wasm bundle not loadable in this environment";
  if (!mod[exportName]) {
    return (
      `wasm bundle predates the \`${String(exportName)}\` export — rebuild ` +
      "with `npm run build:wasm` (or point NEOISM_WASM_PKG_DIR at a fresh build)"
    );
  }
  return false;
}
