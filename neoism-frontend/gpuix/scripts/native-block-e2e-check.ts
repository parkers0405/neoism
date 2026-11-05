import assert from "node:assert/strict";
import { Effect, Schema } from "effect";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { deflateSync } from "node:zlib";
import { loadConfig } from "../src/config";
import { verifiedDisplayBinding } from "../src/display-binding";
import { DaemonConnection } from "../src/daemon/client";
import { TreeSchema, expectVariant, isRecord, variant } from "../src/daemon/protocol";
import { launchNative } from "../tests/support/native-launch";
import type { TreeNode } from "@gpuix/solid/automation";

// Run from gpuix with its Bun runtime. Both daemon and window use debug code;
// every profile, recovery file, shell, and workspace belongs to this invocation.
// Rich regression (only AFTER the parent builds debug daemon/core/display):
// NEOISM_BLOCK_E2E_RICH=1 NEOISM_BLOCK_E2E_KEEP=1 NEOISM_GPUIX_DISPLAY_DEBUG=1 NEOISM_GPUIX_CORE="$PWD/native/target/debug/libneoism_gpuix_core.so" NEOISM_BLOCK_E2E_DAEMON="$PWD/../../target/debug/neoism-workspace-daemon" bun scripts/native-block-e2e-check.ts
// Requires the owned Hyprland foreground, grim, Bash, git, npm and a usable
// debug display addon stamp. No fallback/unsupported provider is a coverage pass.
const homeFixture = process.env.NEOISM_BLOCK_E2E_HOME === "1";
const richFixture = process.env.NEOISM_BLOCK_E2E_RICH === "1";
const flagOnly = process.env.NEOISM_BLOCK_E2E_FLAG_ONLY === "1";
assert.ok(!flagOnly || richFixture, "FLAG_ONLY requires the real rich native fixture");
assert.ok(!(homeFixture && richFixture), "Run rich regression separately from the long Home scenario");
if (homeFixture) {
  assert.ok(["src/ui/HomeTerminalView.tsx", "src/terminal/HomeTerminalView.tsx"].some(path => existsSync(path)), "Home native coverage is prepared; resume only after HomeTerminalView is available");
  assert.ok(readFileSync("src/ui/App.tsx", "utf8").includes("HomeTerminalView") && readFileSync("src/model.ts", "utf8").includes("ensureHomeTerminal"), "Home App/model integration must be present before launching a private window");
}
const core = resolve(process.env.NEOISM_GPUIX_CORE || "native/target/debug/libneoism_gpuix_core.so");
assert.ok(existsSync(core) && (core.includes("/debug/") || richFixture && !!process.env.NEOISM_GPUIX_CORE), "NEOISM_GPUIX_CORE must name an existing debug bridge or an explicitly supplied rich-check artifact");
assert.equal(process.env.NEOISM_GPUIX_DISPLAY_DEBUG, "1", "Set NEOISM_GPUIX_DISPLAY_DEBUG=1 to verify the debug display addon too");
const display = verifiedDisplayBinding(); assert.equal(display.stamp.profile, "debug");
const binary = resolve(process.env.NEOISM_BLOCK_E2E_DAEMON || "../../target/debug/neoism-workspace-daemon");
const directory = mkdtempSync(join(tmpdir(), "neoism-block-native-e2e-"));
const home = join(directory, "home"), workspace = join(home, "workspace"), recovery = join(directory, "session.json"), configPath = join(directory, "config.json");
for (const path of [home, workspace]) mkdirSync(path, { recursive: true });
mkdirSync(join(directory, "cache", "fontconfig"), { recursive: true });
writeFileSync(join(workspace, "choice_a.txt"), "short completion fixture\n");
writeFileSync(join(workspace, "choice_longer.txt"), "long completion fixture\n");
writeFileSync(join(workspace, "clicked.txt"), `PRIVATE_CLICK_FILE_${process.pid}\n`);
mkdirSync(join(workspace, "many"));
for (let index = 0; index < 24; index++) writeFileSync(join(workspace, "many", `suggestion_${String(index).padStart(2, "0")}.txt`), "private scroll fixture\n");
const backgroundFixture = homeFixture || process.env.NEOISM_BLOCK_E2E_BACKGROUND === "1" ? join(directory, "private-background.png") : undefined;
if (backgroundFixture) {
  const chunk = (type: string, data: Buffer) => {
    const name = Buffer.from(type), size = Buffer.alloc(4), checksum = Buffer.alloc(4);
    size.writeUInt32BE(data.length); checksum.writeUInt32BE(Bun.hash.crc32(Buffer.concat([name, data])));
    return Buffer.concat([size, name, data, checksum]);
  };
  const header = Buffer.alloc(13); header.writeUInt32BE(2, 0); header.writeUInt32BE(2, 4); header[8] = 8; header[9] = 2;
  const pixels = Buffer.from([0, 30, 90, 150, 100, 35, 130, 0, 100, 35, 130, 30, 90, 150]);
  writeFileSync(backgroundFixture, Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(pixels)), chunk("IEND", Buffer.alloc(0))]));
}
writeFileSync(join(home, ".bashrc"), "PS1='PRIVATE_E2E> '\nHISTFILE=/dev/null\n");
writeFileSync(join(home, ".zshrc"), "PROMPT='PRIVATE_E2E> '\nHISTFILE=/dev/null\n");
const observationsPath = join(directory, "native-observations.jsonl");
const richBunfig = join(directory, "bunfig.toml");
const alternateWorkspace = join(home, "alternate-workspace");
if (richFixture) {
  assert.equal(process.env.NEOISM_BLOCK_E2E_SHELL || "/bin/bash", "/bin/bash", "Rich session metadata verification requires real Bash");
  mkdirSync(join(home, "bin")); mkdirSync(alternateWorkspace);
  writeFileSync(join(home, "bin", "nativepathprobe"), "#!/bin/bash\nprintf 'PATH_EXECUTED\\n' >> \"$HOME/submits\"\nprintf 'PATH_EXECUTED\\n'\n", { mode: 0o700 });
  writeFileSync(join(home, ".bashrc"), "PS1='PRIVATE_E2E> '\nHISTFILE=/dev/null\nexport PATH=\"$HOME/bin:$PATH\"\nalias nativealiasprobe='nativepathprobe'\nnativefunctionprobe() { nativepathprobe; }\n");
  writeFileSync(join(workspace, "quoted space's é.txt"), "quoted fixture\n");
  writeFileSync(join(workspace, "package.json"), JSON.stringify({ name: "private-native-fixture", scripts: { "native-package-probe": "printf 'PACKAGE_EXECUTED\\n'" } }));
  for (let index = 0; index < 12; index++) writeFileSync(join(alternateWorkspace, `animation_${String(index).padStart(2, "0")}.txt`), "private animation fixture\n");
  // A valid unborn repository and branch refs, made solely with private fixture
  // FS writes; git commands below still run only through the actual app shell.
  mkdirSync(join(workspace, ".git", "refs", "heads"), { recursive: true });
  mkdirSync(join(workspace, ".git", "objects"));
  writeFileSync(join(workspace, ".git", "HEAD"), "ref: refs/heads/main\n");
  writeFileSync(join(workspace, ".git", "config"), "[core]\n\trepositoryformatversion = 0\n\tbare = false\n[user]\n\tname = Native Fixture\n\temail = native@example.invalid\n");
  writeFileSync(observationsPath, "");
  writeFileSync(richBunfig, "preload = " + JSON.stringify([resolve("scripts/native-rich-observe.ts"), resolve("scripts/display-binding-preload.ts"), resolve("node_modules/@gpuix/solid/dist/preload.js")]) + "\n");
}
const shell = process.env.NEOISM_BLOCK_E2E_SHELL || "/bin/bash", nonce = `block-e2e-${process.pid}`;
assert.ok(existsSync(shell), `Requested shell is not installed: ${shell}`);
const position = process.env.NEOISM_BLOCK_E2E_POSITION === "top" || homeFixture ? "top" : "bottom";
const agent = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) { return new URL(request.url).pathname === "/v2/hosting/associate" ? Response.json({ workspaceId: nonce }) : new Response("Unused", { status: 404 }); } });
// Unoptimized nested native containers need a larger OS-level thread stack.
// Supply it before child startup; setting Bun's process.env after Rust starts
// cannot reliably change the cached native thread-stack default.
const privateEnv = { ...process.env, RUST_MIN_STACK: process.env.RUST_MIN_STACK || "67108864", HOME: home, ZDOTDIR: home, SHELL: shell, XDG_CONFIG_HOME: join(directory, "config"), XDG_DATA_HOME: join(directory, "data"), XDG_CACHE_HOME: join(directory, "cache"), XDG_STATE_HOME: join(directory, "state"), NEOISM_CONFIG_DIR: join(directory, "config"), NEOISM_CONFIG_HOME: join(directory, "config"), NEOISM_WORKSPACE_REGISTRY: join(directory, "registry.json"), NEOISM_DAEMON_DATA_DIR: join(directory, "daemon-data"), NEOISM_AGENT_SERVER: `http://127.0.0.1:${agent.port}`, NEOISM_FAST_READINESS_NONCE: nonce, NEOISM_DAEMON_TOKEN: nonce, NEOISM_REQUIRE_AUTH: "0", NEOISM_GPUIX_CORE: core, NEOISM_GPUIX_STATE_PATH: recovery };
const daemon = Bun.spawn([binary, "--neoism-internal-fast-workspace-daemon", "--addr", "127.0.0.1:0", "--no-unix-socket", "--ephemeral", "--state-dir", join(directory, "daemon-state"), "--temporary-parent-pid", String(process.pid), "--workspace", workspace], { cwd: workspace, detached: true, env: privateEnv, stdout: "pipe", stderr: Bun.file(join(directory, "daemon.log")) });
let app: Awaited<ReturnType<typeof launchNative>> | undefined, client: DaemonConnection | undefined, sessionId: string | undefined, output = "", stage = "daemon readiness";
let focusFailure: unknown, focusMonitor: ReturnType<typeof setInterval> | undefined;
const until = async (test: () => boolean | Promise<boolean>) => { const end = Date.now() + 20000; while (true) { if (focusFailure) throw focusFailure; if (await test()) return; if (Date.now() > end) throw new Error(`Timeout at ${stage}; PTY tail: ${JSON.stringify(output.slice(-1800))}`); await Bun.sleep(20); } };
try {
  const reader = daemon.stdout.getReader(); let line = "";
  while (!line.includes("\n")) { const next = await Promise.race([reader.read(), Bun.sleep(20000).then(() => { throw new Error("Readiness timeout"); })]); if (next.done) throw new Error("Daemon exited before readiness"); line += new TextDecoder().decode(next.value); }
  const ready = JSON.parse(line.split("\n")[0]!); assert.equal(ready.nonce, nonce);
  writeFileSync(configPath, JSON.stringify({ terminal: { "block-ui": true, "input-position": position, cursor: { blinking: false }, ...(richFixture ? { shell: { program: shell } } : {}) }, editor: { "vim-mode": false }, ...(backgroundFixture ? { ui: { window: { "background-image": { path: backgroundFixture, opacity: .5, blur: 0 } } } } : {}) }));
  const args = ["--config", configPath, "--daemon", `ws://${ready.address}/session`, ...(richFixture ? [] : ["--reduced-motion"])];
  const config = loadConfig(args, privateEnv); assert.equal(config.terminalBlockUi, true);
  client = new DaemonConnection(config);
  client.subscribe(event => { const entry = variant(event.message); if (event.family === "Pty" && entry?.[0] === "PtyOutput" && isRecord(entry[1]) && entry[1].session_id === sessionId && Array.isArray(entry[1].bytes)) output += new TextDecoder().decode(Uint8Array.from(entry[1].bytes)); });
  stage = "connect"; client.start(); await until(() => client!.state.phase === "connected");
  const request = (family: Parameters<DaemonConnection["request"]>[0], message: unknown) => Effect.runPromise(client!.request(family, message as never));
  const tree = Schema.decodeUnknownSync(TreeSchema)(expectVariant(await request("Workspace", "RequestHostWorkspaceTree"), "HostWorkspaceTree"));
  const host = tree.workspaces.find(item => item.root_dir === workspace); assert.ok(host);
  const alternateId = `${nonce}-animation`;
  if (richFixture) expectVariant(await request("Workspace", { CreateWorkspace: { workspace_id: alternateId, title: "Private animation", root_dir: alternateWorkspace } }), "HostWorkspaceUpserted");
  if (!homeFixture && !richFixture) {
    const created = expectVariant(await request("Pty", { CreatePty: { cwd: workspace, cols: 100, rows: 28, shell, presentation: "blocks" } }), "PtyCreated");
    assert.ok(isRecord(created) && typeof created.session_id === "string"); sessionId = created.session_id as string;
  }
  const welcomeFixture = homeFixture || richFixture;
  const id = welcomeFixture ? `welcome:${host.id}` : `terminal:${sessionId}`;
  writeFileSync(recovery, JSON.stringify({ version: 1, activeWorkspaceId: host.id, activeSurfaceId: id, surfaces: [{ id, workspaceId: host.id, kind: welcomeFixture ? "welcome" : "terminal", title: welcomeFixture ? "Home" : "Terminal", ...(sessionId ? { sessionId } : {}) }], editors: {}, providerStates: {}, drafts: {}, composerDrafts: {}, ui: { openWorkspaceIds: richFixture ? [host.id, alternateId] : [host.id] } }));
  stage = "native window";
  console.log(`Starting private native fixture: ${directory}`);
  let rejectNativeExit: (reason: Error) => void = () => {};
  const nativeExit = new Promise<never>((_, reject) => { rejectNativeExit = reject; });
  app = await Promise.race([launchNative({ command: process.execPath, args: ["scripts/native-block-e2e-child.ts", ...(richFixture ? [`--config=${richBunfig}`] : []), "src/main.tsx", ...args, "--workspace", host.id], cwd: process.cwd(), env: { ...privateEnv, NEOISM_RICH_OBSERVATIONS: observationsPath }, onStderr: chunk => { appendFileSync(join(directory, "native.log"), chunk); process.stderr.write(chunk); if (/Native e2e child exited: code=/.test(chunk)) rejectNativeExit(new Error(chunk.trim())); } }), nativeExit]);
  const call = app.call.bind(app);
  let checkInputFocus: () => Promise<void> = async () => {};
  app.call = (async (method: Parameters<typeof call>[0], params: Parameters<typeof call>[1]) => {
    await checkInputFocus();
    let timer: ReturnType<typeof setTimeout>;
    return Promise.race([call(method, params), new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`Native automation stopped responding during ${stage}`)), 20000); })]).finally(() => clearTimeout(timer));
  }) as typeof app.call;
  const compositor = Bun.which("hyprctl");
  assert.ok(compositor, "Native input verification requires compositor ownership/focus checks");
  const compositorJson = async (operation: string) => {
    const process = Bun.spawn([compositor!, operation, "-j"], { stdout: "pipe", stderr: "pipe" });
    const text = await new Response(process.stdout).text(); assert.equal(await process.exited, 0);
    return JSON.parse(text);
  };
  let focusLost = false;
  let ownedWindow: { pid: number; address: string } | undefined;
  const ensureWindowActive = async () => {
    assert.ok(!focusLost, "FOCUS INTERFERENCE: test permanently stopped; never refocus");
    if (!compositor) return;
    if (!ownedWindow) await until(async () => {
      ownedWindow = (await compositorJson("clients") as { pid: number; address: string }[]).find(window => {
        try { return readFileSync(`/proc/${window.pid}/cmdline`, "utf8").split("\0").includes(configPath); } catch { return false; }
      });
      return !!ownedWindow;
    });
    assert.ok(![546639, 3879902, 1110281, 2039878, 967430].includes(ownedWindow!.pid), "Protected user window must never be controlled");
    const active = await compositorJson("activewindow");
    if (active.pid !== ownedWindow!.pid || active.address !== ownedWindow!.address) focusLost = true;
    assert.ok(!focusLost, "FOCUS INTERFERENCE: aborting without refocusing any window");
  };
  await ensureWindowActive();
  checkInputFocus = ensureWindowActive;
  if (richFixture) {
    let checking = false;
    focusMonitor = setInterval(() => {
      if (checking || focusFailure) return;
      checking = true;
      void ensureWindowActive().catch(error => { focusFailure = error; }).finally(() => { checking = false; });
    }, 75);
  }
  console.log("PASS owned native window compositor focus confirmed");
  const captureOwnWindow = async (name: string) => {
    if (!compositor || !Bun.which("grim")) throw new Error("Own-window screenshot requires the compositor query and grim");
    await ensureWindowActive();
    const window = (await compositorJson("clients") as { pid: number; at: number[]; size: number[] }[]).find(candidate => {
      try { return readFileSync(`/proc/${candidate.pid}/cmdline`, "utf8").split("\0").includes(configPath); } catch { return false; }
    });
    assert.ok(window); const path = join(directory, name);
    const shot = Bun.spawn(["grim", "-g", `${window.at[0]},${window.at[1]} ${window.size[0]}x${window.size[1]}`, path], { stdout: "ignore", stderr: "inherit" }); assert.equal(await shot.exited, 0);
    return path;
  };
  stage = "select recovered terminal";
  const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]);
  if (richFixture) {
    const command = app.getByTestId("terminal-command"), menu = app.getByTestId("terminal-suggestions");
    await command.waitFor({ timeoutMs: 25000 });
    // Current App recovery may activate its Home surface before the seeded
    // standalone Terminal. Observe the exact shell receiving native UI input.
    await until(() => {
      const saved = JSON.parse(readFileSync(recovery, "utf8"));
      const active = saved.surfaces?.find((surface: any) => surface.id === saved.activeSurfaceId);
      if (active?.kind !== "welcome" || !active.sessionId) return false;
      assert.equal(active.workspaceId, host.id); sessionId = active.sessionId; return true;
    });
    await request("Pty", { AttachPty: { session_id: sessionId } });
    const observations = (): { at: number; kind: string; data: any }[] => readFileSync(observationsPath, "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
    const draft = () => observations().filter(row => row.kind === "composer").at(-1)?.data.draft;
    const idle = async () => { await ensureWindowActive(); await until(async () => await command.count() === 1 && await app!.getByTestId("terminal").count() === 0); await Bun.sleep(150); };
    const strip = () => output.replace(/\x1b\][^\x07]*\x07/g, "").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").replace(/\r/g, "");
    const run = async (text: string, marker: string) => { await idle(); output = ""; await command.fill(text); await command.press("enter"); await until(() => strip().split(/\r?\n/).includes(marker)); await idle(); };
    const ledgerCount = () => existsSync(join(home, "submits")) ? readFileSync(join(home, "submits"), "utf8").trim().split("\n").length : 0;
    stage = "private Bash metadata and git/package fixture";
    await run("git add package.json && git commit -qm fixture && git branch native-branch-probe && printf 'GIT_READY\\n'", "GIT_READY");
    await run("type nativepathprobe nativealiasprobe nativefunctionprobe >/dev/null && command -v npm >/dev/null && printf 'METADATA_READY\\n'", "METADATA_READY");
    const cases = [
      { name: "PATH command", query: "nativepathpr", contains: "nativepathprobe", marker: "PATH_EXECUTED", ledger: true },
      { name: "session alias", query: "nativealiaspr", contains: "nativealiasprobe", marker: "PATH_EXECUTED", ledger: true },
      { name: "session function", query: "nativefunctionpr", contains: "nativefunctionprobe", marker: "PATH_EXECUTED", ledger: true },
      { name: "UTF8 safely quoted file", query: "printf '%s\\n' ./quoted", contains: "quoted", marker: "./quoted space's é.txt" },
      { name: "git branch", query: "git checkout native-branch-pr", contains: "native-branch-probe", marker: undefined },
      { name: "npm package script", query: "npm run native-package-pr", contains: "native-package-probe", marker: "PACKAGE_EXECUTED" },
      { name: "option flag", query: "git --ver", contains: "--version", marker: undefined },
    ];
    const results: unknown[] = [];
    const failures: { stage: string; error: string }[] = [];
    for (const test of cases.filter(test => !flagOnly || test.name === "option flag")) {
      stage = `rich native ${test.name}`;
      await idle(); const cursor = Buffer.byteLength(test.query, "utf8");
      const response = expectVariant(await request("Pty", { CompleteTerminal: { session_id: sessionId, line: test.query, cursor } }), "TerminalCompletions");
      appendFileSync(join(directory, "rich-wire.jsonl"), JSON.stringify({ test: test.name, response }) + "\n");
      assert.ok(isRecord(response)); assert.equal(response.session_id, sessionId); assert.equal(response.line, test.query); assert.equal(response.cursor, cursor);
      assert.ok(typeof response.provider === "string" && response.provider.length > 0, "Backend must identify its real completion provider");
      assert.ok(Array.isArray(response.entries) && response.entries.length, `${test.name}: backend must provide native entries, not UI fallback`);
      const entries = response.entries as { value: string; label: string; detail: string; kind: string }[];
      for (const entry of entries) { assert.equal(typeof entry.value, "string"); assert.equal(typeof entry.label, "string"); assert.equal(typeof entry.detail, "string"); assert.equal(typeof entry.kind, "string"); }
      assert.ok(entries.some(entry => entry.value.includes(test.contains)), `${test.name}: missing expected full command`);
      await command.fill(test.query); await command.press("end"); output = ""; const beforeLedger = ledgerCount();
      await command.press("tab"); await app.getByTestId("terminal-suggestion:0").waitFor();
      await until(() => entries.some(entry => entry.value === draft()));
      assert.ok(draft().includes(test.contains), "First preview must be the requested unambiguous completion");
      await Bun.sleep(200); assert.equal(ledgerCount(), beforeLedger); assert.ok(!/\x1b\]133;C(?:;|\x07)/.test(output), "Tab preview submitted a command");
      await command.press("escape"); await until(async () => await menu.count() === 0); assert.equal(draft(), test.query, "Escape must restore exact original query");
      await command.press("tab"); await app.getByTestId("terminal-suggestion:0").waitFor(); await until(() => entries.some(entry => entry.value === draft()));
      const preview = draft(); await captureOwnWindow(`rich-${test.name.replace(/[^a-z0-9]/gi, "-")}.png`);
      output = ""; await command.press("enter");
      if (test.marker) await until(() => strip().split(/\r?\n/).includes(test.marker!));
      else await until(() => /\x1b\]133;D/.test(output));
      await idle();
      assert.equal((output.match(/\x1b\]133;C(?:;|\x07)/g) || []).length, 1, `${test.name}: ONE Enter must submit exactly once`);
      if (test.ledger) assert.equal(ledgerCount(), beforeLedger + 1, "Exactly one real shell invocation");
      if (test.name === "git branch") await run("printf '%s\\n' \"$(git branch --show-current)\"", "native-branch-probe");
      let passed = true;
      if (test.name === "option flag") {
        try { assert.match(strip(), /git version \d/); }
        catch (error) {
          // Keep the real execution assertion red, but finish independent tree
          // checks so one production defect cannot hide unrelated regressions.
          passed = false; failures.push({ stage, error: String(error) });
          writeFileSync(join(directory, "rich-option-failure.pty"), output);
          console.error(`FAIL native option flag: ${JSON.stringify({ preview, output: strip() })}`);
        }
      }
      results.push({ name: test.name, response, preview, passed });
      writeFileSync(join(directory, "rich-results.json"), JSON.stringify({ results, failures }, null, 2));
      if (passed) console.log(`PASS native ${test.name}: backend full-command entries, Tab preview only, Escape restores, one Enter submits once`);
    }
    if (flagOnly) {
      stage = "inspect corrected secondary Git branch completion";
      const line = "git checkout native-branch-pr";
      const branchReply = expectVariant(await request("Pty", { CompleteTerminal: { session_id: sessionId, line, cursor: Buffer.byteLength(line) } }), "TerminalCompletions");
      assert.ok(isRecord(branchReply) && Array.isArray(branchReply.entries));
      const installed = branchReply.entries.filter((entry: any) => entry.detail === "Installed Bash completion function");
      assert.ok(branchReply.entries.length > 0, "Branch completion must still provide a real candidate (identical native/Bash entries may deduplicate)");
      for (const entry of branchReply.entries as any[]) {
        assert.equal(entry.label, "native-branch-probe", "Secondary branch label must not contain the appended delimiter");
        assert.equal(entry.value, "git checkout native-branch-probe ", "Secondary branch command keeps its delimiter outside the token");
      }
      writeFileSync(join(directory, "rich-results.json"), JSON.stringify({ flagOnly: true, results, failures, branchReply, bridge: core, debugDisplay: display.binding }, null, 2));
      console.log(`PASS corrected Git branch candidate values (protocol inspection; ${installed.length} distinct installed-Bash secondary entries, identical entries may deduplicate)`);
      assert.deepEqual(failures, [], "Native flag regression must execute git --version successfully");
      console.log(`PASS narrow native flag regression; artifacts ${directory}`);
    } else {
    stage = "UTF8 byte cursor protocol";
    const unicodeLine = "printf '%s\\n' é ./quoted";
    const unicodeReply = expectVariant(await request("Pty", { CompleteTerminal: { session_id: sessionId, line: unicodeLine, cursor: Buffer.byteLength(unicodeLine) } }), "TerminalCompletions");
    assert.ok(isRecord(unicodeReply)); assert.equal(unicodeReply.cursor, Buffer.byteLength(unicodeLine)); assert.ok(Array.isArray(unicodeReply.entries) && unicodeReply.entries.length > 0);
    // Exercise the UI's own UTF8 cursor path too, not only the direct protocol.
    await command.fill(unicodeLine); await command.press("end"); await command.press("tab"); await app.getByTestId("terminal-suggestion:0").waitFor(); await until(() => (unicodeReply.entries as any[]).some(entry => entry.value === draft())); await command.press("escape"); await until(async () => await menu.count() === 0); assert.equal(draft(), unicodeLine);
    stage = "no-match native pulse without popup or execution";
    const start = observations().length, noMatch = `native_missing_${process.pid}_zzzz`;
    const missingReply = expectVariant(await request("Pty", { CompleteTerminal: { session_id: sessionId, line: noMatch, cursor: Buffer.byteLength(noMatch) } }), "TerminalCompletions");
    assert.ok(isRecord(missingReply)); assert.equal(missingReply.session_id, sessionId); assert.equal(missingReply.line, noMatch); assert.equal(missingReply.cursor, Buffer.byteLength(noMatch)); assert.deepEqual(missingReply.entries, []); assert.ok(missingReply.provider, "No-match must be a real backend result, not provider failure");
    await command.fill(noMatch); output = ""; await command.press("tab");
    await until(() => observations().slice(start).some(row => row.kind === "composer" && row.data.missOpacity > 0));
    assert.equal(await menu.count(), 0, "No-match feedback must not leave a popup");
    await captureOwnWindow("rich-no-match.png"); await Bun.sleep(400);
    assert.equal(observations().filter(row => row.kind === "composer").at(-1)!.data.missOpacity, 0, "No-match pulse must settle"); assert.equal(draft(), noMatch); assert.ok(!/\x1b\]133;C(?:;|\x07)/.test(output));
    await command.fill("");
    console.log("PASS UTF8 cursor and native no-match pulse/no popup/no execution");

    stage = "real workspace tab tree root animation";
    if (!await app.getByTestId("file-tree").count()) await command.press("alt-e");
    await app.getByTestId("file-tree").waitFor(); await Bun.sleep(800);
    const switchStart = observations().length;
    const workspaceTree = await app.call("getTree", {});
    const alternateTab = flatten(workspaceTree.tree ? [workspaceTree.tree] : []).find(node => node.testId?.startsWith("workspace:") && node.testId.includes(alternateId));
    assert.ok(alternateTab?.testId, "Find the qualified private workspace tab");
    await app.getByTestId(alternateTab.testId).click();
    const boundsSamples: unknown[] = [];
    for (let index = 0; index < 24; index++) {
      const { tree } = await app.call("getTree", {});
      boundsSamples.push(flatten(tree ? [tree] : []).filter(node => /^file:animation_/.test(node.testId || "")).map(node => ({ testId: node.testId, bounds: node.bounds })));
      if (index === 1) await captureOwnWindow("rich-tree-revealing.png");
      await Bun.sleep(12);
    }
    await app.getByTestId("file:animation_00.txt").waitFor(); await Bun.sleep(700);
    const reveal = observations().slice(switchStart).filter(row => row.kind === "glyph" && /^animation_/.test(row.data.name));
    const rowPaints = observations().slice(switchStart).filter(row => row.kind === "row" && /^animation_/.test(row.data.name));
    writeFileSync(join(directory, "rich-tree-samples.json"), JSON.stringify({ reveal, rowPaints, boundsSamples }, null, 2));
    assert.ok(rowPaints.length >= 3 && rowPaints.every(row => row.data.opacity === 1), "Only glyph ink fades; actual row/background paint remains opaque");
    assert.ok(reveal.some(row => row.data.opacity > 0 && row.data.opacity < 1), "Actual App workspace change must paint intermediate glyph fade");
    assert.ok(reveal.some(row => row.data.offsetX > 0), "Actual App glyph ink must slide from positive X");
    for (const sample of reveal) assert.ok(Math.abs(sample.data.offsetX - (1 - sample.data.opacity) * 12 * sample.data.scale) < .01, "Legacy +12px scaled offset and glyph opacity must be coupled");
    for (const name of ["animation_00.txt", "animation_01.txt", "animation_02.txt"]) {
      const samples = reveal.filter(row => row.data.name === name);
      assert.ok(samples.length >= 3 && samples.at(-1)!.data.opacity === 1 && samples.at(-1)!.data.offsetX === 0, `${name}: row must animate and settle`);
      // Invert easeOutCubic to recover the elapsed linear 320ms timeline.
      const middle = samples.filter(row => row.data.opacity > .05 && row.data.opacity < .95);
      assert.ok(middle.length >= 2, `${name}: insufficient native frames to verify timing`);
      const starts = middle.map(row => row.at - 320 * (1 - Math.cbrt(1 - row.data.opacity)));
      assert.ok(Math.max(...starts) - Math.min(...starts) < 45, "Row paint follows legacy 320ms cubic ease-out (frame scheduling tolerance 45ms)");
    }
    const onset = (name: string) => { const row = reveal.find(row => row.data.name === name && row.data.opacity > .05 && row.data.opacity < .95)!; return row.at - 320 * (1 - Math.cbrt(1 - row.data.opacity)); };
    assert.ok(Math.abs(onset("animation_02.txt") - onset("animation_00.txt") - 24) < 18, "Visible rows stagger 12ms, rather than fading the whole panel");
    const stable = new Map<string, unknown>();
    for (const rows of boundsSamples as { testId: string; bounds: unknown }[][]) for (const row of rows) { if (stable.has(row.testId)) assert.deepEqual(row.bounds, stable.get(row.testId), "Root reveal must not move row hit targets"); else stable.set(row.testId, row.bounds); }
    assert.ok(stable.size >= 3, "Must sample actual native row hit targets during reveal");
    await captureOwnWindow("rich-tree-settled.png");
    const refreshStart = observations().length;
    // Same authoritative root/upsert forces a real host tree refresh; no root identity change.
    expectVariant(await request("Workspace", { SetWorkspaceRoot: { workspace_id: alternateId, root_dir: alternateWorkspace } }), "HostWorkspaceUpserted");
    await Bun.sleep(800); await app.call("getTree", {});
    await app.getByTestId("file:animation_00.txt").waitFor();
    assert.ok(observations().slice(refreshStart).filter(row => row.kind === "glyph" && /^animation_/.test(row.data.name)).every(row => row.data.opacity === 1 && row.data.offsetX === 0), "Same-root refresh must not replay reveal");
    stage = "cached workspace connection return reveals again";
    const originalTab = flatten(workspaceTree.tree ? [workspaceTree.tree] : []).find(node => node.testId?.startsWith("workspace:") && node.testId.includes(host.id));
    assert.ok(originalTab?.testId);
    await app.getByTestId(originalTab.testId).click(); await app.getByTestId("file:choice_a.txt").waitFor(); await Bun.sleep(650);
    const returnStart = observations().length;
    await app.getByTestId(alternateTab.testId).click(); await app.getByTestId("file:animation_00.txt").waitFor();
    await captureOwnWindow("rich-tree-cached-return.png"); await Bun.sleep(800);
    const cachedReveal = observations().slice(returnStart).filter(row => row.kind === "glyph" && /^animation_/.test(row.data.name));
    writeFileSync(join(directory, "rich-tree-cached-samples.json"), JSON.stringify(cachedReveal, null, 2));
    assert.ok(cachedReveal.some(row => row.data.opacity > 0 && row.data.opacity < 1 && row.data.offsetX > 0), "Cached connection activation must reveal, not get consumed while hidden");
    for (const name of ["animation_00.txt", "animation_01.txt", "animation_02.txt"]) assert.equal(cachedReveal.filter(row => row.data.name === name).at(-1)?.data.opacity, 1, "Cached return settles");
    await app.getByTestId("file:animation_00.txt").click(); await app.getByTestId("code-editor").waitFor();
    writeFileSync(join(directory, "rich-results.json"), JSON.stringify({ results, failures, unicodeReply, missingReply, reveal, cachedReveal, rowPaints, boundsSamples, sameRootRefresh: true, hitTargetOpensFile: true, bridge: core, debugDisplay: display.binding }, null, 2));
    console.log(`PASS real workspace switch: glyph-only reveal/cubic 320ms/12ms stagger/+12px offset, stable clickable row targets, no same-root replay, cached connection return reveals again; artifacts ${directory}`);
    stage = "aggregate rich regression failures";
    assert.deepEqual(failures, [], "Native rich regression has production failures; see rich-results.json (never count unsupported/broken behavior as a pass)");
    }
  } else if (homeFixture) {
    stage = "Home starts with original splash and bottom composer";
    const command = app.getByTestId("terminal-command"), header = app.getByTestId("terminal-history-header"), history = app.getByTestId("terminal-block-history");
    await header.waitFor({ timeoutMs: 25000 }); await command.waitFor();
    await app.getByTestId("welcome-page").waitFor(); await app.getByTestId("splash-wordmark").waitFor();
    const nodes = async () => { const { tree } = await app!.call("getTree", {}); return flatten(tree ? [tree] : []); };
    const tabNodes = async () => (await nodes()).filter(node => /^pane-tab:\d+$/.test(node.testId || ""));
    const paneCount = async () => (await nodes()).filter(node => /^pane:\d+$/.test(node.testId || "")).length;
    const saved = (): { surfaces: { id: string; kind: string; sessionId?: string }[] } => JSON.parse(readFileSync(recovery, "utf8"));
    const attachSurface = async (surfaceId: string) => {
      await until(() => !!saved().surfaces.find(surface => surface.id === surfaceId)?.sessionId);
      sessionId = saved().surfaces.find(surface => surface.id === surfaceId)!.sessionId!;
      await request("Pty", { AttachPty: { session_id: sessionId } }); output = "";
      return sessionId;
    };
    const homeSession = await attachSurface(id);
    const homeTab = (await tabNodes()).find(node => flatten([node]).some(child => child.text === "Home"))!; assert.ok(homeTab);
    const selectTab = async (testId: string) => {
      const rect = await app!.getByTestId(testId).bounds(); await app!.call("click", { x: rect.x + 16, y: rect.y + rect.height / 2 }); await Bun.sleep(200);
    };
    const idleHome = async () => { await ensureWindowActive(); await until(async () => await command.count() === 1 && await app!.getByTestId("terminal").count() === 0); await Bun.sleep(150); };
    const runHome = async (text: string, marker: string) => {
      await idleHome(); output = ""; await command.fill(text); await command.press("enter"); await until(() => output.includes(marker)); await idleHome();
    };
    const historyBox = await history.bounds(), initialHeader = await header.bounds();
    assert.ok((await app.getByTestId("terminal-block-input").bounds()).y >= historyBox.y + historyBox.height - 1, "Home forces bottom composer even when terminal.input-position=top");
    const startShot = await captureOwnWindow("home-start.png");
    writeFileSync(join(directory, "home-start-tree.json"), JSON.stringify(await app.call("getTree", {})));
    console.log("PASS full App Home owns a new shell, original welcome-page/wordmark/actions and forced bottom composer", startShot);

    stage = "Home output translates splash and original buttons remain clickable";
    const action = app.getByTestId("splash-action:open-command-palette"); await action.waitFor();
    const initialAction = await action.bounds();
    await runHome(`export PRIVATE_HOME_SHELL=HOME_${process.pid}; printf 'HOME_%s\\n' STARTED`, "HOME_STARTED");
    await until(async () => (await header.bounds()).y < initialHeader.y - 5);
    const translatedAction = await action.bounds(); assert.ok(translatedAction.y < initialAction.y - 5);
    assert.ok(translatedAction.y >= historyBox.y && translatedAction.y + translatedAction.height <= historyBox.y + historyBox.height, "Translated original splash action must remain within visible scrollback");
    await action.click(); await app.getByTestId("command-palette").waitFor();
    await app.getByTestId("command-palette").press("escape"); await until(async () => await app!.getByTestId("command-palette").count() === 0);
    console.log("PASS real command moves original splash upward; translated palette action opens and closes non-destructively");

    stage = "Home scrollback reveals original header and clear restores it";
    await runHome("for i in {1..60}; do printf 'HOME_ROW_%s\\n' \"$i\"; done", "HOME_ROW_60");
    await until(async () => (await header.bounds()).y + (await header.bounds()).height < historyBox.y + 1);
    await history.wheel(0, 10000); await Bun.sleep(250);
    await until(async () => (await header.bounds()).y >= historyBox.y - 1);
    await captureOwnWindow("home-scrollback.png");
    await runHome("clear", "]133;D");
    await until(async () => Math.abs((await header.bounds()).y - initialHeader.y) < 2);
    const clearShot = await captureOwnWindow("home-clear.png");
    // Compare a margin outside SplashView's paint bounds, at compositor scale.
    const sample = async (shot: string) => {
      const identify = Bun.spawn(["magick", "identify", "-format", "%w", shot], { stdout: "pipe" });
      const scale = Number(await new Response(identify.stdout).text()) / (await app!.getByTestId("app-shell").bounds()).width; assert.equal(await identify.exited, 0);
      const read = Bun.spawn(["magick", shot, "-format", `%[hex:p{${Math.floor((historyBox.x + 2) * scale)},${Math.floor((historyBox.y + 2) * scale)}}]`, "info:"], { stdout: "pipe" });
      const value = await new Response(read.stdout).text(); assert.equal(await read.exited, 0); return value;
    };
    const background = await sample(startShot); assert.notEqual(background.slice(0, 6), "000000", "Configured background image must remain visible behind Home"); assert.equal(await sample(clearShot), background);
    console.log("PASS output hides header, wheel reveals it, clear restores initial position and configured background pixel");

    stage = "Home composer text must not activate splash split chord";
    const beforePanes = await paneCount();
    await command.click(); await command.fill("echo"); await command.press("end"); await command.press("space"); await command.press("h");
    await Bun.sleep(250); assert.equal(await paneCount(), beforePanes); await command.fill("");
    console.log("PASS Space,H typed inside Home composer does not split");

    stage = "Alt+T is a separate terminal and tab switches preserve Home shell";
    const beforeTabs = (await tabNodes()).length;
    await command.press("alt-t"); await until(async () => (await tabNodes()).length > beforeTabs);
    const terminalTab = (await tabNodes()).find(node => flatten([node]).some(child => child.text === "Terminal"))!; assert.ok(terminalTab);
    await until(() => saved().surfaces.some(surface => surface.kind === "terminal" && !!surface.sessionId));
    const terminalSurface = saved().surfaces.find(surface => surface.kind === "terminal" && !!surface.sessionId)!;
    const terminalSession = await attachSurface(terminalSurface.id); assert.notEqual(terminalSession, homeSession);
    assert.equal(await header.count(), 0, "Standalone terminal must not show Home splash");
    await runHome("printf 'SEPARATE_%s\\n' \"${PRIVATE_HOME_SHELL:-EMPTY}\"", "SEPARATE_EMPTY");
    await selectTab(homeTab.testId!); await header.waitFor(); await attachSurface(id);
    assert.equal(sessionId, homeSession); await runHome("printf 'SURVIVED_%s\\n' \"$PRIVATE_HOME_SHELL\"", `SURVIVED_HOME_${process.pid}`);
    console.log("PASS Alt+T has independent session/no splash; returning Home retains session and exported shell variable");

    stage = "Each Home surface owns an independent shell";
    const beforeHomes = saved().surfaces.filter(surface => surface.kind === "welcome").map(surface => surface.id);
    const newHomeButton = (await nodes()).find(node => /^pane-new-welcome:/.test(node.testId || ""))!; assert.ok(newHomeButton);
    await app.getByTestId(newHomeButton.testId!).click(); await header.waitFor();
    await until(() => saved().surfaces.some(surface => surface.kind === "welcome" && !beforeHomes.includes(surface.id) && !!surface.sessionId));
    const secondHome = saved().surfaces.find(surface => surface.kind === "welcome" && !beforeHomes.includes(surface.id))!;
    assert.notEqual(await attachSurface(secondHome.id), homeSession);
    await runHome("printf 'OTHER_HOME_%s\\n' \"${PRIVATE_HOME_SHELL:-EMPTY}\"", "OTHER_HOME_EMPTY");
    await selectTab(homeTab.testId!); await attachSurface(id); await header.waitFor();
    await runHome("clear", "]133;D");
    console.log("PASS second Home has its own independent shell; original Home remains usable");

    stage = "Space,H on original splash splits a separate terminal";
    const beforeSplit = await paneCount();
    await app.getByTestId("welcome-page").click(); await app.getByTestId("welcome-page").press("space h");
    await until(async () => await paneCount() === beforeSplit + 1);
    await captureOwnWindow("home-split.png");
    writeFileSync(join(directory, "home-final-tree.json"), JSON.stringify(await app.call("getTree", {})));
    console.log("PASS Space,H on original splash creates a split terminal");
    console.log(JSON.stringify({ home: true, debugBridge: core, debugDisplay: display.binding, shell, configuredPosition: position, actualPosition: "bottom", nativeWindow: true, isolatedDaemon: true, homeSession, terminalSession, screenshots: directory }));
  } else {
  let tab: TreeNode | undefined;
  await until(async () => { const { tree } = await app!.call("getTree", {}); tab = flatten(tree ? [tree] : []).find(node => /^pane-tab:\d+$/.test(node.testId || "") && flatten([node]).some(child => child.text === "Terminal")); return !!tab; });
  writeFileSync(join(directory, "initial-native-tree.json"), JSON.stringify(await app.call("getTree", {})));
  const tabBounds = await app.getByTestId(tab!.testId!).bounds();
  console.log(`Selecting ${tab!.testId} at ${JSON.stringify(tabBounds)}`);
  await app.call("click", { x: tabBounds.x + 16, y: tabBounds.y + tabBounds.height / 2 });
  stage = "block composer ready";
  const command = app.getByTestId("terminal-command");
  await command.waitFor({ timeoutMs: 25000 });
  const cursor = app.getByTestId("terminal-composer-cursor");
  await command.click(); await cursor.waitFor();
  assert.ok((await cursor.bounds()).width > 4, "Native measured composer cursor must be a thick block, not a beam");
  console.log("PASS actual shared composer cursor has native bounds width > 4");
  const composerBounds = await app.getByTestId("terminal-block-input").bounds(), blockBounds = await app.getByTestId("terminal-block-history").bounds();
  assert.ok(position === "top" ? composerBounds.y < blockBounds.y : composerBounds.y > blockBounds.y, "Configured input position was not applied");
  const idle = async () => { await ensureWindowActive(); await until(async () => await command.count() === 1 && await app!.getByTestId("terminal").count() === 0); await Bun.sleep(100); };
  const run = async (text: string, marker: string) => { await idle(); output = ""; await command.fill(text); await command.press("enter"); await until(() => output.includes(marker)); await idle(); };
  const safe = async (operation: () => Promise<unknown>) => {
    await ensureWindowActive(); const start = output.length; await operation(); await Bun.sleep(300);
    const delta = output.slice(start);
    // A compositor resize can redraw an empty integrated prompt asynchronously.
    // Reject command starts and echoed/printed text, not OSC prompt repaint.
    assert.ok(!/\x1b\]133;C(?:;|\x07)/.test(delta), "Suggestion acceptance executed a command");
    assert.equal(delta.replace(/\x1b\][^\x07]*\x07/g, "").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").trim(), "", "Suggestion acceptance sent text to the shell");
  };
  const plainOutput = () => output.replace(/\x1b\][^\x07]*\x07/g, "").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
  stage = "Enter executes and archives ANSI output";
  const marker = `PRIVATE_EXECUTED_${process.pid}`, text = `printf '\\033[32mPRIVATE_%s\\033[0m\\n' EXECUTED_${process.pid}`;
  await run(text, marker);
  await app.getByTestId("terminal-block-ink").waitFor();
  // A block output is painted by canvas, not DOM text. Retain the native tree
  // as evidence and inspect the archived output through its Copy output action.
  const historyBounds = await app.getByTestId("terminal-block-history").bounds();
  for (let y = 10; y < historyBounds.height; y += 20) {
    await app.mouse.move({ x: historyBounds.x + 25, y: historyBounds.y + y });
    if (await app.getByTestId("block-more").count() || await app.getByTestId("block-copy-output").count()) break;
  }
  await app.getByTestId("block-filter").waitFor();
  assert.equal(await app.getByTestId("block-attach").count(), 0);
  assert.equal(await app.getByTestId("block-save").count(), 0);
  console.log("PASS native block toolbar retains Filter/More without Attach/Save");
  if (process.env.NEOISM_BLOCK_E2E_SCROLL === "1") console.log("Own toolbar screenshot", await captureOwnWindow("toolbar.png"));
  if (await app.getByTestId("block-more").count()) await app.getByTestId("block-more").click();
  await app.getByTestId("block-copy-output").waitFor();
  await app.getByTestId("block-copy-output").click();
  if (Bun.which("wl-paste")) await until(async () => { const read = Bun.spawn(["wl-paste", "--no-newline"], { stdout: "pipe" }); const copied = await new Response(read.stdout).text(); return await read.exited === 0 && copied.includes(marker); });
  const acceptedMarker = `ACCEPTED_${process.pid}`;
  await run(`echo ${acceptedMarker}`, acceptedMarker);
  console.log("PASS real Enter execution, archived ANSI output copy, configured " + position + " flow");
  stage = "Up menu Enter inserts without executing";
  await command.fill(""); await command.press("up"); await app.getByTestId("terminal-suggestions").waitFor(); await safe(() => command.press("enter"));
  await until(async () => await app!.getByTestId("terminal-suggestions").count() === 0);
  output = ""; await command.press("enter"); await until(() => output.includes(acceptedMarker)); await idle();
  console.log("PASS Up menu Enter inserts only; next Enter executes the inserted command");
  if (process.env.NEOISM_BLOCK_E2E_SCROLL === "1") {
    const inspectMenu = async (kind: "history" | "completion") => {
      stage = `${kind} overflowing native menu scroll`;
      const menu = app!.getByTestId("terminal-suggestions"), viewport = app!.getByTestId("terminal-suggestions-viewport");
      await menu.waitFor(); await app!.getByTestId("terminal-suggestions-scrollbar").waitFor(); await Bun.sleep(150);
      const box = await menu.bounds(), input = await app!.getByTestId("terminal-block-input").bounds();
      assert.ok(position === "top" ? box.y >= input.y + input.height - 1 : box.y + box.height <= input.y + 1, "Menu must open on the configured side of the input");
      const snapshot = async (name: string) => {
        const result = await app!.call("getTree", {}); writeFileSync(join(directory, `${kind}-${name}-tree.json`), JSON.stringify(result));
        const nodes = flatten(result.tree ? [result.tree] : []);
        const rows = nodes.filter(node => /^terminal-suggestion:\d+$/.test(node.testId || ""));
        assert.ok(rows.length > 1 && rows.length <= 9, "Viewport must virtualize the overflowing suggestions");
        assert.ok(rows.every(node => flatten([node]).some(child => child.type === "svg")), "Each native suggestion must contain semantic SVG artwork");
        return { rows, nodes };
      };
      const before = await snapshot("before"), thumb = await app!.getByTestId("terminal-suggestions-scrollbar").bounds();
      const selected = before.rows.find(node => node.testId === "terminal-suggestion:0")!;
      const initialShot = await captureOwnWindow(`${kind}-before.png`);
      console.log(`Own ${kind} initial screenshot`, initialShot);
      const identify = Bun.spawn(["magick", "identify", "-format", "%w", initialShot], { stdout: "pipe", stderr: "inherit" });
      const imageWidth = Number(await new Response(identify.stdout).text()); assert.equal(await identify.exited, 0);
      const scale = imageWidth / before.nodes.find(node => node.testId === "app-shell")!.bounds!.width;
      // Native tree omits style/ARIA. Assert rendered colors from the compositor
      // PNG instead of treating absent serialized props as a production failure.
      const pixel = async (x: number, y: number, expected: string) => {
        const read = Bun.spawn(["magick", initialShot, "-format", `%[hex:p{${Math.floor(x * scale)},${Math.floor(y * scale)}}]`, "info:"], { stdout: "pipe", stderr: "inherit" });
        const actual = (await new Response(read.stdout).text()).slice(0, 6).toLowerCase(); assert.equal(await read.exited, 0);
        assert.equal(actual, expected.slice(1, 7).toLowerCase(), `Composited pixel ${x},${y}`);
      };
      await pixel(box.x + 2, box.y + 12, config.theme.bg);
      await pixel(selected.bounds!.x + 5, selected.bounds!.y + 15, config.theme.hover);
      const icon = flatten([selected]).find(node => node.type === "svg" && node.bounds?.width === 16)!.bounds!;
      const iconRead = Bun.spawn(["magick", initialShot, "-crop", `${Math.round(icon.width * scale)}x${Math.round(icon.height * scale)}+${Math.round(icon.x * scale)}+${Math.round(icon.y * scale)}`, "-format", "%k", "info:"], { stdout: "pipe", stderr: "inherit" });
      assert.ok(Number(await new Response(iconRead.stdout).text()) > 1, "Semantic SVG must actually paint nonblank pixels"); assert.equal(await iconRead.exited, 0);
      // The retained viewport is pixel-scrolled; wheel never changes the keyboard choice.
      const direction = kind === "history" && position === "bottom" ? 1 : -1;
      await safe(() => viewport.wheel(0, direction * 60));
      const after = await snapshot("after"), movedThumb = await app!.getByTestId("terminal-suggestions-scrollbar").bounds();
      assert.ok(Math.abs(movedThumb.y - thumb.y) > .1, "Native wheel must move the pixel viewport");
      assert.notDeepEqual(after.rows.map(node => node.testId), before.rows.map(node => node.testId), "Wheel must expose different suggestions");
      // Reverse to the original edge and check clamping after a large wheel delta.
      await safe(() => viewport.wheel(0, -direction * 1000));
      assert.ok(Math.abs((await app!.getByTestId("terminal-suggestions-scrollbar").bounds()).y - thumb.y) < 1, "Reverse wheel clamps at the initial viewport edge");
      await snapshot("returned");
      if (kind === "history") {
        await safe(() => command.press("enter"));
        output = ""; await command.press("enter"); await until(() => /(?:^|[\r\n])PRIVATE_SCROLL_17\r?\n/.test(plainOutput())); await idle();
        await command.fill("echo PRIVATE_SCROLL_"); await command.press("up"); await menu.waitFor();
      }
      // Force a fractional row offset for compositor evidence of actual clipping.
      await safe(() => viewport.wheel(0, direction * 11.1));
      const fractionalShot = await captureOwnWindow(`${kind}-fractional.png`);
      console.log(`Own ${kind} fractional/clipping screenshot`, fractionalShot);
      await snapshot("fractional");
      const viewBox = await viewport.bounds();
      for (const y of [viewBox.y - 3, viewBox.y + viewBox.height + 1]) {
        const strip = Bun.spawn(["magick", fractionalShot, "-crop", `${Math.round(16 * scale)}x${Math.round(2 * scale)}+${Math.round(icon.x * scale)}+${Math.round(y * scale)}`, "-format", "%k:%[hex:p{0,0}]", "info:"], { stdout: "pipe", stderr: "inherit" });
        const value = await new Response(strip.stdout).text(); assert.equal(await strip.exited, 0);
        assert.ok(value.startsWith(`1:${config.theme.bg.slice(1).toUpperCase()}`), `Fractional SVG/text rows must not bleed outside viewport: ${value}`);
      }
      await command.press("escape");
      console.log(`PASS ${position} ${kind}: overflowing/virtualized SVG rows, theme bg/hover, wheel viewport movement, reverse/clamp, fractional clipping screenshot${kind === "history" ? ", original selection after return" : ""}`);
    };
    // Real executions populate recent history without synthetic production hooks.
    for (let index = 0; index < 18; index++) await run(`echo PRIVATE_SCROLL_${index}`, `PRIVATE_SCROLL_${index}`);
    await command.fill("echo PRIVATE_SCROLL_"); await command.press("up"); await inspectMenu("history");
    await command.fill("echo many/suggestion_"); await command.press("end"); await command.press("tab"); await inspectMenu("completion");
    await command.fill("");
  }
  stage = "private fixture browse authorization";
  console.log("Private fixture browse response", JSON.stringify(await request("Files", { BrowserListDir: { path: workspace } })));
  stage = "Tab previews, cycles, ShiftTab returns, one Enter executes";
  await command.fill("echo choice_"); await command.press("end"); await cursor.waitFor();
  const queryCaret = await cursor.bounds();
  await safe(() => command.press("tab")); await app.getByTestId("terminal-suggestion:0").waitFor();
  assert.ok((await app.getByTestId("terminal-suggestion:0").textContent()).includes("choice_a.txt"));
  await until(async () => await cursor.count() === 1 && (await cursor.bounds()).x > queryCaret.x + 8);
  const firstCaret = await cursor.bounds();
  if (backgroundFixture) console.log("Private PNG wallpaper/menu/block-cursor screenshot", await captureOwnWindow("background-menu-cursor.png"));
  await safe(() => command.press("tab"));
  await until(async () => await cursor.count() === 1 && (await cursor.bounds()).x > firstCaret.x + 8);
  const secondCaret = await cursor.bounds();
  await safe(() => command.press("shift-tab"));
  await until(async () => await cursor.count() === 1 && Math.abs((await cursor.bounds()).x - firstCaret.x) < 3);
  assert.ok(secondCaret.x > firstCaret.x + 8, "Second Tab must immediately preview a different draft");
  output = ""; await command.press("enter"); await until(() => /(?:^|[\r\n])choice_a\.txt\r?\n/.test(plainOutput())); await idle();
  console.log("PASS Tab/Tab/ShiftTab changes draft immediately without executing; ONE Enter executes first preview");
  stage = "Escape restores completion query";
  await command.fill("echo choice_"); await command.press("end"); await command.press("tab"); await app.getByTestId("terminal-suggestion:0").waitFor();
  await safe(() => command.press("escape")); await until(async () => await app!.getByTestId("terminal-suggestions").count() === 0);
  output = ""; await command.press("enter"); await until(() => /(?:^|[\r\n])choice_\r?\n/.test(plainOutput())); await idle();
  console.log("PASS Escape restores original query; Enter executes query rather than preview");
  stage = "native Escape closes Tab menu";
  await command.fill("git st"); await command.press("tab"); await app.getByTestId("terminal-suggestions").waitFor();
  await safe(() => command.press("escape")); await until(async () => await app!.getByTestId("terminal-suggestions").count() === 0);
  console.log("PASS native Escape closes Tab menu without executing");
  stage = "idle Ctrl+C clears draft";
  const cancelledMarker = `CANCELLED_DRAFT_${process.pid}`;
  await command.fill(`echo ${cancelledMarker}`); await command.press("ctrl-c"); await Bun.sleep(300); await idle();
  output = ""; await command.press("enter"); await Bun.sleep(500); assert.ok(!output.includes(cancelledMarker), "Idle Ctrl+C left an executable draft");
  await run("printf 'PRIVATE_%s\\n' AFTER_CLEAR", "PRIVATE_AFTER_CLEAR");
  console.log("PASS idle native Ctrl+C clears draft; next Enter does not execute it");
  stage = "long running Ctrl+C";
  await command.fill("printf 'PRIVATE_%s\\n' RUNNING; sleep 30"); await command.press("enter"); await until(() => output.includes("PRIVATE_RUNNING"));
  await app.getByTestId("terminal-block-history").waitFor(); await app.getByTestId("terminal-block-history").press("ctrl-c"); await idle();
  await run("printf 'PRIVATE_%s\\n' INTERRUPTED", "PRIVATE_INTERRUPTED");
  console.log("PASS running-block Ctrl+C and subsequent command");
  stage = "alternate screen returns to blocks";
  await command.fill("printf '\\033[?1049hPRIVATE_ALT'; read answer; printf '\\033[?1049lPRIVATE_%s\\n' RETURNED"); await command.press("enter");
  await until(() => output.includes("PRIVATE_ALT")); await app.getByTestId("terminal").waitFor(); await app.getByTestId("terminal").press("enter"); await until(() => output.includes("PRIVATE_RETURNED")); await idle();
  console.log("PASS alternate screen and return to block composer");
  stage = "Right in middle preserves native movement without accepting ghost";
  await command.click(); await command.fill("echo ACC"); await app.getByTestId("terminal-ghost-text").waitFor();
  await safe(() => app!.call("keystrokes", { keys: "left left right X" }));
  output = ""; await command.press("enter"); await until(() => /(?:^|[\r\n])ACXC\r?\n/.test(plainOutput())); await idle();
  assert.ok(!output.includes(acceptedMarker));
  console.log("PASS Right in middle moves natively; does not accept ghost");
  stage = "Right with selection collapses natively without accepting ghost";
  await command.fill("echo ACC"); await safe(() => app!.call("keystrokes", { keys: "ctrl-a right" }));
  output = ""; await command.press("enter"); await until(() => /(?:^|[\r\n])ACC\r?\n/.test(plainOutput())); await idle();
  assert.ok(!output.includes(acceptedMarker));
  console.log("PASS Right with selection collapses natively; does not accept ghost");
  stage = "ghost and Right acceptance never execute";
  await run(`echo ${acceptedMarker}`, acceptedMarker);
  await command.click(); await command.fill("echo ACC"); await command.press("end"); await app.getByTestId("terminal-ghost-text").waitFor();
  await safe(() => app!.call("keystrokes", { keys: "right" }));
  output = ""; await command.press("enter"); await until(() => output.includes(acceptedMarker)); await idle();
  console.log("PASS ghost Right acceptance and explicit next Enter executes full command");
  const clearScreen = async (clearCommand: string) => {
    await idle(); output = ""; await command.fill(clearCommand); await command.press("enter");
    await until(() => /\x1b\]133;D;0\x07/.test(output)); await idle(); await Bun.sleep(200);
    const bounds = await app!.getByTestId("terminal-block-ink").bounds();
    for (let y = 8; y < bounds.height; y += 16) {
      await ensureWindowActive(); await app!.mouse.move({ x: bounds.x + 24, y: bounds.y + y });
      assert.equal(await app!.getByTestId("block-more").count(), 0, `${clearCommand} retained an archived/empty command box`);
    }
  };
  stage = "clear removes every archived box including its own";
  await clearScreen("clear"); console.log("PASS clear removes all archived command/output boxes, including clear's own box");
  stage = "real ls output filename opens native editor";
  await run("ls -1", "\r\nclicked.txt\r\n"); await Bun.sleep(300);
  console.log("Clicked fixture stat", JSON.stringify(await request("Files", { BrowserStat: { path: join(workspace, "clicked.txt") } })));
  const ink = await app.getByTestId("terminal-block-ink").bounds(), rowHeight = config.fontSize * 1.3;
  const lineY = position === "top" ? ink.y + rowHeight * 6.19 : ink.y + ink.height - rowHeight * 1.5;
  console.log("ls native ink/output coordinates", JSON.stringify({ ink, x: ink.x + 16 * config.scale + 5, y: lineY, rowHeight }));
  if (process.env.NEOISM_BLOCK_E2E_CAPTURE_LS === "1") console.log("Own ls screenshot", await captureOwnWindow("ls-own-window.png"));
  await until(async () => {
    for (const offset of [0, -rowHeight / 2, rowHeight / 2, -rowHeight, rowHeight]) {
      await ensureWindowActive();
      await app!.call("click", { x: ink.x + 16 * config.scale + 5, y: lineY + offset });
      await Bun.sleep(80); if (await app!.getByTestId("code-editor").count()) return true;
    }
    return false;
  });
  await until(async () => {
    const { tree } = await app!.call("getTree", {});
    return flatten(tree ? [tree] : []).some(node => /^pane-tab:\d+$/.test(node.testId || "") && flatten([node]).some(child => child.text === "clicked.txt"));
  });
  console.log("PASS clicking verified filename in real ls -1 canvas output opens actual fixture in native code editor");
  const terminalTab = await app.getByTestId(tab!.testId!).bounds(); await ensureWindowActive();
  await app.call("click", { x: terminalTab.x + 16, y: terminalTab.y + terminalTab.height / 2 }); await idle();
  for (const clearCommand of ["clear -x", "reset"]) {
    stage = `${clearCommand} removes archived boxes`;
    await clearScreen(clearCommand); await run("printf 'PRIVATE_%s\\n' AFTER_REAL_CLEAR", "PRIVATE_AFTER_REAL_CLEAR");
    console.log(`PASS ${clearCommand} removes boxes and next command executes`);
  }
  stage = "alternate-screen clear/redraw preserves prior archived output";
  await clearScreen("clear");
  const preserved = `ALT_HISTORY_${process.pid}`;
  await run(`echo ${preserved}`, preserved);
  await command.fill("printf '\\033[?1049h\\033[2J\\033[HPRIVATE_ALT'; read answer; printf '\\033[?1049lPRIVATE_%s\\n' REDRAW_RETURNED"); await command.press("enter");
  await until(() => output.includes("PRIVATE_ALT")); await app.getByTestId("terminal").waitFor(); await app.getByTestId("terminal").press("enter");
  await until(() => output.includes("PRIVATE_REDRAW_RETURNED")); await idle();
  await app.getByTestId("terminal-block-history").wheel(0, position === "top" ? 10000 : -10000);
  const preservedBounds = await app.getByTestId("terminal-block-history").bounds();
  let foundPreserved = false;
  for (let y = 8; y < preservedBounds.height && !foundPreserved; y += 20) {
    await ensureWindowActive(); await app.mouse.move({ x: preservedBounds.x + 24, y: preservedBounds.y + y });
    if (!await app.getByTestId("block-more").count()) continue;
    await app.getByTestId("block-more").click(); await app.getByTestId("block-copy-output").waitFor(); await app.getByTestId("block-copy-output").click(); await Bun.sleep(100);
    const read = Bun.spawn(["wl-paste", "--no-newline"], { stdout: "pipe" });
    const copied = await new Response(read.stdout).text(); foundPreserved = await read.exited === 0 && copied.includes(preserved);
  }
  assert.ok(foundPreserved, "Alternate-screen redraw lost prior archived output (checked through actual Copy output)");
  console.log("PASS alternate screen full clear/redraw preserves prior archived history");
  console.log(JSON.stringify({ debugBridge: core, debugDisplay: display.binding, shell, position, nativeWindow: true, isolatedDaemon: true, enterExecution: true, archiveCopy: !!Bun.which("wl-paste"), ghostRight: true, middleRight: true, selectionRight: true, upEnterInsertion: true, tabPreview: true, tabEnterExecutes: true, escapeRestores: true, blockCursor: true, outputFileClick: true, clearResets: true, alternateHistoryPreserved: true, escapeMenu: true, idleCtrlC: true, ctrlC: true, alternateReturn: true }));
  }
} catch (error) { console.error(`Native block e2e failed: ${stage}`); console.error("PTY", JSON.stringify(output)); if (app) try { const flatten = (nodes: TreeNode[]): TreeNode[] => nodes.flatMap(node => [node, ...flatten(node.children || [])]); const { tree } = await app.call("getTree", {}); console.error("Native nodes", JSON.stringify(flatten(tree ? [tree] : []).filter(node => node.testId).map(node => ({ testId: node.testId, bounds: node.bounds })))); } catch (diagnostic) { console.error("Native tree unavailable", diagnostic); } console.error((await Bun.file(join(directory, "daemon.log")).text()).slice(-1500)); throw error; }
finally {
  if (focusMonitor) clearInterval(focusMonitor);
  try { await app?.close(); } catch (error) { console.error("Native window cleanup", error); }
  if (client && sessionId && client.state.phase === "connected") try { await Effect.runPromise(client.request("Pty", { ClosePty: { session_id: sessionId } })); } catch {}
  client?.stop(); daemon.kill("SIGTERM"); await daemon.exited; agent.stop(true);
  if (process.env.NEOISM_BLOCK_E2E_KEEP === "1") console.error(`Private fixture retained: ${directory}`); else rmSync(directory, { recursive: true, force: true });
}