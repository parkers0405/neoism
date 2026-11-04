import { expect, test } from "bun:test";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { isolatedDaemon } from "./support/isolated-daemon";
import { until } from "./support/model-fixture";
import type { TuiModel } from "../src/model";

function key(model: TuiModel, name: string, sequence = name) {
  model.key({ name, sequence, ctrl: false, shift: false, meta: false, super: false });
}

test("two TUI models: native Vim, origin undo, authoritative disk save and offline reconnect", async () => {
  const fixture = await isolatedDaemon();
  try {
    const path = join(fixture.root, "shared.txt"); writeFileSync(path, "hello α\n");
    const a = fixture.make(), b = fixture.make();
    await until(() => !!a.state.directories[""] && !!b.state.directories[""], "Both TUI models discover the isolated workspace", 10000);
    const left = (await a.openFile("shared.txt"))!, right = (await b.openFile("shared.txt"))!;
    expect(left).toBeDefined(); expect(right).toBeDefined();
    await until(() => a.documents.isSeeded(left.editor) && b.documents.isSeeded(right.editor), "Both native documents seed", 8000);
    expect(left.editor.text).toBe("hello α\n"); expect(right.editor.text).toBe(left.editor.text);
    expect(left.editor.frame.dirty).toBe(false); expect(right.editor.frame.dirty).toBe(false);
    // Drive actual terminal-key translation, not replace-source or a mock editor.
    key(a, "i"); for (const char of "left ") key(a, char === " " ? "space" : char, char); key(a, "escape", "\x1b");
    await until(() => right.editor.text === left.editor.text && right.editor.text.includes("left "), "Vim insertion reaches peer");
    b.callEditor("goto", { line: 0, col: Buffer.byteLength(right.editor.lines[0]!) - 1 });
    key(b, "a"); key(b, "!", "!"); key(b, "escape", "\x1b");
    await until(() => left.editor.text === right.editor.text && left.editor.text.includes("!"), "Second origin reaches peer");
    key(b, "u");
    await until(() => left.editor.text === right.editor.text && !left.editor.text.includes("!"), "Vim undo converges");
    expect(left.editor.text).toContain("left ");
    expect(await a.saveBuffer()).toBe(true);
    await until(() => !left.editor.frame.dirty && !right.editor.frame.dirty, "Saved acknowledgement cleans both peers");
    expect(readFileSync(path, "utf8")).toBe(left.editor.text);
    b.client.stop();
    a.callEditor("goto", { line: 0, col: 0 }); a.paste("online ");
    b.callEditor("goto", { line: 0, col: 0 }); b.paste("offline ");
    expect(await b.saveBuffer()).toBe(false); expect(b.state.error).toContain("Reconnect");
    b.clearError(); b.client.start();
    await until(() => b.state.connection.phase === "connected" && left.editor.text === right.editor.text && left.editor.text.includes("offline ") && left.editor.text.includes("online "), "Reconnect preserves both origins", 10000);
    expect(await b.saveBuffer()).toBe(true);
    await until(() => !left.editor.frame.dirty && !right.editor.frame.dirty, "Reconnect save cleans all replicas");
    expect(readFileSync(path, "utf8")).toBe(right.editor.text);
    expect(right.editor.text).toContain("left ");
    expect(a.state.error).toBeUndefined(); expect(b.state.error).toBeUndefined();
  } catch (error) { throw new Error(`${error}\nArtifact: ${fixture.executable}\n${fixture.output().slice(-4000)}`, { cause: error }); }
  finally { await fixture.dispose(); }
}, 30000);

test("real daemon Git subscription supplies the status bar's branch and diff counts", async () => {
  const fixture = await isolatedDaemon();
  try {
    const git = (...args: string[]) => {
      const result = Bun.spawnSync(["git", ...args], { cwd: fixture.root, stdout: "pipe", stderr: "pipe" });
      if (result.exitCode) throw new Error(new TextDecoder().decode(result.stderr));
    };
    git("init", "-b", "tui-status");
    writeFileSync(join(fixture.root, "status.txt"), "before\n");
    git("add", "status.txt"); git("-c", "user.name=TUI Test", "-c", "user.email=tui-test@example.invalid", "-c", "commit.gpgsign=false", "commit", "-m", "Fixture baseline");
    const model = fixture.make();
    await until(() => model.state.git?.branch === "tui-status", "Real Git branch subscription", 8000);
    expect(model.state.git?.added).toBe(0); expect(model.state.git?.deleted).toBe(0);
    const buffer = (await model.openFile("status.txt"))!;
    await until(() => model.documents.isSeeded(buffer.editor), "Git fixture document seeds");
    model.callEditor("goto", { line: 0, col: 0 }); model.paste("changed ");
    expect(await model.saveBuffer()).toBe(true);
    await until(() => model.state.git?.added === 1 && model.state.git?.deleted === 1, "Status diff updates after authoritative save", 8000);
  } finally { await fixture.dispose(); }
}, 20000);

test("real daemon: create/edit/save a file and retain buffers across workspace creation and directory changes", async () => {
  const fixture = await isolatedDaemon();
  try {
    const model = fixture.make();
    await until(() => !!model.state.activeWorkspace, "Workspace discovery");
    const original = model.state.activeWorkspace!;
    const created = (await model.createFile("created.txt"))!;
    expect(created).toBeDefined();
    await until(() => model.documents.isSeeded(created.editor), "New empty file seeds");
    key(model, "i"); for (const char of "new file") key(model, char === " " ? "space" : char, char); key(model, "escape", "\x1b");
    expect(await model.saveBuffer()).toBe(true);
    expect(readFileSync(join(fixture.root, "created.txt"), "utf8")).toBe(created.editor.text);
    await expect(model.createFile("created.txt")).rejects.toThrow();
    expect(readFileSync(join(fixture.root, "created.txt"), "utf8")).toBe(created.editor.text);
    const second = join(fixture.root, "other"); mkdirSync(second);
    await model.setWorkspaceDirectory(second, true);
    expect(model.state.activeWorkspace?.root_dir).toBe(second);
    expect(model.state.activeWorkspace?.id).not.toBe(original.id);
    expect(model.state.buffers).toHaveLength(0);
    await model.selectWorkspace(model.state.workspaces.find(workspace => workspace.id === original.id)!);
    expect(model.activeBuffer).toBe(created);
    const third = join(fixture.root, "third"); mkdirSync(third);
    await model.setWorkspaceDirectory(third);
    expect(model.state.activeWorkspace?.id).toBe(original.id);
    expect(model.state.activeWorkspace?.root_dir).toBe(third);
    await model.setWorkspaceDirectory(fixture.root);
    expect(model.activeBuffer).toBe(created);
    expect(model.activeBuffer?.editor.frame.dirty).toBe(false);
  } catch (error) { throw new Error(`${error}\n${fixture.output().slice(-4000)}`, { cause: error }); }
  finally { await fixture.dispose(); }
}, 30000);