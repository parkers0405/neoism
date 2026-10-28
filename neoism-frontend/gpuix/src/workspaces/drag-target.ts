import type {ChromeBounds} from "../ui/chrome-target-bounds";
export interface WorkspaceDragTab {id:string;bounds:ChromeBounds}
export interface WorkspaceDropTarget extends WorkspaceDragTab {position:"before"|"after"}
/** All rectangles are current painted window coordinates, including scroll. */
export function workspaceDropTarget(tabs:readonly WorkspaceDragTab[],clip:ChromeBounds,dragId:string,x:number,y:number):WorkspaceDropTarget|undefined{
 if(x<clip.x||x>clip.x+clip.width||y<clip.y||y>clip.y+clip.height)return;
 const visible=tabs.filter(tab=>tab.bounds.x+tab.bounds.width>clip.x&&tab.bounds.x<clip.x+clip.width).sort((a,b)=>a.bounds.x-b.bounds.x);
 const hit=visible.find(tab=>x>=tab.bounds.x&&x<=tab.bounds.x+tab.bounds.width);
 if(hit)return hit.id===dragId?undefined:{...hit,position:x<hit.bounds.x+hit.bounds.width/2?"before":"after"};
 const edge=x<(visible[0]?.bounds.x??x)?visible[0]:visible.at(-1);
 if(edge&&edge.id!==dragId)return {...edge,position:x<edge.bounds.x?"before":"after"};
}
