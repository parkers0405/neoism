import type { NeoismClient, Session } from "@neoism/sdk";
import { serverScope } from "./types";

/** Native SessionInfo flattens its extra metadata into the session JSON. */
export const isSessionPinned = (session: Session) => session.pinned === true;

/** Discovery index only: the server, never this cache, decides pin state.
 * The API has no pinned-only list. Pins from other clients are discovered as
 * pages/events arrive; cached IDs let known old pins survive recent pagination.
 * No titles, directories, or credentials are persisted here.
 */
export class SessionPinIndex {
    readonly ids = new Set<string>();
    private readonly key?: string;
    constructor(server: string) {
        try {
            const url = new URL(server);
            url.username = ""; url.password = ""; url.search = ""; url.hash = "";
            this.key = "neoism.gui.pin-index:" + serverScope(url.href);
            const ids: unknown = JSON.parse(localStorage.getItem(this.key) || "[]");
            if (Array.isArray(ids)) for (const id of ids) if (typeof id === "string" && id) this.ids.add(id);
        } catch { /* Invalid/unavailable storage never prevents server pinning. */ }
    }
    observe(session: Session | string) {
        const id = typeof session === "string" ? session : session.id;
        if (!id) return;
        const before = this.ids.has(id);
        const pinned = typeof session !== "string" && isSessionPinned(session);
        if (pinned === before) return;
        if (pinned) this.ids.add(id); else this.ids.delete(id);
        try { if (this.key) localStorage.setItem(this.key, JSON.stringify([...this.ids])); } catch { /* Backend pin remains durable. */ }
    }
}

/** Stop scheduling work on scope changes; in-flight responses are also gated. */
export async function hydrateSessionPins(client: NeoismClient, index: SessionPinIndex,
    alive: () => boolean, receive: (session: Session) => void) {
    const ids = [...index.ids];
    let next = 0;
    await Promise.all(Array.from({ length: Math.min(4, ids.length) }, async () => {
        while (alive() && next < ids.length) {
            const id = ids[next++];
            try {
                const session = await client.sessions.get(id);
                if (!alive()) return;
                if (!session?.id || session.id !== id) continue;
                receive(session);
            } catch (error) {
                if (!alive()) return;
                // Authorization/network errors must not erase another user's index.
                if ((error as { status?: number })?.status === 404) index.observe(id);
            }
        }
    }));
}
