// JS-side bridge for `neoism-ui::services::SearchService` (the Rust
// `JsSearchService` shim on the wasm side).
//
// The desktop binary runs `rg`, the `fff_search::FilePicker`, and
// `git status --porcelain` directly on the host. The web frontend has
// no shell-out at all, so the daemon runs those tools and pipes hits
// back over the WebSocket. Mirrors the wire-side shape in
// `neoism-protocol/src/search.rs`.
//
// Wire shape (mirrors the daemon's eventual `ServiceServerMessage`
// routing):
//
//   inbound   ServiceServerMessage::SearchReply { req_id-derived
//             routing -> SearchServerMessage }
//   outbound  ServiceClientMessage::Search    { request_id, message }
//
// The wasm chrome exposes a `set_search_*` setter for each search
// flavour. Each callback receives a `(reqId, jsonArgs)` pair the JS
// host parses into a typed `SearchClientMessage`. On reply arrival
// (`ProtocolClient.onSearchReply`), we look up `req_id` in our pending
// table and either:
//   - resolve a promise (for direct JS callers), OR
//   - call `bridge.service_reply(reqId, payload)` so the wasm side's
//     `JsSearchService` pending-slot wakes the panel that issued the
//     request via `IoError::Pending(req_id)`.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type {
  SearchClientMessage,
  SearchFileMode,
  SearchGrepMode,
  SearchServerMessage,
} from "../workspace/types";

export type {
  SearchClientMessage,
  SearchFileMode,
  SearchGrepMode,
  SearchServerMessage,
};

/**
 * Subset of the chrome bridge surface that `SearchService` needs.
 * Spelled out as a separate interface so the service can be built and
 * unit-tested without depending on the full `ChromeBridgeInstance`
 * shape from `createTerminal.ts`.
 */
export interface SearchBridge {
  /// Reply hook — pushes the parsed JSON payload back into the wasm
  /// side's `JsSearchService` so the panel that surfaced
  /// `IoError::Pending(req_id)` can resume.
  serviceReply?(requestId: number, payload: unknown): void;
  /// Install the JS-side callback the bridge fires when chrome wants
  /// `rg --files` rooted at `cwd`.
  setSearchCollectFiles?(cb: (reqId: number, envelopeJson: string) => void): void;
  /// Install the JS-side callback for fuzzy/exact file picker.
  setSearchFiles?(cb: (reqId: number, envelopeJson: string) => void): void;
  /// Install the JS-side callback for fuzzy directory-only search.
  setSearchDirectories?(cb: (reqId: number, envelopeJson: string) => void): void;
  /// Install the JS-side callback for `rg <query>` (or fuzzy/regex).
  setSearchGrep?(cb: (reqId: number, envelopeJson: string) => void): void;
  /// Install the JS-side callback for `git status --porcelain` rows.
  setSearchGitChanges?(cb: (reqId: number, envelopeJson: string) => void): void;
  /// Install the JS-side callback for `git rev-parse --show-toplevel`.
  setSearchGitRepoRoot?(cb: (reqId: number, envelopeJson: string) => void): void;
  /// Install the JS-side callback for "drop the in-flight search".
  setSearchCancel?(cb: (reqId: number) => void): void;
}

/**
 * Build a `SearchService` and install all `set_search_*` callbacks on
 * `bridge`. Returns the service handle so callers can call `dispose()`
 * on shutdown (idempotent — the bridge holds the callbacks, no
 * explicit teardown today).
 *
 * `bridge`'s optional methods may be absent if the wasm bundle hasn't
 * been rebuilt with the search surface yet; install is best-effort.
 * The protocol-client subscription is installed unconditionally so
 * any promise-based callers still resolve.
 */
export class SearchService {
  /**
   * Pending replies keyed on `req_id`. The same id is shared between
   * direct promise callers and panels that route through
   * `bridge.service_reply` (the wasm side correlates with its own
   * `JsSearchService` pending table).
   */
  private readonly pending = new Map<
    number,
    {
      resolve: (msg: SearchServerMessage) => void;
      reject: (err: Error) => void;
    }
  >();

  /**
   * Highest req_id we've handed out for the promise-based path. We
   * leave the bottom of the u64 space (0 - 0x3fff_ffff) for the wasm
   * bridge to allocate. JS-only callers start above that to avoid
   * collisions.
   */
  private nextReqId = 0x6000_0000;

  constructor(
    private readonly client: ProtocolClient,
    private readonly bridge: SearchBridge,
  ) {}

  /**
   * Install the bridge-side callbacks so chrome searches route through
   * the daemon WebSocket. Idempotent — calling multiple times just
   * replaces the most recently installed callback (wasm-bindgen
   * semantics).
   */
  install(): void {
    this.bridge.setSearchCollectFiles?.((reqId, envelopeJson) => {
      this.client.sendSearch(parseSearchEnvelope(reqId, envelopeJson));
    });
    this.bridge.setSearchFiles?.((reqId, envelopeJson) => {
      this.client.sendSearch(parseSearchEnvelope(reqId, envelopeJson));
    });
    this.bridge.setSearchDirectories?.((reqId, envelopeJson) => {
      this.client.sendSearch(parseSearchEnvelope(reqId, envelopeJson));
    });
    this.bridge.setSearchGrep?.((reqId, envelopeJson) => {
      this.client.sendSearch(parseSearchEnvelope(reqId, envelopeJson));
    });
    this.bridge.setSearchGitChanges?.((reqId, envelopeJson) => {
      this.client.sendSearch(parseSearchEnvelope(reqId, envelopeJson));
    });
    this.bridge.setSearchGitRepoRoot?.((reqId, envelopeJson) => {
      this.client.sendSearch(parseSearchEnvelope(reqId, envelopeJson));
    });
    this.bridge.setSearchCancel?.((reqId) => {
      this.client.sendSearch({ CancelSearch: { req_id: reqId } });
      // Best-effort: settle the in-flight promise with an error so
      // JS callers don't hang waiting on a cancelled request.
      const slot = this.pending.get(reqId);
      if (slot) {
        this.pending.delete(reqId);
        slot.reject(new Error("search cancelled"));
      }
    });
  }

  /**
   * Route an inbound `SearchServerMessage` payload from
   * `ProtocolClient.onSearchReply`. Resolves the matching pending
   * promise (if any) and forwards via `bridge.service_reply` so the
   * wasm panel's `IoError::Pending(req_id)` slot wakes up.
   */
  ingestServerMessage(message: SearchServerMessage): void {
    const reqId = extractReqId(message);
    if (reqId === null) return;
    // Progress frames are intermediate — keep the pending slot alive
    // and only forward to the bridge (don't resolve the promise).
    if ("SearchProgress" in message) {
      this.bridge.serviceReply?.(reqId, message);
      return;
    }
    const slot = this.pending.get(reqId);
    if (slot) {
      this.pending.delete(reqId);
      if ("SearchError" in message) {
        slot.reject(new Error(message.SearchError.message));
      } else {
        slot.resolve(message);
      }
    }
    this.bridge.serviceReply?.(reqId, message);
  }

  /**
   * Promise-based send for callers that don't go through the chrome
   * bridge. Allocates a fresh `req_id`, fires the request, and
   * resolves with the matching `SearchServerMessage`. Rejects with
   * the error message on `SearchError` or on `CancelSearch`.
   */
  request(
    factory: (reqId: number) => SearchClientMessage,
  ): Promise<SearchServerMessage> {
    const reqId = this.nextReqId++;
    return new Promise<SearchServerMessage>((resolve, reject) => {
      this.pending.set(reqId, { resolve, reject });
      try {
        this.client.sendSearch(factory(reqId));
      } catch (err) {
        this.pending.delete(reqId);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  }
}

function parseSearchEnvelope(reqId: number, envelopeJson: string): SearchClientMessage {
  const parsed = JSON.parse(envelopeJson) as SearchClientMessage;
  if ("CollectFiles" in parsed) {
    return { CollectFiles: { ...parsed.CollectFiles, req_id: reqId } };
  }
  if ("SearchFiles" in parsed) {
    return {
      SearchFiles: {
        ...parsed.SearchFiles,
        req_id: reqId,
        mode: coerceFileMode(parsed.SearchFiles.mode),
      },
    };
  }
  if ("SearchDirectories" in parsed) {
    return {
      SearchDirectories: { ...parsed.SearchDirectories, req_id: reqId },
    };
  }
  if ("SearchGrep" in parsed) {
    return {
      SearchGrep: {
        ...parsed.SearchGrep,
        req_id: reqId,
        mode: coerceGrepMode(parsed.SearchGrep.mode),
      },
    };
  }
  if ("SearchGitChanges" in parsed) {
    return { SearchGitChanges: { ...parsed.SearchGitChanges, req_id: reqId } };
  }
  if ("GitRepoRoot" in parsed) {
    return { GitRepoRoot: { ...parsed.GitRepoRoot, req_id: reqId } };
  }
  if ("CancelSearch" in parsed) {
    return { CancelSearch: { req_id: reqId } };
  }
  throw new Error("unknown search envelope");
}

function coerceFileMode(raw: string): SearchFileMode {
  return raw === "Exact" ? "Exact" : "Fuzzy";
}

function coerceGrepMode(raw: string): SearchGrepMode {
  if (raw === "Exact") return "Exact";
  if (raw === "Regex") return "Regex";
  return "Fuzzy";
}

function extractReqId(message: SearchServerMessage): number | null {
  if (!message || typeof message !== "object") return null;
  for (const inner of Object.values(message as Record<string, unknown>)) {
    if (inner && typeof inner === "object" && "req_id" in inner) {
      const id = (inner as { req_id: unknown }).req_id;
      if (typeof id === "number") return id;
    }
  }
  return null;
}
