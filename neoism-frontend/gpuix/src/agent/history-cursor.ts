/** Older daemons omit the cursor even for full pages. The server also accepts
 * durable timeline message/part IDs, so probe once beyond a nonempty page.
 * Empty pages terminate; repeated rows cannot restart the pagination loop.
 * Never use synthetic card IDs as unresolved cursors return the newest page.
 */
export function historyPageCursor(
  cursor: unknown,
  rows: readonly {id: string}[],
  previous: readonly {id: string}[],
  older: boolean,
): string | null {
  if (typeof cursor === "string" && cursor.length) return cursor;
  const first = rows[0]?.id;
  // SDK-authored user messages use UUIDs; server-authored IDs use prefixes.
  if (!first || !/^(?:(?:msg|prt)_[a-zA-Z0-9]+|[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12})$/i.test(first)) return null;
  if (older && previous.some(row => row.id === first)) return null;
  return first;
}
