import type { ProtocolClient } from "../workspace/ProtocolClient";

export type FileBrowserRequest = { kind?: string; path?: string };

/** No token-bearing endpoint URLs or cross-host legacy recents. */
export function fileBrowserRecentsKey(client: Pick<ProtocolClient, "getConnectedHostId" | "agentCatalogIdentity">): string {
  return `neoism.file-browser.recents.v2:${JSON.stringify(client.getConnectedHostId() ?? client.agentCatalogIdentity())}`;
}

export function parseFileBrowserRecents(raw: string | null): string[] {
  try {
    const value: unknown = JSON.parse(raw ?? "[]");
    return Array.isArray(value)
      ? [...new Set(value.filter((item): item is string => typeof item === "string" && item.length > 0))].slice(0, 8)
      : [];
  } catch { return []; }
}

/** Host paths are opaque. Only extracting a selected file's parent is lexical;
 * never resolve host paths with browser/Unix path utilities. */
export function browserFileParent(path: string): string {
  const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (separator < 0) return "";
  if (separator === 0 || (separator === 2 && path[1] === ":")) return path.slice(0, separator + 1);
  return path.slice(0, separator);
}
