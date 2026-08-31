// Aggregate bundle of platform services, passed to the wasm chrome.
//
// Mirrors `neoism-ui::services::Services` but with Promise-based
// methods. The chrome host adapts these to the synchronous Rust
// trait shape by maintaining a pending-request table keyed on
// `RequestId` and re-running panels when replies arrive.
//
// Web-parity consolidation (Capacitor prep): EVERY web service is
// registered here with a uniform shape, in two tiers:
//
//   1. CLIENT-SCOPED — constructed from a `ProtocolClient` alone by
//      `defaultServiceRegistry`. Rebuilt wholesale when the host
//      switches daemons (the client is replaced, so the registry is).
//   2. BRIDGE-SCOPED — additionally need the loaded wasm adapter
//      (`ChromeBridge`), which exists only after `createTerminal`
//      resolves. They start `null` and are attached in place via
//      `attachBridgeServices` once the bridge is up.
//
// Hosts (App / TerminalPanel) should look services up here instead of
// constructing them ad hoc, so an iOS shell gets exactly one wiring
// point instead of N scattered `new XService(...)` calls.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import {
  DaemonFilesService,
  type FilesService,
} from "./FilesService";
import {
  DaemonGitService,
  GitPanelService,
  type GitService,
} from "./GitService";
import {
  BrowserClipboardService,
  type ClipboardService,
} from "./ClipboardService";
import {
  DaemonCommandService,
  type CommandService,
} from "./CommandService";
import { PtyService } from "./PtyService";
import { WorkspaceService } from "./WorkspaceService";
import { DiagnosticsService } from "./DiagnosticsService";
import {
  BrowserNotificationService,
  type NotificationService,
} from "./NotificationService";
import { SearchService, type SearchBridge } from "./SearchService";
import { AgentService, type AgentBridge } from "./agent";
import {
  wasmInputPolicy,
  type WasmInputPolicyModule,
} from "../terminal/createTerminal";

export interface ServiceRegistry {
  // ── Tier 1: client-scoped ────────────────────────────────────────
  files: FilesService;
  git: GitService;
  clipboard: ClipboardService;
  commands: CommandService;
  pty: PtyService;
  workspace: WorkspaceService;
  diagnostics: DiagnosticsService;
  notifications: NotificationService;
  // ── Tier 2: bridge-scoped (null until `attachBridgeServices`) ────
  search: SearchService | null;
  agent: AgentService | null;
  gitPanel: GitPanelService | null;
  /** Shared Rust input-policy exports (IME / touch / mobile keyboard /
   *  presence store) from the loaded wasm bundle, or `null` while the
   *  bundle is still loading. Live lookup — never cache the result
   *  across turns of the event loop. */
  inputPolicy(): WasmInputPolicyModule | null;
}

/** The bridge surface `attachBridgeServices` needs: the union of the
 *  per-service bridge slices. `TerminalPanel`'s wasm adapter satisfies
 *  it structurally. */
export type ServiceRegistryBridge = SearchBridge & AgentBridge;

/**
 * Build the default registry for the browser frontend. Files and git
 * route through the daemon WebSocket; the clipboard talks to
 * `navigator.clipboard`; commands dispatch through a local registry
 * with the protocol client wired in for handlers that need it. The
 * bridge-scoped tier stays `null` until the wasm adapter exists —
 * call `attachBridgeServices` from the `onBridgeReady` path.
 */
export function defaultServiceRegistry(
  client: ProtocolClient,
): ServiceRegistry {
  return {
    files: new DaemonFilesService(client),
    git: new DaemonGitService(client),
    clipboard: new BrowserClipboardService(client),
    commands: new DaemonCommandService(client),
    pty: new PtyService(client),
    workspace: new WorkspaceService(client),
    diagnostics: new DiagnosticsService(client),
    notifications: new BrowserNotificationService(),
    search: null,
    agent: null,
    gitPanel: null,
    inputPolicy: wasmInputPolicy,
  };
}

/**
 * Construct + install the bridge-scoped services once the wasm
 * adapter is up. Idempotent per bridge: calling again after a bridge
 * swap replaces the tier with services bound to the new bridge (the
 * underlying `install()` hooks are themselves idempotent).
 */
export function attachBridgeServices(
  registry: ServiceRegistry,
  client: ProtocolClient,
  bridge: ServiceRegistryBridge,
): void {
  const search = new SearchService(client, bridge);
  search.install();
  registry.search = search;
  registry.agent?.dispose();
  registry.agent = new AgentService(client, bridge);
  const gitPanel = new GitPanelService(client, bridge);
  gitPanel.install();
  registry.gitPanel = gitPanel;
}
