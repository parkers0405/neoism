import type { LayoutFrame, LayoutPane } from "./WorkspaceLayout";

type Node = { Split?: { children: Node[] }; Tabbed?: { children: Node[] } };

/** A visible pane belongs to a split position, independent of which buffer
 * leaf is selected inside its tab stack. Native leaf paths include both kinds
 * of child index; retaining a tab index here would remount the whole strip. */
export function paneHostKey(frame: Pick<LayoutFrame, "tree">, pane: Pick<LayoutPane, "path">): string {
  let node = (frame.tree as { root?: Node } | null)?.root;
  const splits: number[] = [];
  for (const index of pane.path) {
    if (node?.Split) { splits.push(index); node = node.Split.children[index]; }
    else if (node?.Tabbed) node = node.Tabbed.children[index];
    else return `path:${pane.path.join(".")}`;
  }
  return `split:${splits.join(".")}`;
}
