import type { CrdtPeerPresence } from "../workspace/types";

/** Shared code/markdown bridge shape; retain originating OS, not host metadata. */
export function remoteCursorProjection(p: CrdtPeerPresence) {
  return {
    name: p.display_name,
    color: [p.color.r, p.color.g, p.color.b],
    rainbow: p.rainbow ?? false,
    client_os: p.client_os ?? null,
    insert: p.insert ?? true,
    line: p.cursor.line,
    col_utf16: p.cursor.column,
  };
}
