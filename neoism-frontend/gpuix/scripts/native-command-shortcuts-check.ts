import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { launchNative } from "../tests/support/native-launch";
import { createFullAppGitDaemon } from "../tests/support/full-app-git-daemon";
import type { Surface } from "../src/model";

// Exercise production App routing, native focus and input, not source assertions.
const directory = mkdtempSync(join(tmpdir(), "neoism-command-shortcuts-"));
const server = createFullAppGitDaemon();
server.files.set("shortcut.ts", "export const untouched = true;\n");
const config = join(directory, "config.json");
writeFileSync(config, JSON.stringify({ editor: { "vim-mode": true }, terminal: { cursor: { blinking: false } },
  keybinds: { keys: [{ key: "y", with: "control | shift", action: "OpenCommandPalette" }] } }));
const cases: Array<{ name: string; surface: Partial<Surface>; targets: string[] }> = [
  { name: "welcome", surface: { kind: "welcome" }, targets: ["welcome-page", "file-tree"] },
  { name: "code", surface: { kind: "file", path: "shortcut.ts" }, targets: ["code-editor"] },
  { name: "markdown", surface: { kind: "file", path: "README.md" }, targets: ["rich-markdown-editor"] },
  { name: "terminal", surface: { kind: "terminal", sessionId: "fixture-pty" }, targets: ["terminal"] },
  { name: "agent", surface: { kind: "agent" }, targets: ["composer", "conversation"] },
  { name: "git", surface: { kind: "git", git: { mode: "working", root: "/project/a" } }, targets: ["git-review", "git-diff-panel"] },
  { name: "extensions", surface: { kind: "page", page: "extensions" }, targets: ["extensions-page"] },
  { name: "override", surface: { kind: "welcome" }, targets: ["welcome-page"] },
];
try {
  for (const fixture of cases.filter(fixture => !process.argv[2] || process.argv.slice(2).includes(fixture.name))) {
    if (fixture.name === "override") writeFileSync(config, JSON.stringify({ keybinds: { keys: [
      { key: "p", with: "command", action: "None" },
      { key: "y", with: "control | shift", action: "OpenCommandPalette" },
    ] } }));
    const state = join(directory, `${fixture.name}.json`);
    writeFileSync(state, JSON.stringify({ version: 1, activeWorkspaceId: "a", activeSurfaceId: "fixture",
      surfaces: [{ id: "fixture", title: fixture.name, workspaceId: "a", rootDir: "/project/a", ...fixture.surface }],
      editors: {}, providerStates: {}, drafts: {}, composerDrafts: {}, ui: { sidebar: "files", openWorkspaceIds: ["a"], gitPanels: { a: fixture.name === "git" } } }));
    const app = await launchNative({ command: process.execPath, args: ["src/main.tsx", "--daemon", server.endpoint, "--workspace", "a", "--config", config], cwd: process.cwd(),
      env: { ...process.env, NEOISM_GPUIX_STATE_PATH: state } });
    let stage = fixture.name;
    try {
      await app.getByTestId("original-topbar").waitFor({ timeoutMs: 20000 });
      await app.getByTestId("file:README.md").waitFor({ timeoutMs: 20000 });
      // Cold recovery intentionally opens Home. Use real UI entry points.
      if (fixture.name === "git") {
        await app.getByTestId("git-panel-hit:file:0").waitFor({ timeoutMs: 20000 });
        await app.getByTestId("git-panel-hit:file:0").click();
      }
      if (fixture.name === "extensions") {
        await app.getByTestId("topbar-menu").click();
        await app.getByTestId("menu:OpenExtensions").waitFor();
        await app.getByTestId("menu:OpenExtensions").click();
      }
      if (fixture.name === "terminal") await app.call("keystrokes", { keys: "ctrl-t" });
      if (fixture.name === "agent") await app.call("keystrokes", { keys: "alt-a" });
      if (fixture.surface.kind === "file") {
        await app.getByTestId(`file:${fixture.surface.path}`).waitFor({ timeoutMs: 20000 });
        await app.getByTestId(`file:${fixture.surface.path}`).click();
      }
      for (const target of fixture.targets) {
        stage = `${fixture.name}/${target}`;
        if (target === "conversation") {
          await app.getByTestId("composer").fill("Shortcut history fixture");
          await app.getByTestId("composer").press("enter");
        }
        const view = app.getByTestId(target);
        await view.waitFor({ timeoutMs: 20000 });
        for (const chord of fixture.name === "override" ? ["ctrl-shift-y"] : ["cmd-p", "cmd-shift-p", "ctrl-shift-p", "alt-p", "ctrl-shift-y"]) {
          stage = `${fixture.name}/${target}/${chord}`;
          const host = await view.waitFor(); await app.call("focus", { elementId: host.id });
          const writes = server.ptyRequests.filter(r => r.tag === "PtyInput").length;
          const searches = server.searchRequests.length;
          // Deliberately queue text before the native palette focus catches up.
          await app.call("keystrokes", { keys: `${chord} q` });
          await app.getByTestId("command:CloseTab").waitFor();
          assert.equal(await app.getByTestId("finder").count(), 0, `${stage}: opened file finder`);
          assert.equal(server.searchRequests.length, searches, `${stage}: opened file finder`);
          assert.equal(server.ptyRequests.filter(r => r.tag === "PtyInput").length, writes, `${stage}: leaked terminal input`);
          await app.getByTestId("command-palette").press("escape");
        }
        if (fixture.name === "welcome" || fixture.name === "override") {
          stage = `${fixture.name}/finder routing`;
          const host = await view.waitFor(); await app.call("focus", { elementId: host.id });
          await app.call("keystrokes", { keys: fixture.name === "override" ? "cmd-p" : "ctrl-p" });
          if (fixture.name === "welcome") await app.getByTestId("finder").waitFor();
          else assert.equal(await app.getByTestId("finder").count(), 0, "Explicit None must suppress defaults");
          assert.equal(await app.getByTestId("command-dialog").count(), 0);
          if (fixture.name === "welcome") await app.getByTestId("command-palette").press("escape");
        }
      }
      console.log(`PASS ${fixture.name}: ${fixture.targets.join(", ")} command chords and queued input`);
    } catch (error) { console.error(`Command shortcuts failed: ${stage}`); throw error; }
    finally { await app.close(); }
  }
} finally { server.stop(); rmSync(directory, { recursive: true, force: true }); }