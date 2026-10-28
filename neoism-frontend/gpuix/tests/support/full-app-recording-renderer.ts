import type { createRoot, NativeRenderer, EventPayload } from "@gpuix/solid";

export interface RecordedAppNode {
  id: number; type: string; children: number[]; props: Record<string, unknown>;
  style: Record<string, unknown>; text: string; events: Set<string>;
}
/** A native boundary for full component-graph tests. It records public GPUIX
 * mutations and sends fixed positive layout events, never initializes a GPU
 * renderer/window and deliberately does not implement a layout engine. */
export function createRecordingAppRenderer() {
  const nodes = new Map<number, RecordedAppNode>(), layouts = new Set<number>(), focusRequests: number[] = [];
  let nativeRoot: number | undefined, focused: number | null = null, windowId = 0;
  const bounds = new Map<number, { x: number; y: number; width: number; height: number }>();
  const renderer = {
    getWindowSize: () => ({ width: 1440, height: 900 }),
    setWindowTitle() {}, setImage() {}, putPaintImage() {}, dropPaintImage() {},
    getSelectedText: () => null, getScrollOffset: () => [0, 0], scrollIntoView() {},
    getFocusedElementId: () => focused, getFocusedElementIdSnapshot: () => focused,
    getElementBounds: (id: number) => bounds.get(id) || null,
    focusElement(id: number) { focusRequests.push(id); },
    setWindowKeyEvents(enabled: boolean, _up: boolean, id: number) { if (enabled) windowId = id; },
    applyBatch(json: string) {
      const destroyed: number[] = [];
      const remove = (id: number) => {
        const node = nodes.get(id); if (!node) return;
        for (const child of [...node.children]) remove(child);
        for (const parent of nodes.values()) parent.children = parent.children.filter(child => child !== id);
        nodes.delete(id); bounds.delete(id); destroyed.push(id); if (focused === id) focused = null;
      };
      for (const [op, id, a, b] of JSON.parse(json)) {
        if (op === "createElement") nodes.set(id, { id, type: a, children: [], props: {}, style: {}, text: "", events: new Set() });
        else if (op === "destroyElement") remove(id);
        else if (op === "setRoot") nativeRoot = id;
        else if (op === "setText") nodes.get(id)!.text = a;
        else if (op === "setStyle") nodes.get(id)!.style = a;
        else if (op === "setCustomProp") nodes.get(id)!.props[a] = b;
        else if (op === "setEventListener") { if (b) nodes.get(id)!.events.add(a); else nodes.get(id)!.events.delete(a); }
        else if (op === "appendChild" || op === "insertBefore") {
          for (const node of nodes.values()) node.children = node.children.filter(child => child !== a);
          const parent = nodes.get(id)!;
          if (op === "appendChild") parent.children.push(a); else parent.children.splice(parent.children.indexOf(b), 0, a);
        }
      }
      return destroyed;
    },
  };
  const find = (testId: string) => [...nodes.values()].find(node => node.props.testId === testId);
  const ancestors = (id: number) => {
    const result: number[] = [], visited = new Set<number>();
    for (let current: number | undefined = id; current !== undefined && !visited.has(current);) {
      visited.add(current); result.push(current); current = [...nodes.values()].find(node => node.children.includes(current!))?.id;
    }
    return result;
  };
  return {
    renderer: renderer as unknown as NativeRenderer, nodes, find,
    get nativeRoot() { return nativeRoot; }, get focused() { return focused; }, get windowId() { return windowId; },
    dispatchKey(root: ReturnType<typeof createRoot>, id: number | null, event: Omit<EventPayload, "elementId" | "eventType">) {
      for (const elementId of id === null ? [] : ancestors(id)) root.dispatch({ ...event, elementId, eventType: "keyDown" });
      root.dispatch({ ...event, elementId: windowId, eventType: "windowKeyDown" });
    },
    async pump(root: ReturnType<typeof createRoot>) {
      for (let pass = 0; pass < 100; pass++) {
        root.flush(); let progress = false;
        for (const node of [...nodes.values()]) if (node.events.has("layout") && !layouts.has(node.id)) {
          layouts.add(node.id); if (!nodes.has(node.id)) continue;
          const small = node.type === "text", testId = String(node.props.testId || "");
          const box = { x: testId === "file-tree" ? 0 : 300, y: 90, width: small ? Math.max(12, node.text.length * 8) : 960, height: small ? 24 : 680 };
          bounds.set(node.id, box); root.dispatch({ elementId: node.id, eventType: "layout", ...box }); progress = true;
        }
        while (focusRequests.length) {
          const id = focusRequests.shift()!; if (!nodes.has(id) || focused === id) continue;
          const before = focused; focused = id;
          if (before !== null && nodes.has(before)) root.dispatch({ elementId: before, eventType: "blur" });
          root.dispatch({ elementId: id, eventType: "focus" }); progress = true;
        }
        await Promise.resolve();
        if (!progress) { root.flush(); return; }
      }
      throw new Error("Headless App layout/focus did not settle");
    },
  };
}
