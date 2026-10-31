import type {SavedSession} from "../session-store";
import {openWorkspaceIds} from "./window-tabs";

/** A menu connection opens one workspace in this window. Keep every saved
 * editor/layout/draft available without reopening the host's old window tabs.
 * Full window startup and tunnel reconnect still restore their entire strip. */
export function connectionRecovery(saved:SavedSession,requestedWorkspaceId?:string):SavedSession {
 const id=requestedWorkspaceId||saved.activeWorkspaceId||openWorkspaceIds(saved.ui)?.[0];
 return {...saved,activeWorkspaceId:id,ui:{...saved.ui,openWorkspaceIds:id?[id]:[]}};
}
