import type { AgentServerMessage, ProjectRootSummary } from "../workspace/types";
import type { ProtocolClient } from "../workspace/ProtocolClient";

export type NavigatorRow = { value: string; title: string; description: string; footer: string; is_current: boolean; is_disabled: boolean };
export type ConversationResult = NavigatorRow & { directory: string | null; metadata: Record<string, unknown> };
const object = (v: unknown): Record<string, unknown> => v && typeof v === "object" ? v as Record<string, unknown> : {};
const text = (v: unknown): string => typeof v === "string" ? v : "";

export function conversationSearchIndexing(semantic: unknown): boolean {
  return object(semantic).indexing === true;
}

/** Preserve full metadata independently of presentation and the visible catalog. */
export function conversationSearchRows(sessions: unknown, semantic: unknown): ConversationResult[] {
  const root = object(sessions);
  const titles = Array.isArray(sessions) ? sessions : root.items ?? root.sessions ?? root.data;
  const search = object(semantic);
  const hits = search.hits;
  const catalog = (Array.isArray(titles) ? titles : []).map(object);
  const metadataById = new Map(catalog.map((item) => [text(item.id), item]));
  // Ranked grouped results are authoritative, including an empty result. Titles
  // only provide metadata; they must not insert/reorder matches ahead of hits.
  const ranked = Array.isArray(hits) && search.available !== false;
  const results = new Map<string, ConversationResult>();
  for (const raw of ranked ? hits : catalog) {
    const item = object(raw);
    const id = text(item.sessionId) || text(item.session_id) || text(item.id);
    if (!id || results.has(id)) continue;
    const prior = metadataById.get(id);
    const metadata = { ...prior, ...item };
    const directory = text(metadata.directory) || null;
    const source = text(metadata.source) || text(metadata.externalProvider) || "Neoism";
    const project = directory || text(metadata.projectID) || text(metadata.project_id);
    results.set(id, { value: id, title: text(metadata.title) || "Untitled conversation",
      description: (text(metadata.snippet) || text(metadata.excerpt) || project).replace(/\s+/gu, " ").trim(),
      footer: [project, source, text(metadata.role)].filter(Boolean).join(" · "),
      is_current: false, is_disabled: false, directory, metadata });
  }
  return [...results.values()];
}

/** Lexical only: never resolve symlinks, parent segments, or infer a host root. */
function directoryKey(path: string): string {
  const windows = /^[a-zA-Z]:[\\/]/.test(path) || path.startsWith("\\\\");
  const separated = windows ? path.replace(/\\/g, "/") : path;
  const prefix = separated.startsWith("//") && windows ? "//" : separated.startsWith("/") ? "/" : "";
  return prefix + separated.split("/").filter((part) => part && part !== ".").join("/");
}

/** Only server-authorized summary items and the already-declared root enter this
 * union. Never derive directories from a bounded recent session list. */
export function mergeNavigatorProjects(registry: ProjectRootSummary[], directories: Array<{ directory: string; updatedMs: number }>): ProjectRootSummary[] {
  const seen = new Set<string>();
  const chats = directories.filter((item) => typeof item.directory === "string" && item.directory.length > 0).map((item) => ({
    id: `directory:${item.directory}`, path: item.directory,
    name: item.directory.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || item.directory,
    last_opened: item.updatedMs,
  }));
  return [...registry, ...chats].filter((project) => {
    if (!project.path || !(project.path.startsWith("/") || project.path.startsWith("\\\\") || /^[a-zA-Z]:[\\/]/.test(project.path))) return false;
    const key = directoryKey(project.path);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** For an HTTP-backed management registry transport only. Do not infer status
 * from arbitrary error messages (ProtocolClient's daemon registry is non-HTTP). */
export class ProjectRegistryHttpError extends Error {
  constructor(readonly status: number, message: string) {
    super(`HTTP ${status}: ${message}`);
    this.name = "ProjectRegistryHttpError";
  }
}

export type NavigatorProjectDiscoveryState = {
  projects: ProjectRootSummary[];
  registry: "loading" | "ready" | "unavailable" | "error";
  chats: "loading" | "ready" | "error";
  errors: Partial<Record<"registry" | "chats", string>>;
};
let discoverySerial = 0;

/** Both requests stay on the captured authenticated client. The owner forwards
 * agent replies to accept() and calls close() on dismissal/context changes. */
export class NavigatorProjectDiscovery {
  private readonly identity: string;
  private readonly connectionGeneration: number;
  private readonly serial = ++discoverySerial;
  private readonly catalogIdentity: string;
  private registryRows: ProjectRootSummary[] = [];
  private chatRows: Array<{ directory: string; updatedMs: number }> = [];
  private state: NavigatorProjectDiscoveryState = { projects: [], registry: "loading", chats: "loading", errors: {} };
  private timer: ReturnType<typeof setTimeout> | null = null;
  private active = true;
  private started = false;

  constructor(private readonly client: Pick<ProtocolClient, "requestProjectRoots" | "sendAgent" | "allocateRequestId" | "agentCatalogIdentity" | "getGeneration">,
    private readonly workspaceId: string | null,
    private readonly onChange: (state: NavigatorProjectDiscoveryState) => void) {
    this.identity = client.agentCatalogIdentity(workspaceId);
    this.connectionGeneration = client.getGeneration();
    this.catalogIdentity = JSON.stringify([this.identity, "project-picker", this.serial]);
  }

  private current(): boolean {
    return this.active && this.identity === this.client.agentCatalogIdentity(this.workspaceId)
      && this.connectionGeneration === this.client.getGeneration();
  }
  private publish(): void {
    if (this.current()) this.onChange({ ...this.state, errors: { ...this.state.errors },
      projects: mergeNavigatorProjects(this.registryRows, this.chatRows) });
  }
  private fail(source: "registry" | "chats", error: unknown): void {
    if (!this.current() || this.state[source] !== "loading") return;
    if (source === "chats" && this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (source === "registry" && error instanceof ProjectRegistryHttpError && error.status === 404) {
      this.registryRows = [];
      this.state.registry = "unavailable";
      this.publish();
      return;
    }
    this.state[source] = "error";
    this.state.errors[source] = String(error);
    this.publish();
  }
  start(): void {
    if (this.started || !this.current()) return;
    this.started = true;
    // No directory/cursor/limit is sent to global summary discovery.
    this.timer = setTimeout(() => this.fail("chats", "Saved chat projects request timed out"), 30000);
    try {
      this.client.sendAgent(this.client.allocateRequestId(), { ListSessionProjects: {
        catalog_identity: this.catalogIdentity, generation: this.serial,
      } });
    } catch (error) { this.fail("chats", error); }
    void Promise.resolve().then(() => this.client.requestProjectRoots()).then((rows) => {
      if (!this.current()) return;
      this.registryRows = rows;
      this.state.registry = "ready";
      this.publish();
    }).catch((error) => this.fail("registry", error));
  }
  accept(payload: AgentServerMessage): boolean {
    if (!this.current() || typeof payload !== "object" || payload === null) return false;
    const success = "SessionProjects" in payload ? payload.SessionProjects : null;
    const failure = "SessionProjectsFailed" in payload ? payload.SessionProjectsFailed : null;
    const reply = success ?? failure;
    if (!reply || reply.catalog_identity !== this.catalogIdentity || reply.generation !== this.serial) return false;
    if (this.state.chats !== "loading") return true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    if (failure) this.fail("chats", failure.message);
    else if (success) {
      if (!Array.isArray(success.items) || success.items.some((item) => !item || typeof item.directory !== "string" || typeof item.updatedMs !== "number")) {
        this.fail("chats", "Malformed saved chat projects response");
        return true;
      }
      this.chatRows = success.items;
      this.state.chats = "ready";
      this.publish();
    }
    return true;
  }
  close(): void {
    this.active = false;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }
}

export const ADD_NAVIGATOR_WORKSPACE = "navigator:add-workspace";

/** Mirrors catalog dashboard scope using observed/authorized roots, not cwd. */
export function canAddNavigatorWorkspace(currentRoot: string | null, homePath: string | null): boolean {
  return isObservedFilesystemRoot(currentRoot)
    || Boolean(currentRoot && homePath && directoryKey(currentRoot) === directoryKey(homePath));
}

/** Authorized persisted directories; selection changes cwd of the SAME workspace. */
export function projectNavigatorRows(projects: ProjectRootSummary[], currentRoot: string | null, homePath: string | null): NavigatorRow[] {
  const home = homePath ? directoryKey(homePath) : null;
  const current = currentRoot ? directoryKey(currentRoot) : null;
  const seen = new Set<string>(home ? [home] : []);
  const candidates = mergeNavigatorProjects(projects, currentRoot ? [{ directory: currentRoot, updatedMs: 0 }] : []);
  const saved = candidates.filter((p) => {
    const key = directoryKey(p.path);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  }).sort((a, b) => {
    const an = (a.name || a.path).toLowerCase(), bn = (b.name || b.path).toLowerCase();
    return an < bn ? -1 : an > bn ? 1 : a.path < b.path ? -1 : a.path > b.path ? 1 : 0;
  }).map((p) => ({ value: p.path, title: p.name || p.path, description: p.path,
    footer: "", is_current: directoryKey(p.path) === current, is_disabled: false }));
  saved.sort((a, b) => Number(b.is_current) - Number(a.is_current));
  const actions: NavigatorRow[] = canAddNavigatorWorkspace(currentRoot, homePath) ? [{
    value: ADD_NAVIGATOR_WORKSPACE, title: "Add workspace…", description: "Choose a project folder", footer: "", is_current: false, is_disabled: false,
  }] : [];
  return [{ value: homePath ?? "", title: "~", description: homePath || "Home/root directory unavailable for this connection",
    footer: home ? "Dashboard" : "Unavailable", is_current: home !== null && current === home, is_disabled: !home }, ...saved, ...actions];
}

export function isObservedFilesystemRoot(path: string | null | undefined): path is string {
  return path === "/" || (typeof path === "string" && /^[a-zA-Z]:[\\/]$/.test(path));
}

export type DashboardRootBinding = { client: unknown; identity: string; root: string | null; lookupGeneration: number | null; lookupComplete: boolean };

/** Permission cache, not a cwd history: an arbitrary project is never home. */
export class AuthorizedDashboardRootCache {
  private current: DashboardRootBinding | null = null;

  bind(client: unknown, identity: string, observedRoot: string | null | undefined): DashboardRootBinding {
    if (!this.current || this.current.client !== client || this.current.identity !== identity) {
      this.current = { client, identity, root: null, lookupGeneration: null, lookupComplete: false };
    }
    if (!this.current.root && isObservedFilesystemRoot(observedRoot)) this.current.root = observedRoot;
    return this.current;
  }

  isCurrent(binding: DashboardRootBinding): boolean { return this.current === binding; }

  acceptHome(binding: DashboardRootBinding, home: string | null | undefined): boolean {
    if (!this.isCurrent(binding) || !home?.trim()) return false;
    binding.root = home;
    return true;
  }
}

export function navigatorContextCurrent(captured: { client: unknown; identity: string; generation: number; workspaceId: string | null }, current: { client: unknown; identity: string; generation: number; workspaceId: string | null }): boolean {
  return captured.client === current.client && captured.identity === current.identity && captured.generation === current.generation && captured.workspaceId === current.workspaceId;
}
