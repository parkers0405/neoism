// Config-plane glue for the web Settings + Extensions pages.
//
// Thin typed helpers over `ProtocolClient.requestConfig` (the daemon
// `Config` websocket envelope). Settings writes land in the daemon
// host's unified config.json through the exact same
// `neoism_backend::config::write_setting` / `write_keybind` the
// desktop GUI settings panel calls, so the host's fs-watcher
// hot-reloads any desktop app pointed at the same file. Extensions is
// a read-only inventory of what the daemon host has (bundled MCP
// registry + installed index + live language-server adapter state).

import type {
  ConfigDescriptor,
  ConfigDocument,
  ConfigServerMessage,
  ExtensionSummary,
} from "../workspace/types";

/** Structural slice of `ProtocolClient` this service needs — avoids a
 *  hard import cycle with the workspace client. */
export interface ConfigClientLike {
  requestConfig(message: unknown): Promise<ConfigServerMessage>;
}

/** Ensure and fetch the connected daemon host's raw JSONC document. */
export async function ensureConfigDocument(
  client: ConfigClientLike,
): Promise<ConfigDocument | null> {
  try {
    const reply = await client.requestConfig("EnsureConfigDocument");
    if (reply && typeof reply === "object" && "ConfigDocument" in reply) {
      return reply.ConfigDocument.document;
    }
  } catch {
    // The caller presents a host-scoped open error.
  }
  return null;
}

/** Fetch without requiring create/write permission. */
export async function fetchConfigDocument(
  client: ConfigClientLike,
): Promise<ConfigDocument | null> {
  try {
    const reply = await client.requestConfig("GetConfigDocument");
    if (reply && typeof reply === "object" && "ConfigDocument" in reply) {
      return reply.ConfigDocument.document;
    }
  } catch {
    // Missing files fall through to the write-gated ensure operation.
  }
  return null;
}

/** Save raw JSONC with optimistic revision protection. */
export async function saveConfigDocument(
  client: ConfigClientLike,
  content: string,
  expectedRevision: string,
): Promise<ConfigDocument | null> {
  try {
    const reply = await client.requestConfig({
      SaveConfigDocument: { content, expected_revision: expectedRevision },
    });
    if (reply && typeof reply === "object" && "ConfigDocumentSaved" in reply) {
      return reply.ConfigDocumentSaved.document;
    }
  } catch {
    // Conflict and validation details are surfaced by the caller.
  }
  return null;
}

export async function fetchConfigSchema(
  client: ConfigClientLike,
): Promise<ConfigDescriptor[]> {
  try {
    const reply = await client.requestConfig("GetConfigSchema");
    if (reply && typeof reply === "object" && "ConfigSchema" in reply) {
      return reply.ConfigSchema.descriptors;
    }
  } catch {
    // Completion remains unavailable until the next open retries.
  }
  return [];
}

/** One settings action drained out of the wasm settings overlay
 *  (`drainSettingsActions` JSON rows). */
export type SettingsActionRow =
  | { kind: "set"; key: string; value: unknown }
  | { kind: "set_keybind"; action: string; key: string; with: string }
  | { kind: "open_config_file" }
  | { kind: "run_action"; action: string };

/** Parse the JSON array `drainSettingsActions` returns. Unknown rows
 *  are dropped so a newer wasm bundle can't brick the drain loop. */
export function parseSettingsActions(raw: string | null): SettingsActionRow[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((row): row is SettingsActionRow => {
      if (!row || typeof row !== "object") return false;
      const kind = (row as { kind?: unknown }).kind;
      return (
        kind === "set" ||
        kind === "set_keybind" ||
        kind === "open_config_file" ||
        kind === "run_action"
      );
    });
  } catch {
    return [];
  }
}

/** Fetch the daemon host's full config.json as one JSON document
 *  (already comment/trailing-comma stripped), or null on error. */
export async function fetchConfig(
  client: ConfigClientLike,
): Promise<unknown | null> {
  try {
    const reply = await client.requestConfig("GetConfig");
    if (reply && typeof reply === "object" && "Config" in reply) {
      return (reply as { Config: { value: unknown } }).Config.value;
    }
  } catch {
    // Socket hiccup — the settings overlay stays usable with its
    // last-seeded values; a reopen refetches.
  }
  return null;
}

/** Persist one golden dotted-path setting on the daemon host. */
export async function persistSetting(
  client: ConfigClientLike,
  key: string,
  value: unknown,
): Promise<boolean> {
  try {
    const reply = await client.requestConfig({ SetSetting: { key, value } });
    return Boolean(reply && typeof reply === "object" && "SettingWritten" in reply);
  } catch {
    return false;
  }
}

/** Persist (or clear, with an empty `key`) a keybind override. */
export async function persistKeybind(
  client: ConfigClientLike,
  action: string,
  key: string,
  withMods: string,
): Promise<boolean> {
  try {
    const reply = await client.requestConfig({
      SetKeybind: { action, key, with: withMods },
    });
    return Boolean(reply && typeof reply === "object" && "KeybindWritten" in reply);
  } catch {
    return false;
  }
}

/** Read-only extensions inventory of the daemon host. Empty on error
 *  (the page then shows its empty state rather than stale rows). */
export async function fetchExtensions(
  client: ConfigClientLike,
): Promise<ExtensionSummary[]> {
  try {
    const reply = await client.requestConfig("ListExtensions");
    if (reply && typeof reply === "object" && "Extensions" in reply) {
      const entries = (reply as { Extensions: { entries: unknown } }).Extensions
        .entries;
      return Array.isArray(entries) ? (entries as ExtensionSummary[]) : [];
    }
  } catch {
    // Fall through to empty.
  }
  return [];
}

/** localStorage key for the web NeoWorld pet — the browser-profile
 *  analogue of desktop's per-device sqlite `NeoWorldStore` row, same
 *  `StoredPet` field shape. */
export const NEOWORLD_PET_STORAGE_KEY = "neoism.neoworld.pet";

export function loadStoredNeoworldPet(): string | null {
  try {
    return window.localStorage.getItem(NEOWORLD_PET_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function saveStoredNeoworldPet(json: string): void {
  try {
    window.localStorage.setItem(NEOWORLD_PET_STORAGE_KEY, json);
  } catch {
    // Storage full / privacy mode — the pet simply restarts fresh
    // next session, matching a wiped desktop store.
  }
}
