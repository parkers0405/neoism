import type {Workspace} from "../daemon/protocol";
export interface WindowWorkspaceState {workspaces:readonly Workspace[];ui:Readonly<Record<string,unknown>>}
/** Missing membership is a legacy profile; it must not imply the host catalog. */
export function openWorkspaceIds(ui:Readonly<Record<string,unknown>>):string[]|undefined{
 if(!Array.isArray(ui.openWorkspaceIds))return;
 return [...new Set(ui.openWorkspaceIds.filter((id):id is string=>typeof id==="string"&&id.trim().length>0))];
}
export function includeWindowWorkspace(ui:Readonly<Record<string,unknown>>,id:string):string[]{
 const ids=openWorkspaceIds(ui)||[];return !id.trim()||ids.includes(id)?ids:[...ids,id];
}
export function reorderWindowWorkspace(ui:Readonly<Record<string,unknown>>,dragId:string,targetId:string,position:"before"|"after"="before"):string[]{
 const ids=openWorkspaceIds(ui)||[];
 if(dragId===targetId||!ids.includes(dragId)||!ids.includes(targetId))return ids;
 const reordered=ids.filter(id=>id!==dragId),index=reordered.indexOf(targetId)+(position==="after"?1:0);
 reordered.splice(index,0,dragId);return reordered;
}
/** Preserve this window's order and identities, including same-root workspaces.
 * Offline/deleted IDs stay in recovery but cannot manufacture catalog records. */
export function getWindowWorkspaces(state:WindowWorkspaceState):Workspace[]{
 const catalog=new Map(state.workspaces.map(workspace=>[workspace.id,workspace]));
 return (openWorkspaceIds(state.ui)||[]).flatMap(id=>{const workspace=catalog.get(id);return workspace?[workspace]:[];});
}
