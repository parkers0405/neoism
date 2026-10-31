import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { decodeSession, SessionStore } from "../src/session-store";
import { WorkspaceModel, type Surface } from "../src/model";
import { DaemonConnection } from "../src/daemon/client";
import { loadConfig } from "../src/config";
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).reverse().forEach((close) => close()));
const workspace = { id: "a", host_id: "local", title: "A", root_dir: "/project" };
const surface: Surface = { id: "file:a:note.txt", kind: "file", title: "note.txt", workspaceId: "a", rootDir: "/project", path: "note.txt" };
test("independent Agent homes and their drafts survive disk decoding without inventing backend sessions",()=>{
 const app=model();app.openAgentHome();const legacy=app.state.surfaces.find(surface=>surface.id===app.state.activeSurfaceId)!;
 const first=app.newAgentHome()!,second=app.newAgentHome()!;
 const firstKey=app.composerDraftKey(undefined,first.id),secondKey=app.composerDraftKey(undefined,second.id);
 app.setComposerDraft(firstKey,{text:"first private draft",attachments:[]});app.setComposerDraft(secondKey,{text:"second private draft",attachments:[{kind:"image/png",path:"capture.png",bytes:[1,2,3],token:"capture",name:"capture.png"}]});
 const directory=mkdtempSync(join(tmpdir(),"neoism-agent-home-recovery-"));cleanups.push(()=>rmSync(directory,{recursive:true,force:true}));
 const store=new SessionStore({endpoint:"ws://fixture/session"},{XDG_STATE_HOME:directory});store.write(app.captureSession());const recovery=store.read()!;
 expect(recovery.surfaces.filter(surface=>surface.kind==="agent").map(surface=>surface.id)).toEqual([legacy.id,first.id,second.id]);
 expect(recovery.surfaces.filter(surface=>surface.kind==="agent").every(surface=>surface.sessionId===undefined)).toBe(true);
 const restored=model();restored.restoreSession(recovery);expect(restored.state.surfaces.filter(surface=>surface.kind==="agent")).toHaveLength(3);
 expect(restored.state.composerDrafts[firstKey]?.text).toBe("first private draft");expect(restored.state.composerDrafts[secondKey]?.attachments[0]?.bytes).toEqual([1,2,3]);
 const malformed={...recovery,surfaces:[{...first,sessionId:42},{id:"terminal:no-session",kind:"terminal",title:"Terminal",workspaceId:"a"}]};expect(decodeSession(malformed)?.surfaces).toEqual([]);
});
function model() {
  const config = { ...loadConfig(["--config", "/nonexistent/test-config"], {}), vim: true }, client = new DaemonConnection(config);
  const result = new WorkspaceModel(client, config); cleanups.push(() => result.dispose());
  result.update({ workspaces: [workspace], activeWorkspace: workspace, surfaces: [surface], activeSurfaceId: surface.id, files: { [surface.id]: "one\ntwo\nthree" } });
  return result;
}
test("checkpoint dependencies ignore chat paint but retain cursor, draft and tab changes", () => {
  const app = model(), editor = app.getEditor(surface, 1);
  const same = (a:unknown[],b:unknown[]) => a.length === b.length && a.every((value,index)=>Object.is(value,b[index]));
  const initial = app.sessionDependencies();
  expect(same(initial,app.sessionDependencies())).toBe(true);
  editor.queueKey({key:"down",char:""});
  expect(same(initial,app.sessionDependencies())).toBe(false);
  const moved = app.sessionDependencies();
  app.setComposerDraft("checkpoint-test",{text:"unsent",attachments:[]});
  expect(same(moved,app.sessionDependencies())).toBe(false);
  expect(app.captureSession().composerDrafts["checkpoint-test"].text).toBe("unsent");
  const drafted = app.sessionDependencies();
  app.update({activeSurfaceId:"different-tab"});
  expect(same(drafted,app.sessionDependencies())).toBe(false);
});
test("split editor views share edits and undo history while keeping independent cursors", () => {
  const app = model(), first = app.getEditor(surface, 1), second = app.getEditor(surface, 2);
  second.call("key", { key: "j", char: "j" });
  const cursor = second.frame.cursor.line;
  first.call("paste", { text: "edited " });
  expect(second.text()).toBe(first.text()); expect(app.getEditor(surface).text()).toBe(first.text());
  expect(second.frame.cursor.line).toBe(cursor); expect(first.frame.dirty).toBe(true);
  second.call("key", { key: "z", ctrl: true });
  expect(second.text()).toBe(first.text());
});
test("recovery restores unsaved content, saved baseline, split focus, independent view positions, and composer drafts", () => {
  const app = model(), layout = app.getLayout();
  layout.open(surface); layout.split("horizontal", surface);
  const first = app.getEditor(surface, 1), second = app.getEditor(surface, 2);
  second.call("key", { key: "j", char: "j" }); first.call("paste", { text: "unsaved " });
  app.setComposerDraft("thread", { text: "unfinished question", attachments: [] }); app.setUI({ sidebar: "notes", details: false });
  const saved = JSON.parse(JSON.stringify(app.captureSession()));
  const restored = model(); restored.restoreSession(saved);
  expect(restored.getEditor(surface).text()).toBe(first.text()); expect(restored.getEditor(surface).frame.dirty).toBe(true);
  expect(restored.getEditor(surface, 2).frame.cursor.line).toBe(second.frame.cursor.line);
  expect(restored.getLayout().snapshot()).toEqual(layout.snapshot());
  expect(restored.state.composerDrafts.thread.text).toBe("unfinished question"); expect(restored.state.ui.sidebar).toBe("notes");
  restored.getEditor(surface).call("key", { key: "z", ctrl: true });
  expect(restored.getEditor(surface).text()).toBe("one\ntwo\nthree");
});
test("session files are atomic and private, scoped to daemon identity, and omit connection credentials", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism-recovery-")); cleanups.push(() => rmSync(directory, { recursive: true, force: true }));
  const config = { endpoint: "ws://localhost/session?token=secret-upgrade", authToken: "operator-secret" };
  const store = new SessionStore(config, { XDG_STATE_HOME: directory });
  const different = new SessionStore({ ...config, authToken: "someone-else" }, { XDG_STATE_HOME: directory });
  expect(store.path).not.toBe(different.path);
  const saved = model().captureSession(); store.write(saved);
  expect(store.read()).toEqual(saved); if(process.platform!=="win32")expect(statSync(store.path).mode & 0o777).toBe(0o600);
  const raw = readFileSync(store.path, "utf8"); expect(raw).not.toContain("operator-secret"); expect(raw).not.toContain("secret-upgrade");
  writeFileSync(store.path, "{unfinished"); expect(store.read()).toBeUndefined();
  expect(decodeSession({ version: 2, surfaces: [] })).toBeUndefined();
  expect(decodeSession({ version: 1, surfaces: [{ id: "bad", kind: "file" }] })?.surfaces).toEqual([]);
});
test("asynchronous checkpoints retain the newest snapshot and concurrent server profiles", async () => {
  const directory=mkdtempSync(join(tmpdir(),"neoism-async-recovery-"));cleanups.push(()=>rmSync(directory,{recursive:true,force:true}));
  const env={NEOISM_GPUIX_STATE_PATH:join(directory,"session.json")};
  const a=new SessionStore({endpoint:"ws://host-a/session"},env),b=new SessionStore({endpoint:"ws://host-b/session"},env);
  const saved=model().captureSession();
  await Promise.all([
    a.writeAsync({...saved,composerDrafts:{home:{text:"first"}}}),
    b.writeAsync({...saved,composerDrafts:{home:{text:"other server"}}}),
    a.writeAsync({...saved,composerDrafts:{home:{text:"latest unsent draft"}}}),
  ]);
  expect(a.read()?.composerDrafts.home.text).toBe("latest unsent draft");
  expect(b.read()?.composerDrafts.home.text).toBe("other server");
  if(process.platform!=="win32")expect(statSync(a.path).mode&0o777).toBe(0o600);
});
test("a failed asynchronous checkpoint does not poison later recovery writes", async () => {
  const directory=mkdtempSync(join(tmpdir(),"neoism-recovery-retry-"));cleanups.push(()=>rmSync(directory,{recursive:true,force:true}));
  const blocked=join(directory,"blocked");writeFileSync(blocked,"not a directory");
  const store=new SessionStore({endpoint:"ws://fixture/session"},{NEOISM_GPUIX_STATE_PATH:join(blocked,"session.json")});
  const saved=model().captureSession();
  await expect(store.writeAsync(saved)).rejects.toThrow();
  rmSync(blocked);
  await store.writeAsync(saved);
  expect(store.read()).toEqual(saved);
});
test("Welcome is recovered as its own tab without a terminal or Agent session", () => {
  const app = model(), welcome: Surface = { id: "welcome:a", kind: "welcome", title: "Welcome", workspaceId: "a" };
  app.update({ surfaces: [...app.state.surfaces, welcome], activeSurfaceId: welcome.id });
  app.getLayout().open(surface); app.getLayout().open(welcome); app.getLayout().firstTab(welcome.id);
  const saved = decodeSession(JSON.parse(JSON.stringify(app.captureSession())))!;
  expect(saved.surfaces.find(entry => entry.id === welcome.id)).toEqual(welcome);
  expect(saved.activeSurfaceId).toBe(welcome.id);
  const restored = model(); restored.restoreSession(saved);
  expect(restored.getLayout().frame.panes[0].tabs[0].kind).toBe("welcome");
  expect(restored.state.activeSurfaceId).toBe(welcome.id);
  expect(restored.mainTerminalSurface()).toBeUndefined();
});
test("Vim config changes reach every independent editor and its existing rich Markdown view", () => {
  const app = model(), first = app.getEditor(surface, 1), second = app.getEditor(surface, 2);
  const markdownSurface: Surface = { ...surface, id: "file:a:note.md", title: "note.md", path: "note.md" };
  app.update({ surfaces: [...app.state.surfaces, markdownSurface], files: { ...app.state.files, [markdownSurface.id]: "# Heading\none\ntwo\nthree" } });
  const markdown = app.getMarkdown(markdownSurface, 3);
  app.setVimMode(false);
  expect(app.config.vim).toBe(false); expect(app.getEditor(surface).frame.vim).toBe(false);
  expect(first.frame.vim).toBe(false); expect(second.frame.vim).toBe(false); expect(markdown.frame.vim).toBe(false);
  app.setVimMode(true); app.setVimMode(true);
  expect(first.frame.vim).toBe(true); expect(second.frame.vim).toBe(true); expect(markdown.frame.vim).toBe(true);
});
test("Vim changes normalize hidden recovered views without changing ordinary per-document recovery", () => {
  const app = model(), editor = app.getEditor(surface, 2);
  editor.call("key", { key: "j", char: "j" }); editor.call("paste", { text: "unsaved " });
  const markdownSurface: Surface = { ...surface, id: "file:a:note.md", title: "note.md", path: "note.md" };
  app.update({ surfaces: [...app.state.surfaces, markdownSurface], files: { ...app.state.files, [markdownSurface.id]: "# Heading\none\ntwo" } });
  const rich = app.getMarkdown(markdownSurface, 3); rich.call("layout", { width: 800, height: 400 }); rich.call("goto", { line: 2, col: 1 });
  const saved = app.captureSession(), restored = model(); restored.restoreSession(structuredClone(saved));
  // Snapshotting before opening a hidden tab must retain its independent view.
  expect(restored.captureSession().editors["view:" + surface.id + "@2"]).toBeDefined();
  restored.setVimMode(false);
  const hidden = restored.getEditor(surface, 2), hiddenRich = restored.getMarkdown(markdownSurface, 3);
  hiddenRich.call("layout", { width: 800, height: 400 });
  expect(hidden.frame.vim).toBe(false); expect(hidden.frame.mode).toBe("Insert"); expect(hidden.frame.cursor.line).toBe(1);
  expect(hiddenRich.frame.vim).toBe(false); expect(hiddenRich.frame.mode).toBe("Insert"); expect(hiddenRich.frame.cursor.line).toBe(2);
  hidden.call("key", { key: "z", ctrl: true }); expect(hidden.text()).toBe("one\ntwo\nthree");
  // A normal restore keeps an intentional document override until a global
  // setting is explicitly changed.
  const unchanged = model(); unchanged.config.vim = false; unchanged.restoreSession(saved);
  expect(unchanged.getEditor(surface, 2).frame.vim).toBe(true);
});
test("hot remount keeps canonical buffers while restoring independent code and Markdown positions", () => {
  const app = model(), canonical = app.getEditor(surface), first = app.getEditor(surface, 1), second = app.getEditor(surface, 2);
  second.call("key", { key: "j", char: "j" }); first.call("paste", { text: "unsaved " });
  const markdownSurface: Surface = { ...surface, id: "file:a:note.md", title: "note.md", path: "note.md" };
  app.update({ surfaces: [...app.state.surfaces, markdownSurface], files: { ...app.state.files, [markdownSurface.id]: "# Heading\none\ntwo\nthree" } });
  const markdown = app.getMarkdown(markdownSurface, 3); markdown.call("layout", { width: 800, height: 400 }); markdown.call("goto", { line: 2, col: 1 });
  const codeLine = second.frame.cursor.line, richCursor = { ...markdown.frame.cursor }, saved = app.captureSession();
  const seed = { state: app.state, editors: app.editors, recovery: saved };
  app.dispose(true);
  const restored = new WorkspaceModel(new DaemonConnection(app.config), app.config, seed); cleanups.push(() => restored.dispose());
  expect(restored.getEditor(surface)).toBe(canonical);
  expect(restored.getEditor(surface, 2).id).not.toBe(second.id);
  expect(restored.getEditor(surface, 2).frame.cursor.line).toBe(codeLine);
  expect(restored.getEditor(surface).text()).toBe("unsaved one\ntwo\nthree"); expect(restored.getEditor(surface).frame.dirty).toBe(true);
  const restoredMarkdown = restored.getMarkdown(markdownSurface, 3);
  expect(restoredMarkdown.frame.cursor.line).toBe(richCursor.line); expect(restoredMarkdown.frame.cursor.col).toBe(richCursor.col);
});
test("an explicit state path preserves isolated unsaved profiles across A to B to A switches", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism-profile-recovery-")); cleanups.push(() => rmSync(directory, { recursive: true, force: true }));
  const env = { NEOISM_GPUIX_STATE_PATH: join(directory, "session.json") };
  const a = new SessionStore({ endpoint: "ws://host-a/session", pairingToken: "paired-token-a" }, env), b = new SessionStore({ endpoint: "ws://host-b/session", pairingToken: "paired-token-b" }, env);
  const appA = model(); appA.getEditor(surface).call("paste", { text: "unsaved A " }); const savedA = appA.captureSession(); a.write(savedA);
  expect(b.read()).toBeUndefined();
  const appB = model(); appB.getEditor(surface).call("paste", { text: "unsaved B " }); const savedB = appB.captureSession(); b.write(savedB);
  expect(a.read()).toEqual(savedA); expect(b.read()).toEqual(savedB);
  a.write({ ...savedA, composerDrafts: { home: { text: "new A draft" } } });
  expect(a.read()?.composerDrafts.home.text).toBe("new A draft"); expect(b.read()).toEqual(savedB);
  expect(new SessionStore({ endpoint: "ws://host-a/session", pairingToken: "another-identity" }, env).read()).toBeUndefined();
  const raw = readFileSync(a.path, "utf8"), envelope = JSON.parse(raw);
  expect(Object.keys(envelope.profiles)).toHaveLength(2); expect(envelope.editors).toEqual(savedA.editors);
  expect(raw).not.toContain("paired-token-a"); expect(raw).not.toContain("paired-token-b"); if(process.platform!=="win32")expect(statSync(a.path).mode & 0o777).toBe(0o600);
});
test("legacy untagged override migrates to the first identity before another profile is written", () => {
  const directory = mkdtempSync(join(tmpdir(), "neoism-profile-migration-")); cleanups.push(() => rmSync(directory, { recursive: true, force: true }));
  const env = { NEOISM_GPUIX_STATE_PATH: join(directory, "legacy.json") }, saved = model().captureSession();
  writeFileSync(env.NEOISM_GPUIX_STATE_PATH, JSON.stringify(saved));
  const a = new SessionStore({ endpoint: "ws://host-a/session" }, env), b = new SessionStore({ endpoint: "ws://host-b/session" }, env);
  expect(a.read()).toEqual(saved); a.write(saved); expect(b.read()).toBeUndefined();
  const savedB = { ...saved, activeWorkspaceId: "host-b-workspace" }; b.write(savedB);
  expect(a.read()).toEqual(saved); expect(b.read()).toEqual(savedB);
});

test("hidden recovered editors stay unallocated, preserve undo, and protect unsaved tabs", () => {
  const app = model(), editor = app.getEditor(surface);
  editor.call("paste", { text: "unsaved " });
  const saved = app.captureSession(), restored = model();
  restored.restoreSession(saved);
  expect(restored.editors.size).toBe(0);
  expect(restored.state.files[surface.id]).toBe(editor.text());
  expect(restored.isSurfaceModified(surface.id)).toBe(true);
  restored.closeSurface(surface.id);
  expect(restored.state.surfaces.some(s => s.id === surface.id)).toBe(true);
  expect(restored.editors.size).toBe(0);
  expect(restored.captureSession().editors[surface.id]).toEqual(saved.editors[surface.id]);
  const opened = restored.getEditor(surface);
  expect(opened.text()).toBe(editor.text());
  expect(restored.getEditor(surface)).toBe(opened);
  opened.call("key", { key: "z", ctrl: true });
  expect(opened.text()).toBe("one\ntwo\nthree");
});

test("clean recovered tabs can close without allocating native editors", () => {
  const app = model(); app.getEditor(surface);
  const restored = model(); restored.restoreSession(app.captureSession());
  expect(restored.isSurfaceModified(surface.id)).toBe(false);
  restored.closeSurface(surface.id);
  expect(restored.editors.size).toBe(0);
  expect(restored.state.surfaces.some(s => s.id === surface.id)).toBe(false);
});
