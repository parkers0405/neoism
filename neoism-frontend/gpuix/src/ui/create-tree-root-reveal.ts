import { createComputed, createSignal, onCleanup, untrack, type Accessor } from "solid-js";
import { TreeRootReveal, type TreeRevealIdentity, type TreeRevealInput } from "./tree-root-reveal";

const identities = new WeakMap<object, Map<string, TreeRevealIdentity>>();

/** Call once per mounted Tree. `owner` must be the window's AppVisualState,
 * not a global model, so font/theme generations don't replay settled rows. */
export function createTreeRootReveal(owner: object, panel: string, input: Accessor<TreeRevealInput>, scale: Accessor<number>) {
  let panels = identities.get(owner);
  if (!panels) { panels = new Map(); identities.set(owner, panels); }
  let identity = panels.get(panel);
  if (!identity) { identity = {}; panels.set(panel, identity); }
  const [revision, setRevision] = createSignal(0);
  const reveal = new TreeRootReveal(identity, () => setRevision(value => value + 1));
  createComputed(() => { const next = input(); untrack(() => reveal.update(next)); });
  onCleanup(() => reveal.dispose());
  return (index: number) => { revision(); return reveal.row(index, scale()); };
}