import type { Session, SessionRuntimeSnapshot } from "@neoism/sdk";

/** Ownership comes only from current session metadata, never its agent/name. */
export function subagentView(id: string | undefined, active?: Pick<Session, "id" | "parentId">, runtime?: Pick<SessionRuntimeSnapshot, "rootSessionId">) {
    const known = !!id && active?.id === id;
    const parentId = known ? active.parentId : undefined;
    const rootId = runtime?.rootSessionId;
    const returnId = parentId ? (rootId && rootId !== id ? rootId : parentId) : undefined;
    return {
        canCompose: !id || (known && !parentId),
        metadataLoading: !!id && !known,
        isChild: !!parentId,
        returnId,
        // Without a distinct runtime root we only know this is the parent.
        backLabel: rootId && rootId !== id ? "Back to main chat" : "Back to parent chat",
    };
}
