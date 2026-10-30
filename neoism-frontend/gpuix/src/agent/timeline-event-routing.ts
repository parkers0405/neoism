import {isRecord} from "../daemon/protocol";
import {agentFamilyRoot} from "./family";

const transcriptEvents=["ContentDelta","MessageUpdated","HistoryChunk","MessageEnd"] as const;

/** Avoid serializing another open conversation's transcript into every native
 * pane. Family/lifecycle events still reach all panes for header activity.
 * Unknown roots retain the existing fallback: a child's first event can arrive
 * before the catalog identifies its parent. */
export function timelineReceivesEvent(root:string,ui:Record<string,unknown>,knownRoots:Pick<ReadonlySet<string>,"has">,message:unknown):boolean{
 if(!isRecord(message))return true;
 for(const tag of transcriptEvents){
  const body=message[tag];if(!isRecord(body)||typeof body.session_id!=="string")continue;
  const family=agentFamilyRoot(ui,body.session_id);
  return family===root||!knownRoots.has(family);
 }
 return true;
}
