import assert from "node:assert/strict";
import { createRoot, type NativeRenderer, type KeyEvent } from "@gpuix/solid";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { createWindowKeyRouter } from "../../src/commands/window-key-router";
import { AppGeneration } from "../../src/ui/AppGeneration";
import { WorkspaceModel } from "../../src/model";
import { DaemonConnection } from "../../src/daemon/client";
import { loadConfig } from "../../src/config";

// Only GPUIX's actual Solid host/event dispatcher is instantiated. This fake
// native boundary records mutations and never creates a renderer or window.
type Node = { children: number[]; testId?: string; props: Record<string, unknown> };
const nodes = new Map<number, Node>();
let windowEventId = 0, windowAttachments = 0;
const renderer = {
  setWindowKeyEvents(enabled: boolean, _up: boolean, id: number) { if (enabled) { windowEventId = id; windowAttachments++; } },
  applyBatch(json: string) {
    const destroyed: number[] = [];
    const remove = (id: number) => {
      const node = nodes.get(id); if (!node) return;
      for (const child of [...node.children]) remove(child);
      for (const parent of nodes.values()) parent.children = parent.children.filter(child => child !== id);
      nodes.delete(id); destroyed.push(id);
    };
    for (const [op, id, a, b] of JSON.parse(json)) {
      if (op === "createElement") nodes.set(id, { children: [], props: {} });
      else if (op === "destroyElement") remove(id);
      else if (op === "setCustomProp") { nodes.get(id)!.props[a] = b; if (a === "testId") nodes.get(id)!.testId = b; }
      else if (op === "appendChild" || op === "insertBefore") {
        for (const node of nodes.values()) node.children = node.children.filter(child => child !== a);
        const parent = nodes.get(id)!;
        if (op === "appendChild") parent.children.push(a); else parent.children.splice(parent.children.indexOf(b), 0, a);
      }
    }
    return destroyed;
  },
};
const config = loadConfig(["--config", "/nonexistent/window-key-router-headless"], {});
const client = new DaemonConnection(config), sent: unknown[] = [];
client.send = (family, message) => sent.push([family, message]);
const model = new WorkspaceModel(client, config);
const workspace = { id: "a", host_id: "host", title: "A", root_dir: "/project/a" };
model.update({ workspaces: [workspace, { ...workspace, id: "b", title: "B", root_dir: "/project/b" }], activeWorkspace: workspace });
model.ensureWelcome();
const router = createWindowKeyRouter(), normalized: KeyEvent[] = [], handledBy: string[] = [], releases: Array<() => void> = [];
const root = createRoot(renderer as unknown as NativeRenderer, { onKeyDown: event => { normalized.push(event); router.dispatch(event); } });
const [generation, setGeneration] = createSignal({ name: "first" }), [showLocal, setShowLocal] = createSignal(true);
const [state, setState] = createSignal(model.state), unsubscribe = model.subscribe(() => setState(model.state));
const id = (testId: string) => {
  const entry = [...nodes].find(([, node]) => node.testId === testId); assert.ok(entry, `Missing ${testId}`); return entry[0];
};
const agents = () => model.state.surfaces.filter(surface => surface.kind === "agent");
function Generation(props: { name: string }) {
  const handle = (event: KeyEvent) => {
    if (event.key !== "a" || !event.modifiers?.alt) return;
    event.preventDefault(); event.stopPropagation(); handledBy.push(props.name); model.newAgentHome();
  };
  const release = router.register(handle); releases.push(release); onCleanup(release);
  return <div><Show when={showLocal()}><div testId="local-page" onKeyDown={handle}/></Show>
    <For each={state().surfaces.filter(surface => surface.kind === "agent").map(surface => surface.id)}>{surfaceId => {
      const key = model.composerDraftKey(undefined, surfaceId);
      return <textarea testId={`composer:${surfaceId}`} value={state().composerDrafts[key]?.text || ""}
        onChange={event => model.setComposerDraft(key, { text: event.value || "", attachments: [] })}/>;
    }}</For></div>;
}
const key = (eventType: "keyDown" | "windowKeyDown", elementId: number, keystrokeId: number, alt = true) => ({
  eventType, elementId, keystrokeId, key: "a", keyChar: "a", modifiers: { alt, ctrl: false, cmd: false, shift: false },
});
try {
  root.render(() => <AppGeneration value={generation()}>{current => <Generation name={current.name}/>}</AppGeneration>);
  const shell = id("app-shell"), local = id("local-page");
  // A handled descendant and its native window event share stop flags.
  root.dispatch(key("keyDown", local, 1)); root.dispatch(key("windowKeyDown", windowEventId, 1));
  assert.equal(agents().length, 1); assert.equal(normalized.length, 0);
  const first = agents()[0]!, firstKey = model.composerDraftKey(undefined, first.id);
  root.dispatch({ eventType: "change", elementId: id(`composer:${first.id}`), value: "unsent local draft" });
  assert.equal(model.state.composerDrafts[firstKey]?.text, "unsent local draft");
  // Reproduce a completed close: the former native keyboard target no longer
  // exists, and the window event must still reach the current App generation.
  root.flushSync(() => setShowLocal(false));
  assert.equal(root.dispatch(key("keyDown", local, 2)), false);
  root.dispatch(key("windowKeyDown", windowEventId, 2)); root.dispatch(key("windowKeyDown", windowEventId, 2));
  assert.equal(agents().length, 2); assert.equal(normalized.length, 1);
  assert.equal(normalized[0]!.eventType, "keyDown"); assert.equal(normalized[0]!.elementId, 0);
  assert.equal(model.state.composerDrafts[firstKey]?.text, "unsent local draft");
  const second = agents()[1]!, secondKey = model.composerDraftKey(undefined, second.id);
  assert.notEqual(firstKey, secondKey);
  root.dispatch({ eventType: "change", elementId: id(`composer:${second.id}`), value: "second local draft" });
  assert.equal(model.state.composerDrafts[firstKey]?.text, "unsent local draft");
  assert.equal(model.state.composerDrafts[secondKey]?.text, "second local draft");
  root.flushSync(() => setGeneration({ name: "replacement" })); releases[0]!();
  root.dispatch(key("windowKeyDown", windowEventId, 3));
  assert.equal(agents().length, 3); assert.deepEqual(handledBy, ["first", "first", "replacement"]);
  root.dispatch(key("windowKeyDown", windowEventId, 4, false));
  assert.equal(agents().length, 3); assert.equal(normalized.at(-1)!.defaultPrevented, false);
  assert.equal(id("app-shell"), shell); assert.equal(windowAttachments, 1);
  assert.equal(model.state.activeWorkspace?.id, "a"); assert.deepEqual(sent, []);
  console.log(JSON.stringify({ noWindow: true, removedTargetUsesWindowHandler: true, descendantAndWindowDispatchOnce: true,
    normalizedWindowEvent: true, replacementGenerationRegistered: true, independentLocalAgentDrafts: true, ordinaryInputUnhandled: true, nativeRootRetained: true, noDaemonWritesOrPrompts: true }));
} finally { root.unmount(); unsubscribe(); model.dispose(); client.stop(); }
