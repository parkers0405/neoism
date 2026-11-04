import * as Effect from "effect/Effect";
import type { DaemonConnection } from "@neoism/service-client/client";
import { expectVariant, isRecord, variant, type SearchClientMessage } from "@neoism/service-client/protocol";

export type FinderMode = "files" | "grep";
export type FinderMatchMode = "Fuzzy" | "Exact" | "Regex";
/** Same coordinates as daemon/GUI grep: 1-based line, 0-based UTF-8 byte column. */
export interface FinderResult { path: string; line?: number; col?: number; text?: string }
export interface FinderState {
  root: string;
  query: string;
  mode: FinderMode;
  matchMode: FinderMatchMode;
  results: readonly FinderResult[];
  loading: boolean;
  /** Only real SearchProgress counts; undefined when the server emits none. */
  progress?: number;
  error?: string;
}
type SearchClient = Pick<DaemonConnection, "send" | "subscribe" | "subscribeStatus">;

/** Pure GUI selectedTarget path policy; no host filesystem/path resolution. */
function relativePath(path: string, root: string): string {
  path = path.replaceAll("\\", "/");
  const prefix = root.replaceAll("\\", "/").replace(/\/$/, "") + "/";
  const inside = /^[a-z]:/i.test(prefix)
    ? path.toLowerCase().startsWith(prefix.toLowerCase())
    : path.startsWith(prefix);
  return inside ? path.slice(prefix.length) : path;
}

export function normalizeFinderResults(reply: unknown, root: string, mode: FinderMode): FinderResult[] {
  const body = expectVariant(reply, mode === "files" ? "SearchFilesResult" : "SearchGrepResult");
  if (!isRecord(body) || !Array.isArray(body.hits)) throw new Error("Invalid search response");
  return body.hits.flatMap((hit): FinderResult[] => {
    if (!isRecord(hit) || typeof hit.path !== "string" || !hit.path) return [];
    const path = relativePath(hit.path, root);
    if (mode === "files") return [{ path }];
    if (!Number.isSafeInteger(hit.line) || (hit.line as number) < 1 ||
        !Number.isSafeInteger(hit.column) || (hit.column as number) < 0 || typeof hit.text !== "string") return [];
    return [{ path, line: hit.line as number, col: hit.column as number, text: hit.text }];
  });
}

/** GPU-free popup data controller. The parent owns keys, selection and opening.
 * GUI Alt+S / Ctrl+P open files; Alt+P opens the command palette, NOT grep.
 * Key dispatch and the command palette remain the parent's responsibility.
 * search() mirrors native Finder's 75ms file / 220ms grep debounce, empty file
 * listing, two-character grep minimum (except Regex), and Tab mode cycle.
 * All work runs through the existing shared daemon connection, never locally.
 */
export class TuiFinder {
  state: FinderState = { root: "", query: "", mode: "files", matchMode: "Fuzzy", results: [], loading: false };
  private listeners = new Set<(state: FinderState) => void>();
  private timer?: ReturnType<typeof setTimeout>;
  private abort?: AbortController;
  private generation = 0;
  private disposed = false;

  constructor(private readonly client: SearchClient, onChange?: (state: FinderState) => void) {
    if (onChange) this.listeners.add(onChange);
  }

  subscribe(listener: (state: FinderState) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private publish(patch: Partial<FinderState>) {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener(this.state);
  }

  search(root: string, query: string, mode: FinderMode = "files", matchMode: FinderMatchMode = "Fuzzy"): void {
    if (this.disposed) return;
    this.invalidate();
    // Files support only fuzzy/exact, just like the GUI's two-state cycle.
    if (mode === "files" && matchMode === "Regex") matchMode = "Fuzzy";
    const generation = this.generation;
    const key = matchMode === "Fuzzy" ? query.trim().split(/\s+/u).join(" ") : query.trimEnd();
    const eligible = mode === "files" || (key.trim().length > 0 &&
      (matchMode === "Regex" || Array.from(key.trim()).length >= 2));
    this.publish({ root, query, mode, matchMode, results: [], loading: eligible, progress: undefined, error: undefined });
    if (!eligible || generation !== this.generation || this.disposed) return;
    const delay = mode === "files" ? (query === "" ? 0 : 75) : 220;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.execute(generation);
    }, delay);
  }

  cycle(): void {
    const { root, query, mode, matchMode } = this.state;
    const next = matchMode === "Fuzzy" ? "Exact" : matchMode === "Exact" && mode === "grep" ? "Regex" : "Fuzzy";
    this.search(root, query, mode, next);
  }

  private async execute(generation: number) {
    if (this.disposed || generation !== this.generation) return;
    const { root, query, mode, matchMode } = this.state;
    const controller = new AbortController();
    this.abort = controller;
    const message: SearchClientMessage = mode === "files"
      ? { SearchFiles: { req_id: 0, cwd: "", query, mode: matchMode === "Exact" ? "Exact" : "Fuzzy" } }
      : { SearchGrep: { req_id: 0, cwd: "", query, mode: matchMode, case_sensitive: null, file_patterns: [] } };
    const current = () => !this.disposed && generation === this.generation && !controller.signal.aborted;
    try {
      const reply = await Effect.runPromise(this.requestSearch(message, root, count => {
        if (current()) this.publish({ progress: count });
      }), { signal: controller.signal });
      if (current()) this.publish({ results: normalizeFinderResults(reply, root, mode), loading: false });
    } catch (error) {
      if (current()) this.publish({ loading: false, error: error instanceof Error ? error.message : String(error) });
    } finally {
      if (this.abort === controller) this.abort = undefined;
    }
  }

  /** The shared client's generic request() does not expose its assigned id.
   * Correlate progress with send()'s real id, not a guessed next id or query.
   * Effect interruption releases the subscription and best-effort CancelSearch.
   */
  private requestSearch(message: SearchClientMessage, root: string, progress: (count: number) => void) {
    return Effect.callback<unknown, Error>(resume => {
      let id: number | undefined;
      let finished = false;
      const unsubscribe = this.client.subscribe(event => {
        if (event.family !== "Search" || event.requestId !== id || finished) return;
        const entry = variant(event.message);
        if (!entry || !isRecord(entry[1])) return;
        if (entry[0] === "SearchProgress") {
          const count = entry[1].found_so_far;
          if (typeof count === "number" && Number.isSafeInteger(count) && count >= 0) progress(count);
          return;
        }
        finished = true;
        if (entry[0] === "SearchError" || entry[0] === "Error")
          resume(Effect.fail(new Error(typeof entry[1].message === "string" ? entry[1].message : "Search failed")));
        else resume(Effect.succeed(event.message));
      });
      try { id = this.client.send("Search", message, root); }
      catch (error) {
        finished = true;
        resume(Effect.fail(error instanceof Error ? error : new Error(String(error))));
      }
      const unsubscribeStatus = this.client.subscribeStatus(state => {
        if (!finished && state.phase !== "connected") {
          finished = true;
          resume(Effect.fail(new Error(state.reason || "Daemon is offline")));
        }
      });
      const timer = setTimeout(() => resume(Effect.fail(new Error("Search request timed out"))), 15_000);
      return Effect.sync(() => {
        clearTimeout(timer);
        unsubscribe();
        unsubscribeStatus();
        if (!finished && id !== undefined) {
          try { this.client.send("Search", { CancelSearch: { req_id: id } }, root); } catch {}
        }
      });
    });
  }

  private invalidate() {
    this.generation++;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.abort?.abort();
    this.abort = undefined;
  }

  cancel(): void {
    if (this.disposed) return;
    this.invalidate();
    this.publish({ loading: false });
  }

  dispose(): void {
    if (this.disposed) return;
    this.invalidate();
    this.disposed = true;
    this.state = { ...this.state, loading: false };
    this.listeners.clear();
  }
}