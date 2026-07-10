/** A daemon filesystem push invalidates only the tree rooted at that daemon path. */
export function shouldRefreshFileTree(
  changedRoot: string,
  activeWorkspaceRoot: string | null | undefined,
): boolean {
  return !!activeWorkspaceRoot && changedRoot === activeWorkspaceRoot;
}