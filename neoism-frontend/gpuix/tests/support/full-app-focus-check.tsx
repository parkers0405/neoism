import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRoot } from "@gpuix/solid";
import { createWindowKeyRouter } from "../../src/commands/window-key-router";
import { App } from "../../src/ui/App";
import { AppGeneration } from "../../src/ui/AppGeneration";
import {setOverlayViewport} from "../../src/ui/overlay-viewport";
import { AppVisualState } from "../../src/ui/AppVisualState";
import { WorkspaceModel } from "../../src/model";
import { DaemonConnection } from "../../src/daemon/client";
import { loadConfig } from "../../src/config";
import { usePresentationScheduler } from "../../src/animation/frame-scheduler";
import { createAgentDaemon } from "./agent-daemon";
import { createRecordingAppRenderer } from "./full-app-recording-renderer";

const directory = mkdtempSync(join(tmpdir(), "neoism-headless-full-app-")), path = join(directory, "config.json");
const configuration = { appearance: { fonts: { family: "Cascadia Code NF", size: 14 } }, editor: { "vim-mode": true } };
writeFileSync(path, JSON.stringify(configuration)); process.argv.push("--config", path);
const daemon = createAgentDaemon({ configuration, deferCreations: false });
daemon.files.set("code.rs", "pub fn clean() {}\n");
const config = { ...loadConfig(["--config", path], {}), endpoint: daemon.endpoint, authToken: undefined, pairingToken: undefined, reducedMotion: true, cursorBlink: false };
const client = new DaemonConnection(config), model = new WorkspaceModel(client, config), visuals = new AppVisualState();
model.setUI({ openWorkspaceIds: ["a"] });
const host = createRecordingAppRenderer(), keys = createWindowKeyRouter();
const root = createRoot(host.renderer, { onKeyDown: keys.dispatch, keyboardFocusDim: false });
const displayCallbacks = new Set<() => void>();
const restoreSchedule = usePresentationScheduler(callback => {
  displayCallbacks.add(callback);
  return () => displayCallbacks.delete(callback);
});
let keystrokeId = 0, stage = "initial daemon/workspace and actual App mount";
const until = async (predicate: () => boolean) => {
  const deadline = Date.now() + 5000;
  while (!predicate()) { await host.pump(root); if (Date.now() > deadline) throw new Error(`Full App did not settle at ${stage}`); await Bun.sleep(2); }
  await host.pump(root);
};
const press = async (key: string, id: number | null = host.focused, alt = false, char = key.length === 1 ? key : "") => {
  host.dispatchKey(root, id, { key, keyChar: char, keystrokeId: ++keystrokeId, modifiers: { alt, ctrl: false, shift: false, cmd: false } });
  await host.pump(root); await Promise.resolve();
};
const agents = () => model.state.surfaces.filter(surface => surface.kind === "agent");
const command = async (value: string) => {
  await press("p", null, true); await until(() => !!host.find("command-palette"));
  root.dispatch({ elementId: host.find("command-palette")!.id, eventType: "change", value }); await host.pump(root);
  root.dispatch({ elementId: host.find("command-palette")!.id, eventType: "submit" }); await host.pump(root);
};
try {
  model.start(); client.start(); await until(() => model.state.activeWorkspace?.id === "a" && !model.state.loading);
  root.render(() => <AppGeneration value={{ model, config }}>{current => <App model={current.model} config={current.config} visuals={visuals} registerWindowKeys={keys.register}/>}</AppGeneration>);
  await host.pump(root); assert.ok(host.find("welcome-page")); const shell = host.nativeRoot;
  const closedTargets: number[] = [];
  for (const [file, testId] of [["code.rs", "code-editor"], ["README.md", "rich-markdown-editor"]] as const) {
    stage = `actual ${file} SpaceX close`;
    await model.openFile(file); await until(() => !!host.find(testId));
    const editor = host.find(testId)!; closedTargets.push(editor.id);
    await press("space", editor.id, false, " "); await press("x", editor.id);
    await until(() => !host.find(testId) && model.state.surfaces.every(surface => surface.path !== file));
    assert.ok(host.find("welcome-page")); assert.equal(model.state.activeWorkspace?.id, "a");
  }
  stage = "native window AltA after both actual editor hosts were destroyed";
  // The removed local target cannot route anything. The normalized native
  // window event must invoke the real App handler through Root options.
  await press("a", closedTargets.at(-1)!, true); await until(() => agents().length === 1 && !!host.find("composer"));
  stage = "actual App composer opts into native shaped caret geometry and paints its visible block";
  const composer = host.find("composer")!;
  assert.ok(composer.events.has("caretGeometry"), "The real App input must subscribe to native caret geometry");
  const caret = { visible: true, fontSize: 16, remainingWidth: 300, offset: 0, selectionStart: 0, selectionEnd: 0 };
  root.dispatch({ elementId: composer.id, eventType: "caretGeometry", x: 460, y: 360, value: JSON.stringify(caret) });
  const displayFrame = [...displayCallbacks]; displayCallbacks.clear();
  for (const callback of displayFrame) callback();
  await host.pump(root);
  const caretPaint = host.find("agent-composer-cursor-paint");
  assert.ok(caretPaint, "The real App must show the block cursor when its native textarea reports a focused insertion point");
  const polygon = JSON.parse(String(caretPaint.props.commands))[0];
  assert.equal(polygon.k, "p"); assert.equal(polygon.p[0].length, 8);
  const caretBounds = host.find("agent-composer-cursor")!.style;
  assert.ok(Math.abs(Number(caretBounds.width) - 9.6) < .001); assert.equal(Number(caretBounds.height), 20);
  root.dispatch({ elementId: composer.id, eventType: "caretGeometry", x: 460, y: 360, value: JSON.stringify({ ...caret, visible: false }) });
  await host.pump(root); assert.equal(host.find("agent-composer-cursor-paint"), undefined);
  const first = agents()[0]!, firstKey = model.composerDraftKey(undefined, first.id);
  root.dispatch({ elementId: host.find("composer")!.id, eventType: "change", value: "first actual App draft" }); await host.pump(root);
  assert.equal(model.state.composerDrafts[firstKey]?.text, "first actual App draft");
  stage = "focused real Composer shortcut creates one independent sibling";
  await press("a", host.find("composer")!.id, true); await until(() => agents().length === 2 && !!host.find("composer"));
  const second = agents()[1]!, secondKey = model.composerDraftKey(undefined, second.id);
  root.dispatch({ elementId: host.find("composer")!.id, eventType: "change", value: "second actual App draft" }); await host.pump(root);
  assert.notEqual(firstKey, secondKey); assert.equal(model.state.composerDrafts[firstKey]?.text, "first actual App draft");
  assert.equal(model.state.composerDrafts[secondKey]?.text, "second actual App draft");
  stage = "global fallback preserves exact active workspace without extra subscription or prompt";
  await press("a", null, true); await until(() => agents().length === 3 && !!host.find("composer"));
  const third = agents()[2]!, thirdKey = model.composerDraftKey(undefined, third.id);
  root.dispatch({ elementId: host.find("composer")!.id, eventType: "change", value: "third actual App draft" }); await host.pump(root);
  assert.equal(host.nativeRoot, shell); assert.equal(model.state.activeWorkspace?.id, "a");
  assert.equal(daemon.workspaceRequests.filter(request => request.tag === "SubscribeWorkspace" && request.body.workspace_id === "b").length, 0);
  assert.equal(daemon.workspaceRequests.filter(request => request.tag === "SubscribeWorkspace").length, 1);
  assert.equal(daemon.requests.filter(request => ["CreateThread", "SubmitPrompt"].includes(request.tag)).length, 0);
  assert.equal(daemon.ptyRequests.length, 0); assert.equal(model.state.error, undefined);
  stage = "actual AltP q closes only the third Agent and preserves the two older drafts";
  await command("q"); await until(() => agents().length === 2 && !host.find("command-dialog"));
  assert.deepEqual(agents().map(surface => surface.id), [first.id, second.id]);
  assert.equal(model.state.composerDrafts[firstKey]?.text, "first actual App draft");
  assert.equal(model.state.composerDrafts[secondKey]?.text, "second actual App draft");
  assert.equal(model.state.composerDrafts[thirdKey]?.text, "third actual App draft");
  assert.equal(model.state.activeWorkspace?.id, "a"); assert.equal(host.nativeRoot, shell);
  stage = "Root window CtrlShiftW creates exactly one real workspace with Welcome and no PTY or directory prompt";
  const beforeCreates = daemon.workspaceRequests.filter(request => request.tag === "CreateHostWorkspace").length;
  host.dispatchKey(root, null, { key: "w", keyChar: "W", keystrokeId: ++keystrokeId, modifiers: { alt: false, ctrl: true, shift: true, cmd: false } });
  await until(() => !!model.state.activeWorkspace && !["a", "b"].includes(model.state.activeWorkspace.id) && !model.state.creatingWorkspace && !!host.find("welcome-page"));
  const createdId = model.state.activeWorkspace!.id;
  assert.equal(daemon.workspaceRequests.filter(request => request.tag === "CreateHostWorkspace").length, beforeCreates + 1);
  assert.equal(model.state.surfaces.find(surface => surface.id === model.state.activeSurfaceId)?.kind, "welcome");
  assert.equal(host.find("chrome-action-overlay"), undefined); assert.equal(host.find("directory-browser"), undefined);
  assert.equal(daemon.ptyRequests.length, 0); assert.equal(host.nativeRoot, shell);
  assert.equal(model.state.composerDrafts[firstKey]?.text, "first actual App draft"); assert.equal(model.state.composerDrafts[secondKey]?.text, "second actual App draft");
  stage = "actual AltP qall closes the new workspace and returns to A without ending the app";
  await command("qall"); await until(() => model.state.activeWorkspace?.id === "a" && !model.windowWorkspaces().some(workspace => workspace.id === createdId) && !!host.find("composer"));
  assert.equal(host.nativeRoot, shell); assert.deepEqual(agents().map(surface => surface.id), [first.id, second.id]);
  assert.equal(model.state.composerDrafts[firstKey]?.text, "first actual App draft"); assert.equal(model.state.composerDrafts[secondKey]?.text, "second actual App draft");
  assert.ok(model.state.workspaces.some(workspace => workspace.id === createdId));
  assert.equal(daemon.workspaceRequests.filter(request => request.tag === "SubscribeWorkspace" && request.body.workspace_id === "b").length, 0);
  assert.equal(daemon.workspaceRequests.filter(request => /^(Close|Remove|Delete)/.test(request.tag)).length, 0);
  assert.equal(daemon.requests.filter(request => ["CreateThread", "SubmitPrompt"].includes(request.tag)).length, 0); assert.equal(daemon.ptyRequests.length, 0);
  stage="notifications stay within content bounds below all chrome and tab strips";
  for(const viewport of [{x:300,y:128,width:640,height:480},{x:100,y:260,width:360,height:150}]){
    setOverlayViewport(host.renderer,viewport);model.report(new Error("headless notification bounds check"));await host.pump(root);
    const notice=host.find("app-notification")!;assert.ok(notice);const container=[...host.nodes.values()].find(node=>node.children.includes(notice.id))!;
    const left=Number(container.style.left)+Number(notice.style.left),top=Number(container.style.top)+Number(notice.style.top),width=Number(notice.style.width),height=Number(notice.style.height);
    assert.ok(top>=viewport.y&&left>=viewport.x);assert.ok(left+width<=viewport.x+viewport.width+.001);assert.ok(top+height<=viewport.y+viewport.height+.001);
    assert.equal(container.style.overflow,"hidden");model.clearError();await host.pump(root);
  }
  console.log(JSON.stringify({ noWindow: true, fullProductionAppComponents: true, actualCodeAndMarkdownLeaderClose: true,
    postCloseWindowAltACreatesOneAgent: true, focusedComposerShortcutRunsOnce: true, threeIndependentAgentTabs: true,
    actualComposerDraftsIndependent: true, workspaceAOnlySubscribedBeforeCreation: true, noDaemonPromptsOrPty: true, persistentRoot: true,
    agentQClosesOnlyCurrentTab:true,ctrlShiftWCreatesExactlyOneWorkspace:true,newWorkspaceOpensWelcomeWithoutPrompt:true,qallReturnsToAWithoutExiting:true,notificationsRespectBodyBounds:true,actualAgentCaretPaint:true }));
} catch (error) {
  console.error(JSON.stringify({ stage, activeWorkspace: model.state.activeWorkspace?.id, activeSurface: model.state.activeSurfaceId, error: model.state.error,
    surfaces: model.state.surfaces, workspaceRequests: daemon.workspaceRequests, hosts: [...host.nodes.values()].filter(node => node.props.testId).map(node => ({ id: node.id, testId: node.props.testId })) }));
  throw error;
} finally { root.unmount(); restoreSchedule(); visuals.dispose(); model.dispose(); client.stop(); daemon.stop(); rmSync(directory, { recursive: true, force: true }); }
