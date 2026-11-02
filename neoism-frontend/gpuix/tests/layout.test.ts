import { afterEach, expect, test } from "bun:test";
import { WorkspaceLayout, type LayoutSnapshot, type LayoutSurface } from "../src/layout/WorkspaceLayout";
const layouts: WorkspaceLayout[] = [];
const make = (seed?: LayoutSnapshot) => { const layout = new WorkspaceLayout(seed); layouts.push(layout); layout.layout(1000, 700); return layout; };
afterEach(() => layouts.splice(0).forEach((layout) => layout.dispose()));
const file = (id: string): LayoutSurface => ({ id, title: `${id}.rs`, kind: "file" });
const terminal = (id: string): LayoutSurface => ({ id, title: id, kind: "terminal" });

test("native tab groups keep hidden buffers alive, focus existing tabs, and close the last leaf cleanly", () => {
  const layout = make(); expect(layout.frame.panes).toHaveLength(0);
  layout.open(file("one")); const first = layout.frame.focusedLeaf!;
  layout.open(file("two")); expect(layout.frame.panes).toHaveLength(1); expect(layout.frame.leaves).toHaveLength(2);
  expect(layout.frame.panes[0].tabs.map((tab) => tab.surfaceId)).toEqual(["one", "two"]);
  layout.open(file("one")); expect(layout.frame.focusedLeaf).toBe(first); expect(layout.frame.leaves).toHaveLength(2);
  layout.closeSurface("two"); expect(layout.frame.focusedSurfaceId).toBe("one");
  layout.close(first); expect(layout.frame.panes).toHaveLength(0); expect(layout.snapshot().routes).toEqual({});
});

test("mixed splits use the shared geometry, directional focus, ratio bounds, and local close policy", () => {
  const layout = make(); layout.open(terminal("left")); const left = layout.frame.focusedLeaf!;
  layout.split("horizontal", file("right")); const right = layout.frame.focusedLeaf!;
  expect(layout.frame.panes[0].rect).toEqual([0, 0, 498, 700]);
  expect(layout.frame.panes[1].rect).toEqual([502, 0, 498, 700]);
  layout.split("vertical", { id: "bottom", title: "Agent", kind: "agent" });
  expect(layout.frame.panes).toHaveLength(3);
  layout.focusDirection("up"); expect(layout.frame.focusedLeaf).toBe(right);
  layout.focusDirection("left"); expect(layout.frame.focusedLeaf).toBe(left);
  layout.focusDirection("right"); expect(layout.frame.focusedLeaf).toBe(right);
  const divider = layout.frame.dividers.find((d) => d.axis === "horizontal")!;
  layout.resize(divider.path, divider.gap, 3);
  expect(layout.frame.dividers.find((d) => d.axis === "horizontal")!.ratio).toBeCloseTo(0.9);
  layout.close(right); expect(layout.frame.focusedSurfaceId).toBe("bottom");
  expect(layout.frame.panes.map((p) => p.surfaceId)).toEqual(["left", "bottom"]);
});

test("saved split ratios, hidden tabs, focus, routes, and duplicate file view identities round trip", () => {
  const layout = make(); layout.open(file("one")); layout.open(file("two"));
  layout.split("horizontal", file("two"));
  const duplicateViews = layout.frame.leaves.filter((leaf) => leaf.surfaceId === "two");
  expect(new Set(duplicateViews.map((leaf) => leaf.leaf)).size).toBe(2);
  expect(layout.frame.panes[0].tabs.map((t) => t.surfaceId)).toEqual(["one", "two"]);
  expect(layout.frame.panes[1].tabs.map((t) => t.leaf)).toEqual([layout.frame.panes[1].leaf]);
  const divider = layout.frame.dividers[0]; layout.resize(divider.path, divider.gap, 0.63);
  const saved = layout.snapshot(); const restored = make(JSON.parse(JSON.stringify(saved)));
  expect(restored.frame.tree).toEqual(layout.frame.tree);
  expect(restored.frame.panes).toEqual(layout.frame.panes);
  expect(restored.frame.focusedSurfaceId).toBe("two");
  layout.rename({ ...file("two"), title: "renamed.rs" });
  expect(layout.frame.leaves.filter((l) => l.surfaceId === "two").map((l) => l.title)).toEqual(["renamed.rs", "renamed.rs"]);
  expect(saved.routes).not.toEqual(layout.snapshot().routes);
});

test("daemon snapshots preserve recursive splits, tab stacks, paths, and the visible focus", () => {
  const layout = make();
  const leaf = (id: number, path?: string) => ({ kind: "leaf", pane_external_id: id, surface_id: `surface:${id}`, session_id: `session:${id}`, path: path || null });
  const snapshot = { schema_version: 1, workspace_id: "work", focused_pane_external_id: 30, root: {
    kind: "split", axis: "Horizontal", ratios: [0.7], children: [leaf(10), { kind: "tabs", active: 0, children: [leaf(20, "/project/a.rs"), leaf(30, "/project/b.rs")] }],
  } };
  const metadata: string[] = [];
  expect(layout.restoreDaemon(JSON.stringify(snapshot), (id, path, leaf) => {
    metadata.push(leaf.session_id);
    return path ? { id: path, title: path.split("/").pop()!, kind: "file" } : terminal(String(id));
  })).toBe(true);
  expect(metadata).toEqual(["session:10", "session:20", "session:30"]);
  expect(layout.frame.panes.map((p) => p.surfaceId)).toEqual(["10", "/project/b.rs"]);
  expect(layout.frame.focusedSurfaceId).toBe("/project/b.rs");
  expect(layout.frame.panes[1].tabs.map((t) => t.surfaceId)).toEqual(["/project/a.rs", "/project/b.rs"]);
  expect(layout.frame.dividers[0].ratio).toBeCloseTo(0.7);
});

test("drag-to-split and center adoption keep leaf identity and reject a self drop", () => {
  const layout = make(); layout.open(terminal("a")); const a = layout.frame.focusedLeaf!;
  layout.split("horizontal", file("b")); const b = layout.frame.focusedLeaf!;
  expect(layout.dropPreview(510, 350).placement).toBe("left");
  layout.drop(a, 990, 350); expect(layout.frame.panes.map((p) => p.leaf)).toEqual([b, a]);
  layout.drop(a, 100, 350); expect(layout.frame.panes.map((p) => p.leaf)).toEqual([a, b]);
  layout.drop(a, 750, 350); expect(layout.frame.panes).toHaveLength(1);
  expect(layout.frame.focusedLeaf).toBe(a); expect(layout.frame.leaves.map((l) => l.leaf)).toContain(a);
  const before = layout.snapshot(); layout.drop(a, 500, 350); expect(layout.snapshot()).toEqual(before);
});

test("unresolvable or invalid daemon restore leaves the current workspace intact", () => {
  const layout = make(); layout.open(file("keep")); const before = layout.snapshot();
  expect(layout.restoreDaemon({ root: { kind: "leaf", pane_external_id: 20 } }, () => undefined)).toBe(false);
  expect(() => layout.restoreDaemon({ root: { kind: "leaf", pane_external_id: 20 } }, () => file("invalid"))).toThrow();
  expect(layout.snapshot()).toEqual(before);
});

for (const [placement, x, y, axis, before] of [
  ["left", 10, 350, "horizontal", true],
  ["right", 990, 350, "horizontal", false],
  ["top", 500, 80, "vertical", true],
  ["bottom", 500, 690, "vertical", false],
] as const) {
  test(`selected tab tears out of its own stack to the ${placement}`, () => {
    const layout = make(); layout.open(file("one")); layout.open(file("two")); layout.open(file("three"));
    const source = layout.frame.focusedLeaf!, ids = layout.frame.leaves.map(tab => tab.leaf).sort();
    const preview = layout.dropPreview(x, y, source);
    expect(preview.placement).toBe(placement);
    expect(preview.target).not.toBe(source);
    layout.drop(source, x, y);
    expect(layout.frame.panes).toHaveLength(2);
    expect(layout.frame.dividers[0].axis).toBe(axis);
    expect(layout.frame.panes[before ? 0 : 1].leaf).toBe(source);
    expect(layout.frame.panes[before ? 1 : 0].tabs.map(tab => tab.surfaceId)).toEqual(["one", "two"]);
    expect(layout.frame.focusedLeaf).toBe(source);
    expect(layout.frame.leaves.map(tab => tab.leaf).sort()).toEqual(ids);
    expect(make(layout.snapshot()).frame.tree).toEqual(layout.frame.tree);
  });
}

test("self center and single-tab edge drops have no preview and leave the tree intact", () => {
  const layout = make(); layout.open(file("one"));
  const source = layout.frame.focusedLeaf!, saved = layout.snapshot();
  for (const [x, y] of [[10, 350], [990, 350], [500, 80], [500, 690], [500, 350]]) {
    expect(layout.dropPreview(x, y, source).rect).toBeNull();
    layout.drop(source, x, y);
    expect(layout.snapshot()).toEqual(saved);
  }
  layout.open(file("two"));
  const selected = layout.frame.focusedLeaf!, tabs = layout.snapshot();
  expect(layout.dropPreview(500, 350, selected).rect).toBeNull();
  layout.drop(selected, 500, 350);
  expect(layout.snapshot()).toEqual(tabs);
});
