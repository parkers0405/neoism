import { test } from "node:test";
import assert from "node:assert/strict";
import { TerminalPanel } from "./TerminalPanel.ts";

// Exercise metadata routing without constructing a DOM/canvas or loading wasm.
function panelFixture() {
  const shells: string[] = [];
  const panel = Object.create(TerminalPanel.prototype) as any;
  Object.assign(panel, {
    options: { sessionId: "unix" },
    bufferTabs: [
      { kind: "terminal", sessionId: "unix" },
      { kind: "terminal", sessionId: "windows" },
    ],
    activeTabIndex: 0,
    terminalSessionShells: new Map(),
    terminalSessionCwds: new Map(),
    pendingTerminalTabSpawns: [{ title: "pending" }],
    wasmAdapter: { setTerminalShell: (program: string) => shells.push(program) },
  });
  return { panel, shells };
}

test("attach metadata does not consume pending spawns or replace the focused shell", () => {
  const { panel, shells } = panelFixture();
  panel.ptyCreated("unix", "/bin/zsh");
  panel.ptyCreated("windows", "pwsh.exe");
  panel.ptyCreated("windows", "pwsh.exe");
  assert.deepEqual(shells, ["/bin/zsh"]);
  assert.equal(panel.pendingTerminalTabSpawns.length, 1);
  assert.equal(panel.terminalSessionShells.get("windows"), "pwsh.exe");
  assert.equal(panel.activeTabIndex, 0);
});

test("tab activation restores that session's actual shell before replay", () => {
  const { panel, shells } = panelFixture();
  panel.terminalSessionShells.set("unix", "/bin/fish");
  panel.terminalSessionShells.set("windows", "powershell.exe");
  const replays: string[] = [];
  Object.assign(panel, {
    root: { clientWidth: 800, clientHeight: 600 },
    syncActiveBreadcrumbs() {}, handleResize() {}, syncActiveMarkdownLayer() {},
    setMarkdownLayerVisible() {}, scheduleDraw() {},
    replayPtySession(id: string) {
      assert.equal(shells.at(-1), panel.terminalSessionShells.get(id));
      replays.push(id);
    },
  });
  panel.activeTabIndex = 1;
  panel.activateCurrentTabContents(false);
  panel.activeTabIndex = 0;
  panel.activateCurrentTabContents(false);
  assert.deepEqual(shells, ["powershell.exe", "/bin/fish"]);
  assert.deepEqual(replays, ["windows", "unix"]);
});

test("legacy metadata does not erase a known shell on reconnect", () => {
  const { panel, shells } = panelFixture();
  panel.ptyCreated("unix", "/bin/bash");
  panel.ptyCreated("unix");
  assert.deepEqual(shells, ["/bin/bash", "/bin/bash"]);
});

test("spawn command framing uses the target shell rather than the active tab", () => {
  const { panel } = panelFixture();
  const framed: string[][] = [];
  const sent: Array<[string, Uint8Array]> = [];
  panel.pendingTerminalTabSpawns = [{ command: "echo ready" }];
  panel.options.pty = { sendInput: (id: string, bytes: Uint8Array) => sent.push([id, bytes]) };
  panel.wasmAdapter.shellCommandPayload = (program: string, command: string) => {
    framed.push([program, command]);
    return new TextEncoder().encode(`${command}\r`);
  };
  panel.registerTerminalSession = () => {};
  panel.replayBufferTabs = () => {};
  panel.activatePtySession = () => {};
  panel.ptyCreated("fresh", "cmd.exe");
  assert.deepEqual(framed, [["cmd.exe", "echo ready"]]);
  assert.equal(sent[0][0], "fresh");
  assert.equal(new TextDecoder().decode(sent[0][1]), "echo ready\r");
});

test("shell metadata survives asynchronous wasm startup", () => {
  const { panel, shells } = panelFixture();
  const adapter = panel.wasmAdapter;
  panel.wasmAdapter = null;
  panel.ptyCreated("windows", "cmd.exe");
  panel.activeTabIndex = 1;
  panel.wasmAdapter = adapter;
  panel.setTerminalShell("windows", null);
  assert.deepEqual(shells, ["cmd.exe"]);
});
