import assert from "node:assert/strict";
import { createRoot, type NativeRenderer } from "@gpuix/solid";
import { createSignal, onCleanup, onMount } from "solid-js";
import { AppGeneration } from "../../src/ui/AppGeneration";

/** Exercise GPUIX's actual Solid host and mutation queue without opening a
 * desktop window. Each snapshot is one transaction received by native code. */
function recordingRenderer() {
  type Node = { children: number[]; text: string; testId?: string };
  const nodes = new Map<number, Node>(), snapshots: Array<{ root?: number; text: string; shell?: number }> = [];
  let root: number | undefined, attachments = 0;
  const text = (id: number): string => {
    const node = nodes.get(id); return node ? node.text + node.children.map(text).join("") : "";
  };
  const remove = (id: number, destroyed: number[]) => {
    const node = nodes.get(id); if (!node) return;
    for (const child of node.children) remove(child, destroyed);
    for (const parent of nodes.values()) parent.children = parent.children.filter(child => child !== id);
    nodes.delete(id); destroyed.push(id);
  };
  const renderer = {
    setWindowKeyEvents(enabled: boolean) { if (enabled) attachments++; },
    applyBatch(json: string) {
      const destroyed: number[] = [];
      for (const [op, id, a, b] of JSON.parse(json)) {
        if (op === "createElement") nodes.set(id, { children: [], text: "" });
        else if (op === "destroyElement") remove(id, destroyed);
        else if (op === "setRoot") root = id;
        else if (op === "setText") nodes.get(id)!.text = a;
        else if (op === "setCustomProp" && a === "testId") nodes.get(id)!.testId = b;
        else if (op === "appendChild" || op === "insertBefore") {
          for (const node of nodes.values()) node.children = node.children.filter(child => child !== a);
          const parent = nodes.get(id)!;
          if (op === "appendChild") parent.children.push(a);
          else parent.children.splice(parent.children.indexOf(b), 0, a);
        }
      }
      snapshots.push({ root, text: root === undefined ? "" : text(root), shell: [...nodes].find(([, node]) => node.testId === "app-shell")?.[0] });
      return destroyed;
    },
  };
  return { renderer: renderer as unknown as NativeRenderer, snapshots, get attachments() { return attachments; } };
}

{
  const host = recordingRenderer(), root = createRoot(host.renderer);
  const model = { activeSurfaceId: "dirty-file", draft: "keep unsent text", source: "unsaved source" };
  const [generation, setGeneration] = createSignal({ model, size: 14 });
  const mounted: number[] = [], disposed: number[] = [], focus: string[] = [];
  function ScaledApp(props: { model: typeof model; size: number }) {
    const size = props.size;
    onMount(() => { mounted.push(size); focus.push(props.model.activeSurfaceId); });
    onCleanup(() => disposed.push(size));
    return <div><text>{`${props.model.source}@${size}`}</text><text>{props.model.draft}</text></div>;
  }
  try {
    root.render(() => <AppGeneration value={generation()}>{current => <ScaledApp {...current} />}</AppGeneration>);
    const shell = host.snapshots.at(-1)!.shell;
    assert.equal(typeof shell,"number");
    for (const size of [15, 16, 17, 16, 15, 14]) {
      root.flushSync(() => setGeneration({ model, size }));
      await Promise.resolve();
      assert.equal(host.snapshots.at(-1)!.text,`unsaved source@${size}keep unsent text`);
    }
    assert.ok(host.snapshots.every(snapshot => snapshot.root === shell && snapshot.shell === shell && snapshot.text.length > 0));
    assert.equal(host.attachments,1);
    assert.deepEqual(mounted,[14, 15, 16, 17, 16, 15, 14]);
    assert.deepEqual(disposed,[14, 15, 16, 17, 16, 15]);
    assert.deepEqual(focus,Array(7).fill("dirty-file"));
    assert.equal(generation().model,model);
    assert.equal(model.draft,"keep unsent text");
    console.log(JSON.stringify({swaps:6,attachments:host.attachments,rootRetained:true,emptyTransactions:host.snapshots.filter(snapshot=>!snapshot.text).length,restoredFocus:focus}));
  } finally { root.unmount(); }
}
