// JS-side glue for the daemon-hosted Claude API agent proxy.
//
// Wire shape (mirrors `neoism-protocol/src/agent.rs`):
//
//   inbound   ServiceServerMessage::AgentReply { request_id, message }
//   outbound  ServiceClientMessage::Agent      { request_id, message }
//
// The chrome bridge (`ChromeBridge::agent_send_message` /
// `agent_cancel` / `agent_new_thread`) emits the inner
// `AgentClientMessage` JSON; we wrap it in the `Agent` envelope and
// hand it to the WebSocket. Streaming events arrive back through the
// existing `onServiceReply` channel on `ProtocolClient` and we replay
// them into the bridge via `ChromeBridge::agent_event(...)`.

import type { ProtocolClient } from "../workspace/ProtocolClient";
import type {
  AgentClientMessage,
  AgentServerMessage,
} from "../workspace/types";

export type { AgentClientMessage, AgentServerMessage };

export interface AgentBridge {
  agentEvent(eventJson: string): void;
  agentResumeActiveStream?(): boolean;
  /** Snapshot of the composer's sent-prompt history: JSON array of
   *  strings, newest first, capped at 1000 — the same shape (and cap)
   *  as desktop's zsh-style `prompt_history` file. Optional so older
   *  wasm bundles keep working. */
  agentPromptHistoryJson?(): string;
  /** Restore a persisted history snapshot (the
   *  `agentPromptHistoryJson` shape) into the bridge's ledger. */
  agentRestorePromptHistory?(json: string): boolean;
}

/** localStorage key for the persisted agent prompt history. MUST stay
 *  in sync with `PROMPT_HISTORY_KEY` in
 *  `wasm/src/rendered/agent.rs` — the bridge write-throughs to the
 *  same slot on every send, so the service- and bridge-level writers
 *  always converge on identical content. */
export const AGENT_PROMPT_HISTORY_KEY = "neoism.agent.prompt-history.v1";

/** Debounce for save-on-submit so a burst of queued prompts costs one
 *  localStorage write. */
const PROMPT_HISTORY_SAVE_DEBOUNCE_MS = 500;

/** Read the persisted prompt history (newest-first string array).
 *  Best-effort: a denied/absent localStorage yields `[]`. */
export function loadPromptHistory(): string[] {
  try {
    const raw = window.localStorage.getItem(AGENT_PROMPT_HISTORY_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (entry): entry is string =>
        typeof entry === "string" && entry.trim().length > 0,
    );
  } catch {
    return [];
  }
}

/** Persist a newest-first prompt history snapshot. Best-effort — a
 *  full/denied store never breaks sending. */
export function savePromptHistory(newestFirst: string[]): void {
  try {
    window.localStorage.setItem(
      AGENT_PROMPT_HISTORY_KEY,
      JSON.stringify(newestFirst.slice(0, 1000)),
    );
  } catch {
    // Swallowed, mirroring desktop prompt_history's stance on a
    // read-only home directory.
  }
}

/**
 * Subscribes to the WebSocket, routes inbound agent events into the
 * wasm bridge, and exposes a `send` hook the bridge can call when the
 * chrome wants to ship a `SendMessage` envelope. Also owns prompt-
 * history persistence at the service level: restore on construction,
 * debounced save whenever a prompt envelope goes out.
 */
export class AgentService {
  private historySaveTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly client: ProtocolClient,
    private readonly bridge: AgentBridge,
  ) {
    // Load-on-init: seed the bridge's ledger from localStorage so a
    // reloaded tab starts with its zsh-style history intact. The
    // bridge also lazy-loads the same key itself, so this is safe to
    // run (or skip, on an older bundle) in any order.
    const persisted = loadPromptHistory();
    if (persisted.length > 0) {
      this.bridge.agentRestorePromptHistory?.(JSON.stringify(persisted));
    }
    document.addEventListener("visibilitychange", this.handleVisibilityChange);
  }

  private readonly handleVisibilityChange = (): void => {
    if (document.visibilityState === "visible") {
      this.bridge.agentResumeActiveStream?.();
    }
  };

  dispose(): void {
    document.removeEventListener("visibilitychange", this.handleVisibilityChange);
    if (this.historySaveTimer !== null) {
      clearTimeout(this.historySaveTimer);
      this.historySaveTimer = null;
    }
  }

  /** Route a daemon-emitted `AgentServerMessage` into the bridge.
   *  The bridge mirrors `Notice` events into the chrome's global
   *  toast stack internally (`mirror_agent_event_to_bridge`), so we
   *  don't fan it out twice here. */
  ingestServerMessage(message: AgentServerMessage): void {
    this.bridge.agentEvent(JSON.stringify(message));
  }

  /**
   * Ship a pre-allocated `AgentClientMessage` envelope to the daemon.
   * `requestId` MUST be the value the bridge allocated alongside
   * `agent_send_message` so streaming replies tag through the same
   * pending-correlation slot.
   */
  sendEnvelope(requestId: number, envelope: AgentClientMessage): void {
    // `ProtocolClient.sendAgent` wraps the envelope under the
    // top-level `Agent` service tag the daemon dispatches via
    // `ServiceClientMessage::Agent`. Going through the typed sender
    // keeps the unified status / reconnect bookkeeping in
    // `ProtocolClient` and gives us type checking on the inner
    // message variant.
    this.client.sendAgent(requestId, envelope);
    if (isPromptEnvelope(envelope)) {
      this.scheduleHistorySave();
    }
  }

  /**
   * Helper that parses the JSON the bridge emits via its
   * `set_agent_send` callback and ships it to the daemon. The
   * `requestId` and `envelopeJson` come straight off the wasm
   * callback's two arguments.
   */
  forwardBridgeOutbound(requestId: number, envelopeJson: string): void {
    let envelope: AgentClientMessage;
    try {
      envelope = JSON.parse(envelopeJson) as AgentClientMessage;
    } catch (err) {
      if (typeof console !== "undefined") {
        console.warn("[agent] failed to parse outbound envelope", err);
      }
      return;
    }
    this.sendEnvelope(requestId, envelope);
  }

  /** Debounced save-on-submit: pull the bridge's canonical snapshot
   *  and persist it. (The wasm bridge also write-throughs on its own
   *  send paths — same key, same content — so this only matters for
   *  hosts pairing an older bundle with this service.) */
  private scheduleHistorySave(): void {
    const snapshot = this.bridge.agentPromptHistoryJson;
    if (!snapshot) return;
    if (this.historySaveTimer !== null) {
      clearTimeout(this.historySaveTimer);
    }
    this.historySaveTimer = setTimeout(() => {
      this.historySaveTimer = null;
      try {
        const json = snapshot.call(this.bridge);
        const parsed: unknown = JSON.parse(json);
        if (Array.isArray(parsed)) {
          savePromptHistory(parsed as string[]);
        }
      } catch {
        // Snapshot unavailable — the bridge-side write-through still
        // covers persistence.
      }
    }, PROMPT_HISTORY_SAVE_DEBOUNCE_MS);
  }
}

/** True for the envelopes that carry a user prompt (the moments the
 *  history ledger advances). */
function isPromptEnvelope(envelope: AgentClientMessage): boolean {
  return (
    typeof envelope === "object" &&
    envelope !== null &&
    ("SubmitPrompt" in envelope || "SendMessage" in envelope)
  );
}
