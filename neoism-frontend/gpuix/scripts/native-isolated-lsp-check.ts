import assert from "node:assert/strict";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { verifiedDisplayBinding } from "../src/display-binding";
import { launchNative } from "../tests/support/native-launch";
import { createLspDaemon } from "../tests/support/lsp-daemon";

// Exercise the existing native editor/LSP presentation fixture without allowing
// the real user's modal-editor setting to change its keystroke semantics.
// The fixture daemon is synthetic; run real_nil_explicit_navigation_when_installed
// separately for real external-server transport/navigation coverage.
const core = resolve(process.env.NEOISM_GPUIX_CORE || "native/target/debug/libneoism_gpuix_core.so");
assert.ok(core.includes("/debug/") && existsSync(core), "A debug bridge is required");
if (process.env.NEOISM_GPUIX_DISPLAY_DEBUG === "1") {
  const display = verifiedDisplayBinding(); assert.equal(display.stamp.profile, "debug");
  console.log(`Native LSP debug display: ${display.binding}`);
}
const directory = mkdtempSync(join(tmpdir(), "neoism-native-lsp-config-"));
const server = createLspDaemon();
let app: Awaited<ReturnType<typeof launchNative>> | undefined;
let stage = "launch";
const until = async (predicate: () => boolean | Promise<boolean>) => { const deadline = Date.now() + 15000; while (!await predicate()) { if (Date.now() > deadline) throw new Error(`Native LSP timed out at ${stage}`); await Bun.sleep(20); } };
try {
  writeFileSync(join(directory, "config.json"), JSON.stringify({ editor: { "vim-mode": true } }));
  app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "lsp", "--config", join(directory, "config.json")], cwd: process.cwd(), env: { ...process.env, ...(process.env.NEOISM_GPUIX_DISPLAY_DEBUG === "1" ? { RUST_MIN_STACK: process.env.RUST_MIN_STACK || "67108864" } : {}), NEOISM_GPUIX_CORE: core, NEOISM_CONFIG_HOME: directory, XDG_STATE_HOME: join(directory, "state") } });
  await app.getByTestId("file:main.ts").waitFor({ timeoutMs: 25000 }); await app.getByTestId("file:main.ts").click();
  const editor = app.getByTestId("code-editor"); await editor.waitFor();
  // A key injection acknowledgement is not a native/core mode-transition
  // acknowledgement. Give the asynchronous editor bridge a frame between keys
  // rather than relying on release-build speed to serialize modal actions.
  const press = async (key: string) => { await editor.press(key); await Bun.sleep(150); };
  stage = "status and diagnostics";
  await until(() => server.requests.some(request => request.tag === "OpenBuffer"));
  const surface_id = server.requests.find(request => request.tag === "OpenBuffer")!.body.surface_id;
  server.push({ LspSnapshot: { surface_id, file_path: server.path, filetype: "typescript", servers: [{ name: "typescript-language-server", binary: "typescript-language-server", filetype: "typescript", state: "connected", source: "managed" }] } });
  await app.getByTestId("status-lsp").waitFor(); await app.getByTestId("status-lsp").click();
  await app.getByTestId("lsp-status-popup").waitFor(); await app.getByTestId("lsp-server:typescript-language-server").waitFor();
  await app.getByTestId("lsp-status-popup").press("escape"); await until(async () => await app!.getByTestId("lsp-status-popup").count() === 0);
  stage = "completion"; await press("i"); await press("ctrl-space"); await app.getByTestId("lsp-option:done").waitFor(); await app.getByTestId("lsp-option:done").click(); await Bun.sleep(150);
  console.log("PASS debug-native LSP completion and status/diagnostics fixture");
  stage = "hover"; await press("escape"); await press("shift-k"); await app.getByTestId("lsp-hover").waitFor({ timeoutMs: 15000 }); await press("escape");
  stage = "code actions"; await press("ctrl-."); await app.getByTestId("lsp-option:Apply fix").waitFor(); await app.getByTestId("lsp-option:Apply fix").click();
  await until(() => server.requests.some(request => request.tag === "ApplyLspCodeActionAt"));
  const action = server.requests.find(request => request.tag === "ApplyLspCodeActionAt")!.body;
  assert.equal(action.action.server_id, "fixture"); assert.equal(action.action.document_revision, "revision");
  stage = "rename"; await press("f2"); await app.getByTestId("lsp-rename-input").waitFor(); await app.getByTestId("lsp-rename-input").fill("renamed"); await app.getByTestId("lsp-rename-input").press("enter");
  await until(() => server.requests.some(request => request.tag === "LspQueryAt" && request.body.action === "rename" && request.body.text === "renamed"));
  stage = "definition"; await press("f12"); await press("ctrl-space");
  await until(() => server.requests.some(request => request.tag === "LspQueryAt" && request.body.action === "completion" && request.body.line === 1 && request.body.character === 8)); await press("escape");
  stage = "references"; await press("shift-f12"); await app.getByTestId("lsp-references").waitFor(); await app.getByTestId("lsp-reference-filter").press("enter");
  stage = "format/save"; await press("alt-shift-f"); await until(() => server.writes.length > 0 && server.requests.some(request => request.tag === "DidSave"));
  assert.ok(server.writes[0].startsWith("FORM"));
  console.log("PASS debug-native LSP hover, code actions, rename, byte-coordinate navigation, format/save and diagnostics fixture");
} catch (error) {
  console.error(`Native LSP failed at ${stage}`, server.requests.filter(request => ["LspQueryAt", "ApplyLspCodeActionAt"].includes(request.tag)));
  throw error;
} finally { await app?.close(); server.stop(); rmSync(directory, { recursive: true, force: true }); }