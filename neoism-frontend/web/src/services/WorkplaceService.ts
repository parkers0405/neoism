// Multi-daemon "workplace" registry — combined wave 6 + 12-F + 13-F.
//
// The full multi-daemon switcher consists of three layers, all glued
// together here so `App.ts` has a single entry point:
//
//   * The **registry** (`Map<DaemonId, WorkplaceEntry>`) — every
//     workplace the operator has either configured manually or promoted
//     from a Tailscale discovery hit. URLs (and labels) are persisted to
//     `localStorage`; pairing tokens are **never** written to disk per
//     the wave 12-G threat model — they are kept in memory for the life
//     of the page so a hot reload prompts again rather than leaking a
//     long-lived credential into web storage.
//   * The **active connection** — at most one `ProtocolClient` is live
//     at a time. `connect(id, handlers)` opens it; `disconnect()` /
//     `switchTo(id, handlers)` close it. The chrome owns the
//     `TerminalPanel` lifecycle and re-mounts it against the new client
//     when the active workplace changes.
//   * **Tailscale discovery** — the in-memory `discovered` map +
//     `refreshTailscalePeers()` introduced in wave 13-F. Discovered
//     peers are *candidates*; promoting one is a separate call into
//     `addWorkplace(...)` (the switcher widget does this on click).
//
// `WorkplaceSwitcher` is a minimal DOM widget that renders both the
// registered workplaces and the discovered Tailscale peers, with a
// click-to-switch / click-to-add affordance plus the pairing-token
// prompt flow.

import {
  ProtocolClient,
  type ProtocolClientHandlers,
  type ProtocolClientOptions,
} from "../workspace/ProtocolClient";
import type { WorkspaceService } from "./WorkspaceService";
import type { WorkspaceSummary, WorkspaceTabSummary } from "../workspace/types";

/**
 * Per-workplace UI preferences mirroring the wire shape exposed by
 * `WorkspaceServerMessage::WorkplacePreferences` on the daemon
 * side. Pure data: every field is optional / defaulted so partial
 * updates from older clients round-trip cleanly without nuking the
 * fields they didn't touch.
 *
 * Stored on the daemon (single registry file under
 * `~/.local/share/neoism/workspaces.json`), NOT in `localStorage` —
 * per the F3 task spec the chrome no longer owns this state so a
 * second laptop or a phone agent picks up the same theme/layout on
 * connect.
 */
export interface WorkplacePreferences {
  theme?: string;
  font_size?: number;
  sidebar_widths?: Record<string, number>;
  /** Opaque JSON-serialised session-layout snapshot. The chrome owns
   *  the shape; the daemon (and this service) treat it as a blob so
   *  newer layouts can round-trip without protocol changes. */
  session_tree?: string;
}

/** DI hook for tests. Production callers get the default factory which
 *  constructs the real `ProtocolClient`; unit tests pass a stub that
 *  records the options + handlers and lets them drive
 *  `onHelloAck`/etc. through a captured handle without touching real
 *  WebSockets. */
export type ProtocolClientFactory = (
  options: ProtocolClientOptions,
  handlers: ProtocolClientHandlers,
) => ProtocolClient;

const defaultClientFactory: ProtocolClientFactory = (options, handlers) =>
  new ProtocolClient(options, handlers);

/** Opaque stable id for a registered workplace. Matches the
 *  `tailscale:<hostname>@<ip>` form for discovered peers, and falls
 *  back to a hash of the URL for manually-typed entries (`manual:<url>`).
 *  Stable across page reloads because the registry persists the id
 *  alongside the URL/label. */
export type DaemonId = string;

/** Persisted shape of one workplace in the registry. Pairing tokens
 *  intentionally live only in `WorkplaceService.tokens` (in-memory)
 *  and are never serialised. */
export interface WorkplaceEntry {
  id: DaemonId;
  label: string;
  url: string;
  /** `tailscale` for entries promoted from discovery, `manual` for
   *  entries typed in the connection screen. */
  transport: "tailscale" | "manual";
}

/** Shape of one peer returned by the daemon's `GET /tailnet-peers`
 *  endpoint. Mirrors `neoism_workspace_daemon::tailnet::TailnetPeer`
 *  on the wire — we keep the surface small so newer daemons can grow
 *  the payload without breaking older browsers. */
export interface TailnetPeer {
  hostname: string;
  ip: string;
  online: boolean;
}

/** One candidate workplace surfaced by Tailscale discovery. The host
 *  hands this to the user's switcher as "click to add to your
 *  workplaces" — `id` is opaque and stable across refreshes, `url`
 *  is the canonical `ws://<ip>:<port>/session` the chrome would
 *  connect to. */
export interface DiscoveredWorkplace {
  id: string;
  label: string;
  url: string;
  transport: "tailscale";
  peer: TailnetPeer;
}

/** Outcome of a `refreshTailscalePeers` call. `added` is the set of
 *  newly-discovered peers (i.e. peers we hadn't seen on a previous
 *  refresh); `total` is the raw count the daemon reported. The caller
 *  can compare `added.length` vs `total` to render a "found N (M
 *  new)" message in the switcher. */
export interface TailscaleRefreshResult {
  added: DiscoveredWorkplace[];
  total: number;
}

export type WorkplaceListener = (
  event:
    | { kind: "discovered"; entries: DiscoveredWorkplace[] }
    | { kind: "cleared" }
    | { kind: "registry"; entries: WorkplaceEntry[]; activeId: DaemonId | null }
    | { kind: "active-changed"; activeId: DaemonId | null }
    | {
        // Wave 4B: the workspace the web client is currently viewing was
        // re-homed to a different host (its `running_on_host_id`
        // changed — e.g. promoted to a cloud node). The client should
        // FOLLOW it. `resolved=true` means we found a connectable URL
        // for the new host (`targetUrl`/`targetId` are set and a re-dial
        // was kicked off via `switchTo`); `resolved=false` means the new
        // host could not be mapped to a URL from the data we have (see
        // `resolveHostUrl` — needs a daemon-side address field) so the
        // chrome should surface a "workspace moved to <host>, can't
        // auto-follow" status instead of silently staying put.
        kind: "rehome";
        workspaceId: string;
        previousHostId: string | null;
        newHostId: string;
        resolved: boolean;
        targetId: DaemonId | null;
        targetUrl: string | null;
      }
    | {
        // Emitted after the active connection's `Hello` handshake.
        // `accepted=false` means the daemon will close the socket
        // shortly; the chrome can surface `reason` (e.g. "invalid
        // pairing token") via a toast / status line. `accepted=true`
        // optionally carries the daemon's `tailscale whois` resolved
        // identity for chrome attribution.
        kind: "hello-ack";
        activeId: DaemonId | null;
        accepted: boolean;
        reason: string | null;
        peerIdentity: string | null;
        connectedHostId: string | null;
      }
    | {
        // F3: emitted whenever the daemon pushes a
        // `WorkplacePreferencesChanged` broadcast (either in response
        // to a `SetWorkplacePreferences` from this client or from a
        // paired surface). Chrome listeners use this to re-apply the
        // theme / font size / sidebar widths without polling.
        kind: "preferences";
        workspaceId: string;
        prefs: WorkplacePreferences;
      },
) => void;

/** localStorage key for the persisted registry. URLs + `lastActiveId`
 *  only (NO pairing tokens — those live in-memory per the wave 12-G
 *  threat model). The persisted shape is `{ entries: WorkplaceEntry[],
 *  lastActiveId: DaemonId | null }` — see `WorkplaceService.persist`.
 *  `lastActiveId` records the workplace the operator was on the last
 *  time `connect()` ran so a page refresh can re-dial the same daemon
 *  without forcing a click through the switcher. Note: a live
 *  connection is **not** restored automatically (the chrome still
 *  drives the actual `connect()` call) — this field is purely a
 *  preferred-default hint. */
const STORAGE_KEY = "neoism.workplaces.v1";

/** Default daemon WebSocket port — matches `crate::main` in
 *  `neoism-workspace-daemon`. The web side builds candidate URLs from
 *  `{ip, port: DAEMON_WS_PORT, path: "/session"}` because the
 *  `/tailnet-peers` endpoint only returns hostnames + IPs. */
const DAEMON_WS_PORT = 7878;

/**
 * Tailscale discovery surface. Pure-data: holds the most recent
 * snapshot of discovered peers + a listener fan-out, and exposes one
 * async `refresh` method that drives the HTTP call. The class
 * intentionally does **not** own any `ProtocolClient` instances —
 * promoting a discovered peer into an actual connection is the
 * caller's job (which today routes through `App.connect(...)` in
 * `frontends/web/src/app/App.ts`).
 */
export class WorkplaceService {
  private discovered = new Map<string, DiscoveredWorkplace>();
  private readonly listeners = new Set<WorkplaceListener>();
  /** Persisted registry. Hydrated from localStorage in `constructor`. */
  private readonly registry = new Map<DaemonId, WorkplaceEntry>();
  /** In-memory pairing tokens keyed by `DaemonId`. **Never** persisted
   *  per the wave 12-G threat model — a hot reload re-prompts. */
  private readonly tokens = new Map<DaemonId, string>();
  /** F3 cache of per-workplace UI preferences received from the active
   *  daemon's `WorkplacePreferences` / `WorkplacePreferencesChanged`
   *  messages. Authoritative source is the daemon — this is just a
   *  client-side mirror so the chrome can re-apply theme/font/sidebar
   *  widths on connect, on workplace switch, or in response to a paired
   *  surface mutating the same workplace.
   *
   *  The map is keyed by *workspace id* (the daemon's stable identifier
   *  for a workspace within the connected daemon) — NOT by `DaemonId`.
   *  Multi-daemon parity is the daemon's job; this service just routes
   *  the active connection's broadcasts. */
  private readonly preferences = new Map<string, WorkplacePreferences>();
  private activeId: DaemonId | null = null;
  private activeClient: ProtocolClient | null = null;
  /** Durable machine id learned from the active daemon's accepted HelloAck. */
  private connectedHostId: string | null = null;
  /** Wave 4B "follow the workspace" state. `followedWorkspaceId` is the
   *  workspace the web client is currently *viewing* (set by the chrome
   *  via `setFollowedWorkspace`); `followedHomeHostId` is the last
   *  `running_on_host_id` we observed for it. When `observeWorkspaceHoming`
   *  sees that home flip to a different host we resolve the new host to a
   *  URL and re-dial so the user keeps seeing the same workspace at its
   *  new home. Both null until the chrome reports the active workspace. */
  private followedWorkspaceId: string | null = null;
  private followedHomeHostId: string | null = null;
  /** Handlers the service ships to `switchTo` when it performs an
   *  automatic re-dial on re-home. `null` disables the auto-reconnect
   *  (the service still emits a `rehome` event so the chrome can drive
   *  the swap itself). The chrome installs this once via
   *  `setRehomeHandlers` so the service can reuse the same
   *  `ProtocolClient` handler bundle it builds for manual switches. */
  private rehomeHandlers: ProtocolClientHandlers | null = null;
  /** Persisted record of the last workplace `connect()` was driven
   *  against. Survives page reloads (unlike `activeId`, which is reset
   *  on every page load because no socket is live yet). The chrome can
   *  read this via `getLastActiveId()` to auto-redial on boot, or use
   *  it as the connection-screen's default URL hint. */
  private lastActiveId: DaemonId | null = null;
  /** localStorage shim. `null` in tests / non-browser; pulled out so a
   *  test can pass a stub without touching globals. */
  private readonly storage: Storage | null;
  /** Factory for building a `ProtocolClient`. Defaults to the real
   *  constructor; tests inject a fake so they can observe `connect()` /
   *  `disconnect()` / `sendWorkspace()` calls without opening a real
   *  WebSocket. The factory receives the same `(options, handlers)`
   *  pair the production constructor takes. */
  private readonly clientFactory: ProtocolClientFactory;

  constructor(
    storage: Storage | null = pickDefaultStorage(),
    clientFactory: ProtocolClientFactory = defaultClientFactory,
  ) {
    this.storage = storage;
    this.clientFactory = clientFactory;
    this.hydrateFromStorage();
  }

  // -----------------------------------------------------------------
  // Discovery (wave 6 / 13-F)
  // -----------------------------------------------------------------

  /** Snapshot of the currently-discovered peers, in stable order by
   *  hostname. Cheap — no IO. */
  listDiscovered(): DiscoveredWorkplace[] {
    return Array.from(this.discovered.values()).sort((a, b) =>
      a.label.localeCompare(b.label),
    );
  }

  /** Subscribe to discovery mutations. Returns an unsubscribe handle. */
  subscribe(listener: WorkplaceListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Drop every discovered entry. Useful when switching focus to a
   *  different daemon whose tailnet may not include the same peers. */
  clearDiscovered(): void {
    if (this.discovered.size === 0) return;
    this.discovered.clear();
    this.emit({ kind: "cleared" });
  }

  // -----------------------------------------------------------------
  // Registry (wave 12-F)
  // -----------------------------------------------------------------

  /** Snapshot of every registered workplace, ordered by label. */
  listWorkplaces(): WorkplaceEntry[] {
    return Array.from(this.registry.values()).sort((a, b) =>
      a.label.localeCompare(b.label),
    );
  }

  /** The currently-focused workplace id (or `null` if no connection
   *  is live). */
  getActiveId(): DaemonId | null {
    return this.activeId;
  }

  /** The active `ProtocolClient` if one is open. Useful for callers
   *  that need to send protocol messages without going through the
   *  per-service wrappers. */
  getActiveClient(): ProtocolClient | null {
    return this.activeClient;
  }

  /** Stable host id for the active connection. Null means the connected
   * daemon predates this handshake field, not that URL comparison is safe. */
  getConnectedHostId(): string | null {
    return this.connectedHostId;
  }

  /** The URL of the currently-focused workplace — handy for the
   *  Tailscale discovery button which needs a daemon to query. */
  getActiveUrl(): string | null {
    if (!this.activeId) return null;
    return this.registry.get(this.activeId)?.url ?? null;
  }

  /**
   * Insert (or update) a workplace in the registry. If the id already
   * exists the URL / label / transport are refreshed; otherwise a new
   * entry is appended. `pairingToken` (if non-empty) is stashed in the
   * in-memory token table so a subsequent `connect()` can ship it as
   * the auth token. Empty strings clear any existing token (trust
   * local). Persists the registry to localStorage as a side effect.
   */
  addWorkplace(
    entry: Omit<WorkplaceEntry, "id"> & { id?: DaemonId },
    pairingToken: string = "",
  ): WorkplaceEntry {
    const id = entry.id ?? deriveManualId(entry.url);
    const stored: WorkplaceEntry = {
      id,
      label: entry.label,
      url: entry.url,
      transport: entry.transport,
    };
    this.registry.set(id, stored);
    if (pairingToken.length > 0) {
      this.tokens.set(id, pairingToken);
    }
    this.persist();
    this.emitRegistry();
    return stored;
  }

  /** Drop a workplace from the registry. If it's currently active the
   *  connection is closed first. */
  removeWorkplace(id: DaemonId): void {
    if (!this.registry.has(id)) return;
    if (this.activeId === id) {
      this.disconnect();
    }
    this.registry.delete(id);
    this.tokens.delete(id);
    this.persist();
    this.emitRegistry();
  }

  /** Stash a pairing token for an existing registry entry without
   *  re-adding it. Empty string clears. Never persisted. */
  setPairingToken(id: DaemonId, token: string): void {
    if (token.length === 0) {
      this.tokens.delete(id);
    } else {
      this.tokens.set(id, token);
    }
  }

  /** True if the entry has an in-memory pairing token (i.e. the user
   *  has authenticated this session). */
  hasPairingToken(id: DaemonId): boolean {
    return this.tokens.has(id);
  }

  /** In-memory pairing token for `id`, or `undefined` if none was
   *  stashed (legacy / trust-local daemon). Used by the
   *  `Hello`-handshake plumbing to fish out the secret it should
   *  ship as the first WebSocket frame. */
  pairingToken(id: DaemonId): string | undefined {
    return this.tokens.get(id);
  }

  // -----------------------------------------------------------------
  // Connection lifecycle (wave 12-F + 13-F integration)
  // -----------------------------------------------------------------

  /**
   * Open a `ProtocolClient` against the registered workplace `id` and
   * mark it active. If a client is already open against a *different*
   * workplace it is disconnected first (the chrome owns terminal-panel
   * lifecycle and re-mounts when `active-changed` fires). If `id` is
   * already the active client the existing handle is returned without
   * dialling a fresh socket.
   *
   * Returns the `ProtocolClient` so the caller can wire up
   * service-level subscribers (PtyService etc.) before calling
   * `client.connect()`. **The caller is responsible** for calling
   * `client.connect()` — this matches the legacy `App.connect` flow
   * where construction and dial are separated so the chrome can
   * register one-shot listeners between the two.
   */
  connect(
    id: DaemonId,
    handlers: ProtocolClientHandlers,
    replacementIntent: "switch" | "rehome" = "switch",
  ): ProtocolClient {
    const entry = this.registry.get(id);
    if (!entry) {
      throw new Error(`unknown workplace id: ${id}`);
    }
    if (this.activeId === id && this.activeClient) {
      return this.activeClient;
    }
    if (this.activeClient) {
      this.disconnectInternal(/*emit*/ false, replacementIntent);
    }
    const token = this.tokens.get(id);
    // Wrap the caller's `onHelloAck` (if any) so we always observe
    // the daemon's handshake outcome — fan it out as a
    // `hello-ack` event on the listener bus and, on rejection,
    // tear the connection down so a downstream consumer doesn't keep
    // pushing into a doomed socket while the daemon is mid-close.
    const userOnHelloAck = handlers.onHelloAck;
    const userOnWorkspaceReply = handlers.onWorkspaceReply;
    let client: ProtocolClient;
    const wrappedHandlers: ProtocolClientHandlers = {
      ...handlers,
      onHelloAck: (accepted, reason, peerIdentity, connectedHostId) => {
        // A superseded socket may finish its handshake after switchTo has
        // installed another client. Never let that stale acknowledgement
        // overwrite the active daemon identity or disconnect the new socket.
        if (this.activeId !== id || this.activeClient !== client) return;
        const stableHostId = accepted ? (connectedHostId ?? null) : null;
        this.connectedHostId = stableHostId;
        this.emit({
          kind: "hello-ack",
          activeId: id,
          accepted,
          reason,
          peerIdentity,
          connectedHostId: stableHostId,
        });
        if (!accepted) {
          // The daemon is closing the socket on its end; mirror that
          // locally so the chrome stops dispatching workspace
          // messages into a doomed connection. We use the
          // disconnect path that emits `active-changed` so the
          // chrome's status line / switcher react immediately.
          if (typeof console !== "undefined") {
            console.warn(
              `[workplace] daemon rejected Hello: ${reason ?? "(no reason)"}`,
            );
          }
          // ProtocolClient owns the terminal auth-rejected state and has
          // already stopped auto-retry. Keep the stable facade registered so
          // the connection gate can offer a workplace switch without a
          // misleading transient `closed` state overwriting the rejection.
        }
        userOnHelloAck?.(accepted, reason, peerIdentity, stableHostId);
      },
      onWorkspaceReply: (payload) => {
        // F3: intercept WorkplacePreferences / WorkplacePreferencesChanged
        // so a single subscription point can re-apply theme / sidebar
        // widths without forcing every downstream chrome consumer to
        // type-narrow the externally-tagged union themselves. We still
        // forward the raw payload to the user's handler so anything
        // that wants the full reply stream keeps working.
        this.ingestWorkspaceReplyForPreferences(payload);
        userOnWorkspaceReply?.(payload);
      },
    };
    client = this.clientFactory(
      {
        url: entry.url,
        authToken: token,
        // Same string for now: the legacy `?token=` channel and the
        // newer `Hello { token }` handshake both authenticate
        // against the same operator-minted secret. Splitting them
        // out is a future hardening step; today the operator pastes
        // one token, and both wire surfaces see it.
        pairingToken: token,
        clientName: "neoism-web",
      },
      wrappedHandlers,
    );
    this.activeClient = client;
    this.activeId = id;
    // Persist `lastActiveId` so a page reload defaults the connection
    // screen back to the workplace the operator was last on. Tokens
    // are deliberately NOT persisted here — `persist()` only writes
    // URLs + `lastActiveId`. See `STORAGE_KEY` doc.
    if (this.lastActiveId !== id) {
      this.lastActiveId = id;
      this.persist();
    }
    this.emit({ kind: "active-changed", activeId: id });
    return client;
  }

  /**
   * Ask the active daemon to re-ship its workspace + editor-surface
   * inventory so the chrome can rehydrate its pane state. Used after
   * `switchTo()` to force-rebroadcast in case the daemon already sent
   * its push frames before the new chrome subscribed. The handshake
   * (`open` event in `App.handleStatus`) also triggers these on socket
   * open, so this is a belt-and-braces re-request — calling it twice
   * is safe because both `SessionList` and `EditorSurfaceList` are
   * idempotent pushes.
   *
   * Returns `false` if no client is currently active (caller should
   * route through `connect()` instead).
   */
  requestPaneSnapshot(): boolean {
    if (!this.activeClient) return false;
    // Push-style messages: no per-request id, the daemon responds with
    // `SessionList` + `EditorSurfaceList` frames the chrome's
    // `onWorkspaceReply` handler already routes through the
    // `WorkspaceService` per-pane bookkeeping.
    this.activeClient.sendWorkspace("ListSessions");
    this.activeClient.sendWorkspace("ListEditorSurfaces");
    return true;
  }

  /**
   * The workplace id `connect()` was last driven against. Persisted to
   * localStorage so a page reload knows which entry to dial. Returns
   * `null` if no `connect()` has ever happened on this storage backend
   * — the chrome should fall back to the registry's first entry (or
   * the connection-screen default URL) in that case. */
  getLastActiveId(): DaemonId | null {
    return this.lastActiveId;
  }

  /**
   * Switch the active workplace. Sugar over `disconnect()` + `connect()`
   * — exposed separately so the chrome can react to a single
   * `active-changed` event when promoting a click on the switcher into
   * a new daemon connection. Returns the freshly-constructed
   * `ProtocolClient`; the caller still owns the `connect()` call.
   */
  switchTo(
    id: DaemonId,
    handlers: ProtocolClientHandlers,
    intent: "switch" | "rehome" = "switch",
  ): ProtocolClient {
    return this.connect(id, handlers, intent);
  }

  /** Close the active connection (if any) without changing the
   *  registry. Emits `active-changed` with `null`. */
  disconnect(): void {
    this.disconnectInternal(/*emit*/ true, "manual");
  }

  private disconnectInternal(
    emit: boolean,
    intent: "manual" | "switch" | "rehome" = "manual",
  ): void {
    if (this.activeClient) {
      try {
        this.activeClient.disconnect(intent);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[workplace] disconnect threw", err);
        }
      }
    }
    this.activeClient = null;
    this.connectedHostId = null;
    const previous = this.activeId;
    this.activeId = null;
    // F3 preferences are daemon-scoped — drop the cache so a future
    // `connect()` against a different daemon doesn't surface stale
    // theme / sidebar widths from the previous registry.
    this.preferences.clear();
    // Wave 4B: drop the follow target so a fresh `connect()` re-seeds it
    // from the new daemon's tree rather than re-home-chasing against a
    // stale `running_on_host_id` baseline. The chrome re-arms it via
    // `setFollowedWorkspace` once the new connection's tree lands. Note:
    // a `switchTo`-driven re-dial calls this through `disconnectInternal`
    // *before* the new socket opens, which is fine — the new tree
    // re-seeds the baseline on connect.
    this.followedWorkspaceId = null;
    this.followedHomeHostId = null;
    if (emit && previous !== null) {
      this.emit({ kind: "active-changed", activeId: null });
    }
  }

  // -----------------------------------------------------------------
  // Follow-the-workspace re-homing (Wave 4B)
  //
  // When the workspace the web client is *viewing* gets re-homed to a
  // different host (its `running_on_host_id` changes — e.g. promoted to
  // a cloud node), the client should FOLLOW it: resolve the new host's
  // daemon URL and reconnect the active `ProtocolClient` there, then the
  // chrome re-attaches the same workspace at its new home.
  //
  // The chrome drives this by (1) telling the service which workspace is
  // active (`setFollowedWorkspace`) and (2) feeding every
  // `WorkspaceSummary` it learns about (from `HostWorkspaceTree`,
  // `HostWorkspaceList`, `WorkspaceControlChanged`) into
  // `observeWorkspaceHoming`. The service watches the followed
  // workspace's `running_on_host_id` and re-dials when it moves.
  // -----------------------------------------------------------------

  /**
   * Register the `ProtocolClient` handler bundle the service should ship
   * when it auto-re-dials on re-home. Without this the service still
   * *detects* the re-home and emits a `rehome` event, but won't open the
   * new socket itself (the chrome can react to the event and drive
   * `switchTo` by hand). The chrome passes the same handlers it builds
   * for manual workplace switches so the re-dialled connection routes
   * frames identically.
   */
  setRehomeHandlers(handlers: ProtocolClientHandlers | null): void {
    this.rehomeHandlers = handlers;
  }

  /**
   * Tell the service which workspace the web client is currently
   * viewing. The re-home watcher only fires for *this* workspace —
   * moves of other (background) workspaces don't yank the active
   * connection around. `homeHostId` seeds the baseline so a later
   * `observeWorkspaceHoming` can tell a genuine move from the first
   * observation. Pass `null` to stop following (e.g. on disconnect).
   */
  setFollowedWorkspace(
    workspaceId: string | null,
    homeHostId: string | null = null,
  ): void {
    this.followedWorkspaceId = workspaceId;
    this.followedHomeHostId = homeHostId;
  }

  /** The workspace id the chrome reported as currently-viewed, or
   *  `null` if none has been set. */
  getFollowedWorkspaceId(): string | null {
    return this.followedWorkspaceId;
  }

  /**
   * Feed workspace summaries (from any host/workspace tree push) into
   * the re-home watcher. If the *followed* workspace's
   * `running_on_host_id` has flipped to a different host since the last
   * observation, resolve the new host to a connectable URL and — if a
   * URL is found and it differs from the active connection — re-dial
   * there, then emit a `rehome` event so the chrome can re-attach the
   * workspace at its new home.
   *
   * Safe to call on every workspace push: it's a no-op unless the
   * followed workspace is present AND its home actually changed.
   */
  observeWorkspaceHoming(summaries: readonly WorkspaceSummary[]): void {
    if (!this.followedWorkspaceId) return;
    const followed = summaries.find((w) => w.id === this.followedWorkspaceId);
    if (!followed) return;
    const newHome = followed.running_on_host_id ?? null;
    if (!newHome) return;
    const previousHome = this.followedHomeHostId;
    if (newHome === previousHome) return;
    // Record the new home eagerly so a duplicate push for the same move
    // doesn't re-trigger the swap while the new socket is still dialling.
    this.followedHomeHostId = newHome;
    // First observation (no prior home recorded) is just baseline
    // seeding, not a move — don't yank the connection on connect.
    if (previousHome === null) return;

    const target = this.resolveHostTarget(newHome);
    if (!target) {
      // The new host couldn't be mapped to a URL from the data we have
      // (see `resolveHostUrl`). Surface the move so the chrome can tell
      // the user "this workspace moved to <host> — reconnect manually"
      // rather than silently leaving them on the stale connection.
      this.emit({
        kind: "rehome",
        workspaceId: followed.id,
        previousHostId: previousHome,
        newHostId: newHome,
        resolved: false,
        targetId: null,
        targetUrl: null,
      });
      return;
    }

    // Already connected to the host that now owns the workspace — the
    // move was a no-op for us (e.g. flip-back-to-local where we were the
    // home all along). Nothing to re-dial.
    if (this.activeId === target.id) {
      return;
    }

    // Resolvable: ensure the target host is in the registry, then
    // re-dial. We only auto-reconnect when the chrome installed
    // `rehomeHandlers`; otherwise we just emit and let the chrome drive
    // the swap with its own handler bundle.
    this.ensureRegistered(target);
    if (this.rehomeHandlers) {
      this.switchTo(target.id, this.rehomeHandlers, "rehome");
    }
    this.emit({
      kind: "rehome",
      workspaceId: followed.id,
      previousHostId: previousHome,
      newHostId: newHome,
      resolved: true,
      targetId: target.id,
      targetUrl: target.url,
    });
  }

  /**
   * Best-effort resolution of a daemon `host_id` (the value carried in
   * `WorkspaceSummary.running_on_host_id`) to a connectable `ws://…`
   * daemon URL. Returns `null` when no mapping can be found.
   *
   * Resolution sources, in priority order:
   *   1. A registry entry whose `DaemonId` already encodes the host —
   *      `tailscale:<hostname>@<ip>` entries whose `<hostname>` equals
   *      the host id (operators commonly set `NEOISM_HOST_ID` to the
   *      tailnet hostname), or a `manual:` entry whose URL host matches.
   *   2. A discovered tailnet peer whose `hostname` equals the host id.
   *
   * The robust path (Wave 4E): the daemon now advertises a dialable
   * `daemon_url` on `HostSummary` (populated from `NEOISM_HOST_URL` in
   * `bootstrap_hosts`). `recordHostDaemonUrls` caches `host_id -> url`
   * from the host-workspace tree, and `resolveHostTarget` step (0) uses
   * it directly. The hostname/registry heuristics below remain as a
   * fallback for hosts that don't advertise a `daemon_url` yet
   * (e.g. a local bootstrap host with no `NEOISM_HOST_URL`).
   */
  resolveHostUrl(hostId: string): string | null {
    return this.resolveHostTarget(hostId)?.url ?? null;
  }

  /** host_id -> canonical daemon_url, learned from `HostSummary.daemon_url`
   *  in the host-workspace tree (Wave 4E). The direct mapping that
   *  supersedes the hostname heuristic for hosts that advertise an
   *  address. */
  private readonly hostDaemonUrls = new Map<string, string>();

  /** Record `daemon_url` for hosts that advertise one, so `resolveHostUrl`
   *  can map a `running_on_host_id` straight to a dialable URL instead of
   *  guessing from tailnet hostnames. Called by the chrome whenever a
   *  host-workspace tree arrives. */
  recordHostDaemonUrls(
    hosts: ReadonlyArray<{ id: string; daemon_url?: string | null }>,
  ): void {
    for (const host of hosts) {
      const url = host.daemon_url?.trim();
      if (url) this.hostDaemonUrls.set(host.id, url);
    }
  }

  /** Internal: resolve a host id to a `{ id: DaemonId, url }` target the
   *  re-dial can switch to. See `resolveHostUrl` for the heuristic + the
   *  daemon-side TODO. */
  private resolveHostTarget(
    hostId: string,
  ): { id: DaemonId; url: string } | null {
    // (0) Robust mapping (Wave 4E): the daemon advertised this host's
    //     dialable `daemon_url` on its `HostSummary`. Preferred over the
    //     hostname heuristics below. Reuse an existing registry entry for
    //     that URL when present, else synthesise a manual id exactly the
    //     way `addWorkplace` does.
    const advertised = this.hostDaemonUrls.get(hostId);
    if (advertised) {
      for (const entry of this.registry.values()) {
        if (entry.url === advertised) return { id: entry.id, url: entry.url };
      }
      return { id: deriveManualId(advertised), url: advertised };
    }
    // (1) Registry entry that already encodes this host. Tailscale ids
    //     are `tailscale:<hostname>@<ip>`; match when `<hostname>` is the
    //     host id. Manual ids are `manual:<url>`; match when the URL's
    //     hostname is the host id.
    for (const entry of this.registry.values()) {
      if (hostMatchesEntry(hostId, entry)) {
        return { id: entry.id, url: entry.url };
      }
    }
    // (2) A discovered tailnet peer whose hostname is the host id. We
    //     synthesise the canonical `ws://<ip>:<port>/session` URL +
    //     `tailscale:<hostname>@<ip>` id so `ensureRegistered` can
    //     promote it before the switch.
    for (const peer of this.discovered.values()) {
      if (peer.peer.hostname === hostId) {
        return { id: peer.id, url: peer.url };
      }
    }
    return null;
  }

  /** Insert the re-home target into the registry if it isn't there
   *  already, so `switchTo` can dial it. Reuses any pairing token the
   *  operator already stashed for that id. Does NOT clobber an existing
   *  entry's label/transport (the operator may have customised them). */
  private ensureRegistered(target: { id: DaemonId; url: string }): void {
    if (this.registry.has(target.id)) return;
    const transport: WorkplaceEntry["transport"] = target.id.startsWith(
      "tailscale:",
    )
      ? "tailscale"
      : "manual";
    this.addWorkplace({
      id: target.id,
      url: target.url,
      label: friendlyLabelFromDaemonUrl(target.url),
      transport,
    });
  }

  // -----------------------------------------------------------------
  // Per-workplace preferences (F3)
  //
  // The daemon owns the canonical state (single workspaces.json
  // registry); the service keeps a hot in-memory mirror so the chrome
  // can re-apply theme / sidebar widths without a round-trip on every
  // render. `requestPreferences` and `setPreferences` push the
  // mutation onto the active socket; the daemon then fans a
  // `WorkplacePreferencesChanged` out to every connected client (the
  // submitter included) which lands in `ingestWorkspaceReplyForPreferences`
  // below.
  // -----------------------------------------------------------------

  /** Latest known preferences for `workspaceId`, or `undefined` if the
   *  daemon hasn't pushed any yet (chrome should treat this as
   *  "apply host defaults"). The map is invalidated on
   *  `disconnect()` because preferences are scoped to the active
   *  daemon's registry. */
  getPreferences(workspaceId: string): WorkplacePreferences | undefined {
    return this.preferences.get(workspaceId);
  }

  /** Ask the active daemon for the persisted preferences of
   *  `workspaceId`. The reply lands as a `WorkplacePreferences`
   *  frame which `ingestWorkspaceReplyForPreferences` caches +
   *  re-emits as a `preferences` listener event. Returns `false`
   *  when there's no live socket. */
  requestPreferences(workspaceId: string): boolean {
    if (!this.activeClient) return false;
    this.activeClient.sendWorkspace({
      GetWorkplacePreferences: { workspace_id: workspaceId },
    });
    return true;
  }

  /** Push a preferences mutation up to the daemon. The daemon
   *  persists + fans the resulting `WorkplacePreferencesChanged` out
   *  to every connected client, including this one — so callers do
   *  NOT need to update `this.preferences` synchronously; the cache
   *  will refresh when the broadcast lands. Returns `false` when
   *  there's no live socket. */
  setPreferences(workspaceId: string, prefs: WorkplacePreferences): boolean {
    if (!this.activeClient) return false;
    this.activeClient.sendWorkspace({
      SetWorkplacePreferences: { workspace_id: workspaceId, prefs },
    });
    return true;
  }

  /** Drain a `WorkspaceServerMessage` for the F3 preferences variants
   *  (`WorkplacePreferences` reply + `WorkplacePreferencesChanged`
   *  broadcast). Anything else is a no-op so the user's
   *  `onWorkspaceReply` keeps owning the rest of the union. */
  private ingestWorkspaceReplyForPreferences(payload: unknown): void {
    if (!payload || typeof payload !== "object") return;
    const rec = payload as Record<string, unknown>;
    const inner =
      pickPreferencesFrame(rec.WorkplacePreferencesChanged) ??
      pickPreferencesFrame(rec.WorkplacePreferences);
    if (!inner) return;
    this.preferences.set(inner.workspaceId, inner.prefs);
    this.emit({
      kind: "preferences",
      workspaceId: inner.workspaceId,
      prefs: inner.prefs,
    });
  }

  // -----------------------------------------------------------------
  // Persistence
  // -----------------------------------------------------------------

  private hydrateFromStorage(): void {
    if (!this.storage) return;
    let raw: string | null;
    try {
      raw = this.storage.getItem(STORAGE_KEY);
    } catch {
      return;
    }
    if (!raw) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return;
    }
    if (!parsed || typeof parsed !== "object") return;
    const entriesRaw = (parsed as Record<string, unknown>).entries;
    if (!Array.isArray(entriesRaw)) return;
    for (const rawEntry of entriesRaw) {
      if (!rawEntry || typeof rawEntry !== "object") continue;
      const e = rawEntry as Record<string, unknown>;
      if (typeof e.id !== "string" || e.id.length === 0) continue;
      if (typeof e.url !== "string" || e.url.length === 0) continue;
      const label = typeof e.label === "string" ? e.label : e.url;
      const transport: WorkplaceEntry["transport"] =
        e.transport === "tailscale" ? "tailscale" : "manual";
      this.registry.set(e.id, { id: e.id, url: e.url, label, transport });
    }
    // We deliberately do NOT restore `activeId` — the page just
    // loaded, no socket is open, so claiming an active workplace would
    // lie to subscribers. The chrome is expected to call `connect()`
    // explicitly once the operator picks a workplace. We DO restore
    // `lastActiveId` so the chrome can default the connection screen
    // back to the workplace the operator last connected to.
    const lastActiveRaw = (parsed as Record<string, unknown>).lastActiveId;
    if (typeof lastActiveRaw === "string" && this.registry.has(lastActiveRaw)) {
      this.lastActiveId = lastActiveRaw;
    }
  }

  private persist(): void {
    if (!this.storage) return;
    const payload = {
      entries: Array.from(this.registry.values()),
      // No `activeId` — see `hydrateFromStorage`. Tokens are
      // intentionally absent too (see class doc). `lastActiveId` IS
      // persisted (it's just an id, not a credential) so a page reload
      // can default the connection screen back to the operator's last
      // workplace.
      lastActiveId: this.lastActiveId,
    };
    try {
      this.storage.setItem(STORAGE_KEY, JSON.stringify(payload));
    } catch {
      // Quota / disabled storage: degrade silently. The registry stays
      // live in memory for this session.
    }
  }

  private emitRegistry(): void {
    this.emit({
      kind: "registry",
      entries: this.listWorkplaces(),
      activeId: this.activeId,
    });
  }

  /**
   * Query the daemon's `/tailnet-peers` HTTP endpoint and merge the
   * result into the in-memory discovery list.
   *
   * `currentDaemonUrl` is the websocket URL of the daemon whose tailnet
   * we want to enumerate. The method rewrites `ws://` / `wss://` to
   * `http://` / `https://` and points the path at `/tailnet-peers`
   * (replacing whatever path was there, e.g. `/session`), so the
   * operator can drive discovery against whichever daemon is currently
   * focused without us having to plumb a second URL through the
   * connection screen.
   *
   * Network failures, parser misses, and "tailscale not installed"
   * (the daemon returns an empty list in that case) all surface as
   * `{ added: [], total: 0 }`. The caller can tell "no peers" apart
   * from "discovery failed" by checking the in-memory list before vs
   * after — `added.length === 0 && total === 0` is the "tailscale
   * unavailable" path; `added.length === 0 && total > 0` means
   * everything is already in the registry.
   */
  async refreshTailscalePeers(
    currentDaemonUrl: string,
  ): Promise<TailscaleRefreshResult> {
    const httpUrl = tailnetPeersUrlFromDaemonUrl(currentDaemonUrl);
    if (httpUrl === null) {
      return { added: [], total: 0 };
    }
    let peers: TailnetPeer[] = [];
    try {
      const response = await fetch(httpUrl, { method: "GET" });
      if (!response.ok) {
        return { added: [], total: 0 };
      }
      const body = (await response.json()) as unknown;
      peers = coerceTailnetPeers(body);
    } catch {
      // Network / CORS / DNS failure — degrade to "no peers" so the
      // switcher renders an empty discovery list rather than throwing.
      return { added: [], total: 0 };
    }
    const added: DiscoveredWorkplace[] = [];
    for (const peer of peers) {
      const id = `tailscale:${peer.hostname}@${peer.ip}`;
      if (this.discovered.has(id)) {
        // Refresh the cached entry's "online" flag without changing
        // the identity — the switcher can re-render to dim offline
        // peers without losing click-to-add affordances.
        const existing = this.discovered.get(id)!;
        existing.peer = peer;
        continue;
      }
      const entry: DiscoveredWorkplace = {
        id,
        label: peer.hostname,
        url: peerToDaemonUrl(peer),
        transport: "tailscale",
        peer,
      };
      this.discovered.set(id, entry);
      added.push(entry);
    }
    this.emit({ kind: "discovered", entries: this.listDiscovered() });
    return { added, total: peers.length };
  }

  private emit(event: Parameters<WorkplaceListener>[0]): void {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch (err) {
        if (typeof console !== "undefined") {
          console.warn("[workplace] listener threw", err);
        }
      }
    }
  }
}

/**
 * Top-right chrome widget that surfaces both the registered
 * workplaces and any peers discovered via the focused daemon's
 * `/tailnet-peers` endpoint. Lives in the HTML layer (not the wasm
 * canvas) so it's reachable before any `ProtocolClient` is open and
 * stays mounted across daemon switches.
 *
 * Two click affordances:
 *   * Registered workplace row → `onSwitch(entry)` (the host
 *     disconnects and re-connects against the new id).
 *   * Discovered peer row → prompts for a pairing token (blank ==
 *     trust local) and calls `onPick(entry, token)` so the host can
 *     promote it into the registry and immediately switch to it.
 */
export interface WorkplaceSwitcherOptions {
  mount: HTMLElement;
  service: WorkplaceService;
  workspaceService?: WorkspaceService | null;
  /** URL of the daemon to query for discovery. The host is expected
   *  to keep this in sync with whatever workplace is focused.
   *  Defaults to `service.getActiveUrl()` if not provided. */
  currentDaemonUrl?: () => string | null;
  /** Called when the user accepts a discovered peer. `pairingToken`
   *  is an empty string for the "trust local" path. */
  onPick: (entry: DiscoveredWorkplace, pairingToken: string) => void;
  /** Called when the user clicks an already-registered workplace.
   *  The host disconnects the current client and re-opens against
   *  the picked entry. */
  onSwitch: (entry: WorkplaceEntry) => void;
  onWorkspaceSwitch?: (workspace: WorkspaceSummary) => void;
  onWorkspaceControl?: (workspace: WorkspaceSummary) => void;
  onWorkspaceCreate?: (hostId: string, title: string | null, rootDir: string | null) => void;
  /** Override for the prompt. Defaults to `window.prompt(...)`; tests
   *  inject a deterministic stub. Returning `null` cancels. */
  promptForPairingToken?: (entry: DiscoveredWorkplace) => string | null;
  prompt?: (message: string, defaultValue?: string) => string | null;
}

export class WorkplaceSwitcher {
  private readonly root: HTMLDivElement;
  private readonly discoverButton: HTMLButtonElement;
  private readonly status: HTMLSpanElement;
  private readonly workspaceTree: HTMLDivElement;
  private readonly registeredList: HTMLUListElement;
  private readonly discoveredList: HTMLUListElement;
  private readonly unsubscribe: () => void;
  private readonly unsubscribeWorkspace: () => void;
  private busy = false;

  constructor(private readonly options: WorkplaceSwitcherOptions) {
    this.root = document.createElement("div");
    this.root.className = "workplace-switcher";
    this.root.setAttribute("role", "group");
    this.root.setAttribute("aria-label", "Workplace switcher");

    const workspaceHeader = document.createElement("span");
    workspaceHeader.className = "workplace-switcher-heading";
    workspaceHeader.textContent = "Workspaces";
    this.root.appendChild(workspaceHeader);

    this.workspaceTree = document.createElement("div");
    this.workspaceTree.className = "workplace-switcher-workspaces";
    this.workspaceTree.setAttribute("aria-label", "Host workspaces");
    this.root.appendChild(this.workspaceTree);

    // Registered workplaces — click to switch the active daemon.
    const registeredHeader = document.createElement("span");
    registeredHeader.className = "workplace-switcher-heading";
    registeredHeader.textContent = "Hosts";
    this.root.appendChild(registeredHeader);

    this.registeredList = document.createElement("ul");
    this.registeredList.className = "workplace-switcher-registered";
    this.registeredList.setAttribute("aria-label", "Registered workplaces");
    this.root.appendChild(this.registeredList);

    this.discoverButton = document.createElement("button");
    this.discoverButton.type = "button";
    this.discoverButton.className = "workplace-switcher-discover";
    this.discoverButton.textContent = "Discover via Tailscale";
    this.discoverButton.title =
      "Query the focused daemon's tailnet for paired peers";
    this.discoverButton.addEventListener("click", () => {
      void this.runDiscovery();
    });
    this.root.appendChild(this.discoverButton);

    this.status = document.createElement("span");
    this.status.className = "workplace-switcher-status";
    this.status.setAttribute("aria-live", "polite");
    this.root.appendChild(this.status);

    this.discoveredList = document.createElement("ul");
    this.discoveredList.className = "workplace-switcher-discovered";
    this.discoveredList.setAttribute("aria-label", "Discovered Tailscale peers");
    this.root.appendChild(this.discoveredList);

    options.mount.appendChild(this.root);
    this.render();
    this.unsubscribe = options.service.subscribe(() => this.render());
    this.unsubscribeWorkspace = options.workspaceService?.subscribe(() => {
      this.renderWorkspaceTree();
    }) ?? (() => {});
    options.workspaceService?.requestHostWorkspaceTree();
  }

  dispose(): void {
    this.unsubscribeWorkspace();
    this.unsubscribe();
    this.root.remove();
  }

  private currentDaemonUrl(): string | null {
    return (
      this.options.currentDaemonUrl?.() ?? this.options.service.getActiveUrl()
    );
  }

  /** Drive one discovery round against the focused daemon's URL. */
  private async runDiscovery(): Promise<void> {
    if (this.busy) return;
    const url = this.currentDaemonUrl();
    if (!url) {
      this.status.textContent =
        "(connect to a workplace first to discover its tailnet)";
      return;
    }
    this.busy = true;
    this.discoverButton.disabled = true;
    this.status.textContent = "Discovering…";
    try {
      const result = await this.options.service.refreshTailscalePeers(url);
      if (result.total === 0) {
        this.status.textContent =
          "No tailnet peers (tailscale not installed?)";
      } else {
        const knownCount = result.total - result.added.length;
        this.status.textContent = `Found ${result.total} peer${
          result.total === 1 ? "" : "s"
        } (${result.added.length} new, ${knownCount} known)`;
      }
    } catch (err) {
      this.status.textContent = `Discovery failed: ${
        err instanceof Error ? err.message : String(err)
      }`;
    } finally {
      this.busy = false;
      this.discoverButton.disabled = false;
    }
  }

  private askForPairingToken(entry: DiscoveredWorkplace): string | null {
    if (this.options.promptForPairingToken) {
      return this.options.promptForPairingToken(entry);
    }
    if (typeof window === "undefined") return null;
    return window.prompt(
      `Pairing token for ${entry.label} (leave blank to trust-local)`,
      "",
    );
  }

  private render(): void {
    this.renderWorkspaceTree();
    this.renderRegistered();
    this.renderDiscovered();
  }

  private renderWorkspaceTree(): void {
    this.workspaceTree.replaceChildren();
    const tree = this.options.workspaceService?.getHostWorkspaceTree();
    if (!tree || tree.workspaces.length === 0) {
      const empty = document.createElement("div");
      empty.className = "workplace-switcher-row-empty";
      empty.textContent = "(no daemon workspaces published yet)";
      this.workspaceTree.appendChild(empty);
      return;
    }

    const tabsByWorkspace = new Map<string, WorkspaceTabSummary[]>();
    for (const tab of tree.tabs) {
      const tabs = tabsByWorkspace.get(tab.workspace_id) ?? [];
      tabs.push(tab);
      tabsByWorkspace.set(tab.workspace_id, tabs);
    }

    const renderWorkspaceList = (workspaces: typeof tree.workspaces): HTMLUListElement => {
      const list = document.createElement("ul");
      list.className = "workplace-switcher-workspace-list";
      for (const workspace of workspaces) {
        const li = document.createElement("li");
        li.className = "workplace-switcher-row";

        const button = document.createElement("button");
        button.type = "button";
        button.className = "workplace-switcher-row-pick";
        const root = workspace.root_dir ? ` — ${workspace.root_dir}` : "";
        button.textContent = `▣ ${workspace.title}${root}`;
        button.title = `Switch to workspace ${workspace.title}`;
        button.addEventListener("click", () => {
          this.options.onWorkspaceSwitch?.(workspace);
        });
        li.appendChild(button);

        if (this.options.onWorkspaceControl) {
          const control = document.createElement("button");
          control.type = "button";
          control.className = "workplace-switcher-row-action";
          control.textContent = "control";
          control.title = `Control workspace ${workspace.title}`;
          control.addEventListener("click", (event) => {
            event.stopPropagation();
            this.options.onWorkspaceControl?.(workspace);
          });
          li.appendChild(control);
        }

        const tabs = tabsByWorkspace.get(workspace.id) ?? [];
        if (tabs.length > 0) {
          const tabList = document.createElement("ul");
          tabList.className = "workplace-switcher-tab-list";
          for (const tab of tabs) {
            const tabItem = document.createElement("li");
            tabItem.className = "workplace-switcher-tab-row";
            tabItem.textContent = `${tab.active ? "◆" : "◇"} ${tab.title}`;
            tabList.appendChild(tabItem);
          }
          li.appendChild(tabList);
        }

        list.appendChild(li);
      }
      return list;
    };

    const renderedWorkspaceIds = new Set<string>();
    for (const host of tree.hosts) {
      const workspaces = tree.workspaces.filter(
        (workspace) => workspace.host_id === host.id,
      );
      if (workspaces.length === 0) continue;
      for (const workspace of workspaces) renderedWorkspaceIds.add(workspace.id);

      const group = document.createElement("section");
      group.className = "workplace-switcher-host-group";

      const hostLabel = document.createElement("div");
      hostLabel.className = "workplace-switcher-host-label";
      const status = hostStatus(host);
      const dot = document.createElement("span");
      dot.className = `workplace-switcher-host-dot workplace-switcher-host-dot-${status}`;
      dot.textContent = "●";
      dot.title = hostStatusTitle(status);
      hostLabel.appendChild(dot);
      hostLabel.append(` ${host.label}`);
      group.appendChild(hostLabel);

      if (this.options.onWorkspaceCreate) {
        const create = document.createElement("button");
        create.type = "button";
        create.className = "workplace-switcher-row-action workplace-switcher-create-workspace";
        create.textContent = "new workspace";
        create.title = `Create workspace on ${host.label}`;
        create.addEventListener("click", () => {
          const prompt = this.options.prompt ?? ((message, defaultValue) => window.prompt(message, defaultValue));
          const title = prompt("Workspace title", "Workspace");
          if (title === null) return;
          const rootDir = prompt("Workspace root directory", "");
          if (rootDir === null) return;
          this.options.onWorkspaceCreate?.(
            host.id,
            title.trim() || null,
            rootDir.trim() || null,
          );
        });
        group.appendChild(create);
      }

      group.appendChild(renderWorkspaceList(workspaces));
      this.workspaceTree.appendChild(group);
    }

    const ungrouped = tree.workspaces.filter(
      (workspace) => !renderedWorkspaceIds.has(workspace.id),
    );
    if (ungrouped.length > 0) {
      const group = document.createElement("section");
      group.className = "workplace-switcher-host-group";

      const hostLabel = document.createElement("div");
      hostLabel.className = "workplace-switcher-host-label";
      hostLabel.textContent = "○ Unassigned host";
      group.appendChild(hostLabel);
      group.appendChild(renderWorkspaceList(ungrouped));
      this.workspaceTree.appendChild(group);
    }
  }

  private renderRegistered(): void {
    this.registeredList.replaceChildren();
    const activeId = this.options.service.getActiveId();
    const entries = this.options.service.listWorkplaces();
    if (entries.length === 0) {
      const li = document.createElement("li");
      li.className = "workplace-switcher-row workplace-switcher-row-empty";
      li.textContent = "(no workplaces yet — discover or connect to add one)";
      this.registeredList.appendChild(li);
      return;
    }
    for (const entry of entries) {
      const li = document.createElement("li");
      li.className = "workplace-switcher-row";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "workplace-switcher-row-pick";
      if (entry.id === activeId) {
        button.classList.add("workplace-switcher-row-active");
        button.setAttribute("aria-current", "true");
      }
      const marker = entry.id === activeId ? "★" : "•";
      button.textContent = `${marker} ${entry.label}`;
      button.title = `Switch to ${entry.label} (${entry.url})`;
      button.addEventListener("click", () => {
        if (entry.id === activeId) return;
        this.options.onSwitch(entry);
      });
      li.appendChild(button);
      this.registeredList.appendChild(li);
    }
  }

  private renderDiscovered(): void {
    this.discoveredList.replaceChildren();
    for (const entry of this.options.service.listDiscovered()) {
      const li = document.createElement("li");
      li.className = "workplace-switcher-row";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "workplace-switcher-row-pick";
      // Dim offline peers (still clickable — the daemon may have come
      // back since the last `/tailnet-peers` snapshot, and the user
      // can promote it anyway so a later `connect()` re-dials).
      if (!entry.peer.online) {
        button.classList.add("workplace-switcher-row-offline");
      }
      const dot = entry.peer.online ? "●" : "○";
      button.textContent = `${dot} ${entry.label} (${entry.peer.ip})`;
      button.title = entry.peer.online
        ? `Connect to ${entry.label} at ${entry.url}`
        : `${entry.label} appears offline — connect anyway at ${entry.url}`;
      button.addEventListener("click", () => {
        const token = this.askForPairingToken(entry);
        if (token === null) {
          // Operator cancelled — leave the entry in the discovered
          // list so they can retry without re-running discovery.
          return;
        }
        this.options.onPick(entry, token);
      });
      li.appendChild(button);
      this.discoveredList.appendChild(li);
    }
  }
}

/**
 * Translate a daemon WebSocket URL (e.g. `ws://laptop-a:7878/session`)
 * into the matching `/tailnet-peers` HTTP URL the discovery endpoint
 * lives at. Returns `null` for inputs we can't reliably rewrite — the
 * caller treats `null` as "discovery unavailable" and shows an empty
 * peer list.
 */
export function tailnetPeersUrlFromDaemonUrl(
  daemonUrl: string,
): string | null {
  let parsed: URL;
  try {
    parsed = new URL(daemonUrl);
  } catch {
    return null;
  }
  if (parsed.protocol === "ws:") {
    parsed.protocol = "http:";
  } else if (parsed.protocol === "wss:") {
    parsed.protocol = "https:";
  } else if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return null;
  }
  parsed.pathname = "/tailnet-peers";
  parsed.search = "";
  parsed.hash = "";
  return parsed.toString();
}

/**
 * Build the canonical `ws://<ip>:<port>/session` URL for a discovered
 * peer. We use the IP (not the hostname) because the browser may not
 * resolve `*.ts.net` short names without MagicDNS configured locally,
 * whereas the tailnet IP is always reachable on a routable tailnet.
 */
export function peerToDaemonUrl(peer: TailnetPeer): string {
  // Wrap IPv6 in brackets so the URL parser doesn't mistake the
  // colons for a port separator.
  const host = peer.ip.includes(":") ? `[${peer.ip}]` : peer.ip;
  return `ws://${host}:${DAEMON_WS_PORT}/session`;
}

/** Derive a stable id for a manually-typed workplace. We don't have a
 *  guaranteed-unique identifier (tailscale entries get
 *  `tailscale:<host>@<ip>`, but a plain ws URL has nothing equivalent)
 *  so we use the URL itself, prefixed to keep the namespace separate
 *  from discovered ids. */
function deriveManualId(url: string): DaemonId {
  return `manual:${url}`;
}

/**
 * Wave 4B: does the daemon `host_id` plausibly identify this registry
 * entry? Used by the re-home resolver as a best-effort host→URL bridge
 * (see `WorkplaceService.resolveHostUrl` for the threat model + the
 * daemon-side TODO). Matches when:
 *   * the entry is `tailscale:<hostname>@<ip>` and `<hostname>` equals
 *     the host id (common when `NEOISM_HOST_ID` is the tailnet
 *     hostname), or
 *   * the entry's URL host (the `<host>` in `ws://<host>:<port>/…`)
 *     equals the host id.
 * Deliberately conservative — a miss returns `false` so the resolver
 * reports "unresolvable" rather than dialling the wrong daemon.
 */
function hostMatchesEntry(hostId: string, entry: WorkplaceEntry): boolean {
  if (entry.id.startsWith("tailscale:")) {
    const hostname = entry.id.slice("tailscale:".length).split("@")[0];
    if (hostname === hostId) return true;
  }
  try {
    const parsed = new URL(entry.url);
    // `hostname` drops the port and the IPv6 brackets, so it compares
    // cleanly against a bare host id.
    if (parsed.hostname === hostId) return true;
  } catch {
    // Unparseable URL — fall through to "no match".
  }
  return false;
}

type HostHealth = "up" | "stale" | "down" | "unknown";

function hostStatus(host: { online?: boolean; last_seen?: number }): HostHealth {
  if (!host.online) return "down";
  const lastSeen = host.last_seen ?? 0;
  if (lastSeen <= 0) return "unknown";
  const ageSeconds = Math.floor(Date.now() / 1000) - lastSeen;
  return ageSeconds > 120 ? "stale" : "up";
}

function hostStatusTitle(status: HostHealth): string {
  switch (status) {
    case "up":
      return "online";
    case "stale":
      return "stale";
    case "down":
      return "offline";
    case "unknown":
      return "unknown";
  }
}

/** Short label for a daemon URL used when auto-registering a re-home
 *  target. Mirrors the chrome's `friendlyLabelFromUrl` (host:port,
 *  protocol/path stripped); kept local so the service has no chrome
 *  dependency. Falls back to the raw URL when parsing fails. */
function friendlyLabelFromDaemonUrl(url: string): string {
  try {
    const parsed = new URL(url);
    return parsed.host || url;
  } catch {
    return url;
  }
}

/** Pick the default `Storage` to persist the registry against. Returns
 *  `null` in non-browser environments (Node tests, SSR) so the service
 *  degrades to "in-memory only" without throwing. */
function pickDefaultStorage(): Storage | null {
  try {
    if (typeof window !== "undefined" && window.localStorage) {
      return window.localStorage;
    }
  } catch {
    // localStorage access can throw in sandboxed iframes / strict
    // browser modes — treat it the same as "no storage".
  }
  return null;
}

/** Coerce a `WorkplacePreferences` / `WorkplacePreferencesChanged`
 *  frame's payload into the typed `{ workspaceId, prefs }` shape. The
 *  daemon's externally-tagged JSON shape is
 *  `{ "WorkplacePreferences": { "workspace_id": "...", "prefs": {...} } }`
 *  — this helper unwraps the inner object and applies the same
 *  field-by-field validation pattern the rest of this file uses for
 *  wire payloads. Returns `null` for anything that doesn't match (the
 *  caller treats `null` as "not a prefs frame, leave alone"). */
function pickPreferencesFrame(
  raw: unknown,
): { workspaceId: string; prefs: WorkplacePreferences } | null {
  if (!raw || typeof raw !== "object") return null;
  const rec = raw as Record<string, unknown>;
  const workspaceId = rec.workspace_id;
  const prefsRaw = rec.prefs;
  if (typeof workspaceId !== "string" || workspaceId.length === 0) return null;
  if (!prefsRaw || typeof prefsRaw !== "object") return null;
  const p = prefsRaw as Record<string, unknown>;
  const prefs: WorkplacePreferences = {};
  if (typeof p.theme === "string") prefs.theme = p.theme;
  if (typeof p.font_size === "number" && Number.isFinite(p.font_size)) {
    prefs.font_size = p.font_size;
  }
  if (p.sidebar_widths && typeof p.sidebar_widths === "object") {
    const widths: Record<string, number> = {};
    for (const [k, v] of Object.entries(
      p.sidebar_widths as Record<string, unknown>,
    )) {
      if (typeof v === "number" && Number.isFinite(v)) widths[k] = v;
    }
    prefs.sidebar_widths = widths;
  }
  if (typeof p.session_tree === "string") prefs.session_tree = p.session_tree;
  return { workspaceId, prefs };
}

/** Narrow the raw `fetch().json()` payload back to `TailnetPeer[]`. We
 *  validate field-by-field instead of trusting the type assertion so a
 *  daemon with a slightly-newer wire shape can't smuggle in fields we
 *  later index as strings. */
function coerceTailnetPeers(body: unknown): TailnetPeer[] {
  if (!body || typeof body !== "object") return [];
  const peersRaw = (body as Record<string, unknown>).peers;
  if (!Array.isArray(peersRaw)) return [];
  const out: TailnetPeer[] = [];
  for (const raw of peersRaw) {
    if (!raw || typeof raw !== "object") continue;
    const rec = raw as Record<string, unknown>;
    if (typeof rec.hostname !== "string" || rec.hostname.length === 0) continue;
    if (typeof rec.ip !== "string" || rec.ip.length === 0) continue;
    out.push({
      hostname: rec.hostname,
      ip: rec.ip,
      online: typeof rec.online === "boolean" ? rec.online : false,
    });
  }
  return out;
}
