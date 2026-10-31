import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type { AppConfig } from "./config";
import type { Surface } from "./model";

export interface SavedSession {
  version: 1;
  activeWorkspaceId?: string;
  activeSurfaceId?: string;
  surfaces: Surface[];
  editors: Record<string, unknown>;
  providerStates: Record<string, unknown>;
  drafts: Record<string, unknown>;
  composerDrafts: Record<string, { text: string; attachments?: unknown[] }>;
  ui: Record<string, unknown>;
}
const record = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
// Hot remounts and server profiles can briefly have separate stores for one
// path. Keep their read/merge/replace transactions in order too.
const pendingWrites = new Map<string, Promise<void>>();
export function decodeSession(value: unknown): SavedSession | undefined {
  if (!record(value) || value.version !== 1 || !Array.isArray(value.surfaces)) return;
  const surfaces = value.surfaces.filter((s): s is Surface => record(s) && typeof s.id === "string" && typeof s.title === "string"
    && typeof s.workspaceId === "string" && ["file", "terminal", "agent", "page", "welcome", "git"].includes(String(s.kind))
    && (s.kind !== "file" || typeof s.path === "string") && (s.kind !== "page" || s.page === "extensions")
    && (s.kind !== "git" || record(s.git) && typeof s.git.root === "string" && ["working","commit","pull-request","pull-request-inbox","pull-request-create"].includes(String(s.git.mode)))
    && (s.kind === "file" || s.kind === "page" || s.kind === "welcome" || s.kind === "git" || (s.kind === "agent" && s.sessionId === undefined) || typeof s.sessionId === "string"));
  const composerDrafts: SavedSession["composerDrafts"] = {};
  if (record(value.composerDrafts)) for (const [key, draft] of Object.entries(value.composerDrafts)) {
    if (record(draft) && typeof draft.text === "string") composerDrafts[key] = { text: draft.text, ...(Array.isArray(draft.attachments) ? { attachments: draft.attachments } : {}) };
  }
  return { version: 1, surfaces, editors: record(value.editors) ? value.editors : {},
    providerStates: record(value.providerStates) ? value.providerStates : {}, drafts: record(value.drafts) ? value.drafts : {}, composerDrafts,
    ui: record(value.ui) ? value.ui : {}, activeWorkspaceId: typeof value.activeWorkspaceId === "string" ? value.activeWorkspaceId : undefined,
    activeSurfaceId: typeof value.activeSurfaceId === "string" ? value.activeSurfaceId : undefined };
}

/** Separate GPUIX recovery files never overwrite desktop layout/config files. */
export class SessionStore {
  readonly path: string;
  private readonly identity: string;
  private readonly profiles: boolean;
  private last = "";
  constructor(config: Pick<AppConfig, "endpoint" | "authToken" | "pairingToken" | "transportIdentity">, env = process.env) {
    // Scope recovery to the endpoint and authenticated identity without writing credentials.
    this.identity = createHash("sha256").update(JSON.stringify(config.transportIdentity ? [config.transportIdentity] : [config.endpoint, config.pairingToken || config.authToken || ""])).digest("hex").slice(0, 24);
    this.profiles = !!env.NEOISM_GPUIX_STATE_PATH;
    this.path = env.NEOISM_GPUIX_STATE_PATH || join(env.XDG_STATE_HOME || join(homedir(), ".local/state"), "neoism/gpuix", `${this.identity}.json`);
  }
  read(): SavedSession | undefined {
    if (!existsSync(this.path)) return;
    try {
      const raw = readFileSync(this.path, "utf8"), value: unknown = JSON.parse(raw);
      if (!record(value)) return;
      const profile = this.profiles && record(value.profiles) ? decodeSession(value.profiles[this.identity]) : undefined;
      const session = profile || (value.identity === undefined || value.identity === this.identity ? decodeSession(value) : undefined);
      if (session) this.last = raw;
      return session;
    }
    catch { return; }
  }
  private encode(session: SavedSession, previous: Record<string, unknown>) {
    const profiles: Record<string, SavedSession> = {};
    if (this.profiles) {
      if (record(previous.profiles)) for (const [identity, value] of Object.entries(previous.profiles)) {
        const saved = decodeSession(value); if (/^[0-9a-f]{24}$/.test(identity) && saved) profiles[identity] = saved;
      }
      const old = decodeSession(previous);
      if (old && typeof previous.identity === "string" && /^[0-9a-f]{24}$/.test(previous.identity)) profiles[previous.identity] = old;
      profiles[this.identity] = session;
    }
    // Keep the current snapshot at the top level for existing CLI/debug tools.
    // An explicit shared path additionally retains every server's private state.
    return JSON.stringify({ ...session, identity: this.identity, ...(this.profiles ? { profiles } : {}) });
  }
  write(session: SavedSession) {
    let previous: Record<string, unknown> = {};
    if (this.profiles && existsSync(this.path)) try { const value: unknown = JSON.parse(readFileSync(this.path, "utf8")); if (record(value)) previous = value; } catch {}
    const raw = this.encode(session, previous);
    if (raw === this.last) return;
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    const pending = `${this.path}.${process.pid}.next`;
    writeFileSync(pending, raw, { mode: 0o600 }); chmodSync(pending, 0o600);
    renameSync(pending, this.path); this.last = raw;
  }
  /** Serialize atomic replacements without blocking input/paint on filesystem
   * latency. A failed write must not prevent subsequent recovery attempts. */
  writeAsync(session: SavedSession): Promise<void> {
    const next = (pendingWrites.get(this.path) || Promise.resolve()).then(async () => {
      let previous: Record<string, unknown> = {};
      if (this.profiles) try { const value: unknown = JSON.parse(await readFile(this.path, "utf8")); if (record(value)) previous = value; } catch {}
      const raw = this.encode(session, previous);
      if (raw === this.last) return;
      await mkdir(dirname(this.path), { recursive: true, mode: 0o700 });
      const pending = `${this.path}.${process.pid}.next`;
      await writeFile(pending, raw, { mode: 0o600 });
      await chmod(pending, 0o600);
      await rename(pending, this.path);
      this.last = raw;
    });
    const settled = next.catch(() => {}).finally(() => {
      if (pendingWrites.get(this.path) === settled) pendingWrites.delete(this.path);
    });
    pendingWrites.set(this.path, settled);
    return next;
  }
}
