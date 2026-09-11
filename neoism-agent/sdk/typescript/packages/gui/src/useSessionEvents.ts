import { isSessionPinned } from "./sessionPins";
import { subscribeGuiEvents } from "./sharedEvents";
import { useEffect, type Dispatch, type SetStateAction } from "react";
import type { NeoismClient, Session } from "@neoism/sdk";
import { errorMessage } from "./types";

/** Use the same root/title/directory filter and ordering as the sessions API. */
export function recentSessions(sessions: Session[], directory: string, search: string): Session[] {
    const query = search.toLowerCase();
    return [...new Map(sessions.map(s => [s.id, s])).values()].filter((s) => !s.parentId &&
        (!directory || s.directory === directory) &&
        (!query || s.title.toLowerCase().includes(query)))
        .sort((a, b) => Number(isSessionPinned(b)) - Number(isSessionPinned(a)) || b.time.updated - a.time.updated || b.id.localeCompare(a.id));
}

export function mergeSession(old: Session | undefined, incoming: Session): Session {
    if (old && old.time.updated > incoming.time.updated) return old;
    const merged: Session = { ...old, ...incoming, time: { ...old?.time, ...incoming.time } };
    // Native unpin removes the flattened metadata key rather than sending false.
    if (!isSessionPinned(incoming)) delete merged.pinned;
    return merged;
}

/** Metadata is independent of the filtered, paginated Recents list. */
export function useSessionEvents(
    client: NeoismClient,
    directory: string,
    search: string,
    setSessions: Dispatch<SetStateAction<Session[]>>,
    notify: (error: string) => void,
    onSession: (session: Session | string) => void,
) {
    useEffect(() => {
        const abort = new AbortController();
        void (async () => {
            try {
                for await (const event of subscribeGuiEvents(client, { signal: abort.signal, tail: true })) {
                    if (abort.signal.aborted) break;
                    if (event.type === "session.updated" || event.type === "session.created" || event.type === "session.compacted") {
                        const session = event.data.info;
                        onSession(session);
                        setSessions((old) => recentSessions([
                            ...old.filter((s) => s.id !== session.id),
                            mergeSession(old.find((s) => s.id === session.id), session),
                        ], directory, search));
                    } else if (event.type === "session.deleted") {
                        onSession(event.data.sessionID);
                        setSessions((old) => old.filter((s) => s.id !== event.data.sessionID));
                    }
                }
            } catch (e) {
                if (!abort.signal.aborted) notify(errorMessage(e));
            }
        })();
        return () => abort.abort();
    }, [client, directory, search, setSessions, notify, onSession]);
}
