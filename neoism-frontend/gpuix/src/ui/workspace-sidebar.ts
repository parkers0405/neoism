export type WorkspaceSidebar = "files" | "conversations" | "notes";
export type WorkspaceSidebars = Record<string, WorkspaceSidebar | null>;
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const choice = (value: unknown): WorkspaceSidebar | null | undefined => value === null || value === "files" || value === "conversations" || value === "notes" ? value : undefined;

/** The legacy window-wide setting belongs only to the first restored workspace.
 * Once a map exists, an unseen workspace gets its own Files default. */
export function workspaceSidebar(ui: Record<string, unknown>, workspaceId?: string): WorkspaceSidebar | undefined {
  const stored = record(ui.workspaceSidebars) ? choice(workspaceId ? ui.workspaceSidebars[workspaceId] : undefined) : choice(ui.sidebar);
  return stored === null ? undefined : stored ?? "files";
}

export function initialWorkspaceSidebars(ui: Record<string, unknown>, workspaceId?: string): WorkspaceSidebars | undefined {
  if (!workspaceId || record(ui.workspaceSidebars)) return;
  const previous = choice(ui.sidebar);
  return { [workspaceId]: previous === undefined ? "files" : previous };
}

export function withWorkspaceSidebar(ui: Record<string, unknown>, workspaceId: string, value: WorkspaceSidebar | undefined): WorkspaceSidebars {
  const stored = record(ui.workspaceSidebars) ? ui.workspaceSidebars : {};
  return { ...stored, [workspaceId]: value ?? null } as WorkspaceSidebars;
}
