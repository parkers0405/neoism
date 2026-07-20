// Wave 7F — web side of the multiplayer presence plane.
//
// Identity + buffer-id helpers shared by the inbound store
// (`RemotePresenceStore`), the outbound publisher
// (`PresencePublisher`), and the surfaces that draw remote carets
// (markdown DOM overlay).
//
// The wire format is `neoism-protocol/src/crdt.rs` (mirrored in
// `workspace/types.ts`); the semantics deliberately mirror the shared
// Rust `neoism-frontend/shared/src/editor/crdt/remote_presence.rs` so
// web and desktop peers behave identically on the same daemon.

export interface PresenceIdentity {
  /** Stable per-browser-profile id, e.g. `chrome-3f2a9c1d@web`. */
  peerId: string;
  /** What other peers see riding the caret, e.g. `Chrome · web`. */
  displayName: string;
}

/**
 * Derive the presence/document buffer id for a file path. MUST stay in
 * lockstep with the daemon's `crdt_buffer_id_for_path` and the shared
 * Rust `presence_buffer_id_for_path` (`file://<abs-path>`) so web,
 * desktop, and daemon peers land on the same channel for the same
 * file.
 *
 * Accepts absolute paths, already-prefixed `file://` ids (returned
 * untouched), and workspace-relative paths (resolved against
 * `workspaceRoot` when it is absolute).
 */
export function presenceBufferIdForPath(
  path: string,
  workspaceRoot?: string | null,
): string {
  const normalized = path.replace(/\\/g, "/").trim();
  if (normalized.startsWith("file://")) return normalized;
  if (normalized.startsWith("/")) return `file://${normalized}`;
  const root = (workspaceRoot ?? "").replace(/\\/g, "/").replace(/\/+$/, "");
  if (root.startsWith("/") && normalized.length > 0) {
    return `file://${root}/${normalized.replace(/^\.\//, "")}`;
  }
  return `file://${normalized}`;
}

const PEER_ID_STORAGE_KEY = "neoism.presence.peer-id";

/**
 * Stable local identity for the presence plane. Mirrors the desktop's
 * `local_presence_identity()` (`user@host` / hostname): on the web the
 * closest stable analogue is a per-browser-profile random id persisted
 * in localStorage — `peer_id` survives reloads so the stable-hashed
 * cursor color other peers see does not flicker between visits.
 */
export function localPresenceIdentity(): PresenceIdentity {
  const browser = browserName();
  let stable: string | null = null;
  try {
    stable = globalThis.localStorage?.getItem(PEER_ID_STORAGE_KEY) ?? null;
  } catch {
    stable = null;
  }
  if (!stable || !/^[0-9a-f]{8,}$/.test(stable)) {
    stable = randomHex(8);
    try {
      globalThis.localStorage?.setItem(PEER_ID_STORAGE_KEY, stable);
    } catch {
      // Private mode / storage denied: identity is per-session only.
    }
  }
  return {
    peerId: `${browser.toLowerCase()}-${stable}@web`,
    displayName: `${browser} · web`,
  };
}

function browserName(): string {
  const ua =
    typeof navigator !== "undefined" ? navigator.userAgent ?? "" : "";
  if (/edg(e|a|ios)?\//i.test(ua)) return "Edge";
  if (/firefox|fxios/i.test(ua)) return "Firefox";
  if (/chrom(e|ium)|crios/i.test(ua)) return "Chrome";
  if (/safari/i.test(ua)) return "Safari";
  return "Web";
}

function randomHex(bytes: number): string {
  const buffer = new Uint8Array(bytes);
  const cryptoApi = globalThis.crypto;
  cryptoApi?.getRandomValues?.(buffer);
  if (buffer.every((byte) => byte === 0)) {
    for (let i = 0; i < buffer.length; i += 1) {
      buffer[i] = Math.floor(Math.random() * 256);
    }
  }
  return Array.from(buffer, (byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  );
}
