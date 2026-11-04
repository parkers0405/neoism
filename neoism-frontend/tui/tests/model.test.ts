import { afterEach, describe, expect, test } from "bun:test";
import { treeRows } from "../src/model";
import { modelFixture, secondWorkspace, until, workspace } from "./support/model-fixture";

const fixtures: ReturnType<typeof modelFixture>[] = [];
afterEach(() => { for (const fixture of fixtures.splice(0).reverse()) fixture.dispose(); });
function fixture(options: Parameters<typeof modelFixture>[0] = {}) { const value = modelFixture(options); fixtures.push(value); return value; }

describe("native document model over WebSocket", () => {
  test("handshake scopes reads, sorts the tree and keeps editable buffers across Home and refresh", async () => {
    const { model, requests } = fixture();
    await until(() => !!model.state.directories[""]);
    expect(model.state.activeWorkspace?.id).toBe(workspace.id);
    expect(requests[0]?.message).toMatchObject({ Hello: { client_name: "neoism-tui" } });
    expect(requests.find(request => request.family === "Files")?.workspace_root).toBe(workspace.root_dir);
    const rows = treeRows(model.state.directories, model.state.expanded, workspace.root_dir);
    expect(rows.map(row => row.name)).toEqual(["src", "z.txt"]);
    await model.activateRow(rows[0]!);
    expect(treeRows(model.state.directories, model.state.expanded, workspace.root_dir).map(row => row.name)).toEqual(["src", "code.rs", "z.txt"]);
    const buffer = (await model.openFile("src/code.rs"))!;
    await until(() => model.documents.isSeeded(buffer.editor));
    model.paste("local ");
    model.showHome();
    expect(model.activeBuffer).toBeUndefined();
    await model.refresh();
    expect(await model.openFile("src/code.rs")).toBe(buffer);
    expect(buffer.editor.text).toContain("local ");
    expect(buffer.editor.frame.dirty).toBe(true);
    expect(requests.filter(request => request.message.ReadFile)).toHaveLength(1);
  });

  test("an explicit missing workspace never silently selects another", async () => {
    const { model } = fixture({ workspaceId: "missing" });
    await until(() => !!model.state.error);
    expect(model.state.error).toContain("was not found");
    expect(model.state.activeWorkspace).toBeUndefined();
  });

  test("out-of-order opens retain both buffers without stealing focus; Home invalidates focus only", async () => {
    const { model } = fixture({ delayFiles: true });
    await until(() => !!model.state.directories[""]);
    await Promise.all([model.openFile("slow.txt"), model.openFile("fast.txt")]);
    expect(model.state.activePath).toBe("fast.txt");
    expect(model.state.buffers).toHaveLength(2);
    const pending = model.openFile("third.txt"); model.showHome(); await pending;
    expect(model.state.activePath).toBeUndefined();
    expect(model.state.buffers).toHaveLength(3);
    expect(model.state.loading).toBe(false);
  });

  test("binary data never creates an editor", async () => {
    const { model } = fixture({ binary: true });
    await until(() => !!model.state.directories[""]);
    await model.openFile("binary.dat");
    expect(model.state.error).toContain("Binary files");
    expect(model.allBuffers).toHaveLength(0);
  });

  test("workspace views retain distinct same-path buffers and hidden dirty buffers protect quit", async () => {
    const { model } = fixture(); await until(() => !!model.state.directories[""]);
    const first = (await model.openFile("same.txt"))!;
    await until(() => model.documents.isSeeded(first.editor)); model.paste("first ");
    await model.selectWorkspace(secondWorkspace);
    const second = (await model.openFile("same.txt"))!;
    await until(() => model.documents.isSeeded(second.editor));
    expect(second.editor).not.toBe(first.editor);
    expect(second.editor.frame.dirty).toBe(false);
    await model.refresh();
    expect(model.activeBuffer).toBe(second);
    model.requestQuit(); expect(model.state.confirmation?.kind).toBe("quit");
    model.cancelConfirmation(); expect(model.state.quit).not.toBe(true);
    await model.selectWorkspace(workspace); await model.refresh();
    expect(model.activeBuffer).toBe(first); expect(first.editor.text).toContain("first ");
    expect(model.allBuffers).toHaveLength(2);
    model.closeBuffer(); expect(model.state.confirmation?.kind).toBe("close");
    model.cancelConfirmation(); expect(model.activeBuffer).toBe(first);
    model.closeBuffer(); await model.resolveConfirmation(true);
    expect(model.allBuffers).toEqual([second]);
    model.requestQuit(); expect(model.state.quit).toBe(true);
  });

  test("initial-sync typing rebases against authoritative changes and save waits then becomes clean", async () => {
    const peer = fixture({ holdSnapshots: true }), model = peer.model;
    await until(() => !!model.state.directories[""]);
    const buffer = (await model.openFile("seed.txt"))!;
    model.paste("local ");
    await until(() => peer.docs.size === 1);
    peer.docs.values().next().value!.getText("buffer").insert(0, "remote ");
    let finished = false; const saving = model.saveBuffer().then(value => { finished = true; return value; });
    await Bun.sleep(25); expect(finished).toBe(false);
    expect(peer.requests.some(request => request.message.SaveBuffer)).toBe(false);
    peer.releaseSnapshots(); expect(await saving).toBe(true);
    expect(buffer.editor.text).toContain("local "); expect(buffer.editor.text).toContain("remote ");
    expect(buffer.editor.frame.dirty).toBe(false);
    expect(peer.saved.get(model.documents.bufferId(buffer.editor)!)).toBe(buffer.editor.text);
    expect(peer.requests.some(request => request.message.WriteFile)).toBe(false);
  });

  test("two native Vim clients preserve origin undo and reconnect offline edits on the same workspace", async () => {
    const peer = fixture(), a = peer.model, b = peer.make();
    await until(() => !!a.state.directories[""] && !!b.state.directories[""]);
    const left = (await a.openFile("shared.txt"))!, right = (await b.openFile("shared.txt"))!;
    await until(() => a.documents.isSeeded(left.editor) && b.documents.isSeeded(right.editor));
    const key = (model: typeof a, name: string, sequence = name) => model.key({ name, sequence, ctrl: false, shift: false, meta: false, super: false });
    key(a, "i"); key(a, "A"); key(a, "escape", "\x1b");
    await until(() => left.editor.text === right.editor.text && right.editor.text.startsWith("A"));
    b.callEditor("goto", { line: 0, col: Buffer.byteLength(right.editor.text) - 1 });
    key(b, "a"); key(b, "!"); key(b, "escape", "\x1b");
    await until(() => left.editor.text === right.editor.text && left.editor.text.includes("!"));
    key(b, "u");
    await until(() => left.editor.text === right.editor.text && !left.editor.text.includes("!"));
    expect(right.editor.text).toStartWith("A");
    expect(await a.saveBuffer()).toBe(true);
    await until(() => !left.editor.frame.dirty && !right.editor.frame.dirty);
    b.client.stop();
    a.callEditor("goto", { line: 0, col: 0 }); a.paste("online ");
    b.callEditor("goto", { line: 0, col: 0 }); b.paste("offline ");
    b.closeBuffer(); expect(b.state.confirmation?.kind).toBe("close");
    await b.resolveConfirmation(true);
    expect(b.state.confirmation?.kind).toBe("close");
    expect(b.activeBuffer).toBe(right); expect(b.state.error).toContain("Reconnect");
    b.cancelConfirmation(); b.clearError(); b.client.start();
    await until(() => b.state.connection.phase === "connected" && left.editor.text === right.editor.text && right.editor.text.includes("offline ") && right.editor.text.includes("online "));
    expect(await b.saveBuffer()).toBe(true);
    await until(() => !left.editor.frame.dirty && !right.editor.frame.dirty);
    expect(peer.saved.get(b.documents.bufferId(right.editor)!)).toBe(right.editor.text);
    expect(a.state.error).toBeUndefined(); expect(b.state.error).toBeUndefined();
    b.requestQuit(); expect(b.state.quit).toBe(true);
  });

  test("reconnect refresh retains every workspace; save-and-quit includes hidden dirty buffers", async () => {
    const peer = fixture(), model = peer.model;
    await until(() => !!model.state.directories[""]);
    const first = (await model.openFile("same.txt"))!;
    await until(() => model.documents.isSeeded(first.editor)); model.paste("one ");
    await model.selectWorkspace(secondWorkspace);
    const second = (await model.openFile("same.txt"))!;
    await until(() => model.documents.isSeeded(second.editor)); model.paste("two ");
    model.client.stop(); model.paste("offline ");
    const handshakes = peer.requests.filter(request => request.message.Hello).length;
    model.reconnect();
    await until(() => peer.requests.filter(request => request.message.Hello).length > handshakes && model.state.connection.phase === "connected");
    await model.refresh();
    expect(model.activeBuffer).toBe(second);
    await model.selectWorkspace(workspace); await model.refresh();
    expect(model.activeBuffer).toBe(first); expect(first.editor.text).toContain("one ");
    expect(second.editor.text).toContain("offline ");
    model.requestQuit(); expect(model.state.confirmation?.kind).toBe("quit");
    await model.resolveConfirmation(true);
    expect(model.state.quit).toBe(true); expect(model.state.confirmation).toBeUndefined();
    for (const buffer of [first, second]) {
      expect(buffer.editor.frame.dirty).toBe(false);
      expect(peer.saved.get(model.documents.bufferId(buffer.editor)!)).toBe(buffer.editor.text);
    }
  });
});