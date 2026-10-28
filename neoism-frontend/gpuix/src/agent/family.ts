import {isRecord} from "../daemon/protocol";
export interface AgentFamilyRow{id:string;parent:string;title:string;agent?:string;status:string;depth:number;startedAt?:number}
export interface AgentFamily{root:string;roster:AgentFamilyRow[]}
const records=(value:unknown):Record<string,unknown>=>isRecord(value)?value:{};
export function agentFamilyRoot(ui:Record<string,unknown>,session:string):string{
 const roots=records(ui.agentFamilyRoots),parents=records(ui.agentParentSessions);if(typeof roots[session]==="string"&&roots[session])return roots[session] as string;
 const seen=new Set<string>();let root=session;while(typeof parents[root]==="string"&&parents[root]&&!seen.has(root)){seen.add(root);root=parents[root] as string;}return root;
}
export function agentFamily(ui:Record<string,unknown>,session:string):AgentFamily{
 const root=agentFamilyRoot(ui,session),value=records(ui.agentFamilyRosters)[root];return{root,roster:Array.isArray(value)?value.filter((row):row is AgentFamilyRow=>isRecord(row)&&typeof row.id==="string"&&typeof row.parent==="string"&&typeof row.title==="string"&&typeof row.status==="string"&&typeof row.depth==="number"):[]};
}
/** Persist only family routing/metadata. Canonical Rust still owns lifecycle,
 * revision rejection, visible-roster policy and ordered header navigation. */
export function noteAgentFamily(ui:Record<string,unknown>,message:unknown):Record<string,unknown>|undefined{
 if(!isRecord(message))return;
 const tag=Object.keys(message)[0],body=records(message[tag]);
 if(!["CatalogFamily","RuntimeSnapshot","SubagentUpdate","SubagentMetadata"].includes(tag))return;
 const originalParents=records(ui.agentParentSessions),originalRoots=records(ui.agentFamilyRoots);
 const originalRosters=records(ui.agentFamilyRosters),originalVersions=records(ui.agentFamilyVersions);
 let parents=originalParents,roots=originalRoots,rosters=originalRosters,versions=originalVersions;
 let root="",incoming:Record<string,unknown>[]=[];
 const snapshot=records(body.snapshot),isSnapshot=tag==="CatalogFamily"||tag==="RuntimeSnapshot";
 if(isSnapshot){
  root=String(snapshot.rootSessionId||body.session_id||"");if(!root)return;
  incoming=Array.isArray(snapshot.branches)?snapshot.branches.filter(isRecord):[];
 }else{
  const id=body.session_id;if(typeof id!=="string"||!id)return;
  const parent=typeof body.parent_session_id==="string"?body.parent_session_id:String(parents[id]||"");
  root=typeof body.root_session_id==="string"&&body.root_session_id?body.root_session_id:parent?agentFamilyRoot(ui,parent):String(roots[id]||"");
  if(!root)return;incoming=[{...body,sessionId:id,parentSessionId:parent}];
 }
 if(tag==="SubagentMetadata"&&typeof body.session_id==="string"&&!parents[body.session_id]&&!roots[body.session_id])return;
 if(isSnapshot){
  const key=root+":"+tag,previous=records(versions[key]);
  const revision=typeof snapshot.familyRevision==="number"?snapshot.familyRevision:0;
  const execution=tag==="CatalogFamily"?snapshot.executionId??null:null;
  const retired=Array.isArray(previous.retired)?previous.retired:[];
  if(previous.execution===execution&&typeof previous.revision==="number"&&revision<previous.revision||tag==="CatalogFamily"&&retired.includes(execution))return;
  if(previous.execution!==execution||previous.revision!==revision){
   versions={...versions,[key]:{execution,revision,retired:previous.execution!==undefined&&previous.execution!==execution?[...retired,previous.execution]:retired}};
  }
 }
 const stored=rosters[root],previous=Array.isArray(stored)?stored.filter((row):row is AgentFamilyRow=>isRecord(row)&&typeof row.id==="string"):[];
 const previousById=new Map(previous.map(row=>[row.id,row]));
 const authoritative=tag==="CatalogFamily"||tag==="RuntimeSnapshot"&&snapshot.branchesAuthoritative===true;
 const rows=authoritative?[] as AgentFamilyRow[]:[...previous];
 for(const raw of incoming){
  const id=String(raw.sessionId||raw.session_id||"");if(!id||id===root)continue;
  const parent=String(raw.parentSessionId||raw.parent_session_id||parents[id]||root);
  if(parents[id]!==parent){if(parents===originalParents)parents={...parents};parents[id]=parent;}
  if(roots[id]!==root){if(roots===originalRoots)roots={...roots};roots[id]=root;}
  const old=previousById.get(id),reported=String(raw.status||old?.status||"running").toLowerCase();
  const status=reported==="outstanding"?(raw.permissionPending===true?"blocked":"running"):reported;
  const row:AgentFamilyRow={id,parent,title:typeof raw.title==="string"&&raw.title?raw.title:old?.title||id,status,
   agent:typeof raw.agent==="string"?raw.agent:old?.agent,depth:typeof raw.depth==="number"?raw.depth:old?.depth||1,
   startedAt:typeof raw.startedAt==="number"?raw.startedAt:typeof raw.started_at==="number"?raw.started_at:old?.startedAt};
  if(authoritative)rows.push(row);else if(old)rows[rows.findIndex(entry=>entry.id===id)]=row;else if(tag!=="SubagentMetadata")rows.push(row);
 }
 for(let index=0;index<rows.length;index++){
  const row=rows[index];let depth=1,parent=row.parent;const seen=new Set<string>([row.id]);
  while(parent&&parent!==root&&!seen.has(parent)){seen.add(parent);depth++;parent=String(parents[parent]||root);}
  if(row.depth!==depth)rows[index]={...row,depth};
 }
 // Compare only this family's rows. A catalog baseline can contain thousands
 // of unrelated roots; cloning/stringifying the complete maps for each one
 // stalls rendering and prevents the socket from draining its bounded queue.
 const fields=(row:AgentFamilyRow)=>[row.id,row.parent,row.title,row.agent,row.status,row.depth,row.startedAt];
 const unchanged=rows.length===previous.length&&rows.every((row,index)=>fields(row).every((value,key)=>value===fields(previous[index])[key]));
 if(!unchanged){rosters={...rosters,[root]:rows};}
 const patch:Record<string,unknown>={};
 if(parents!==originalParents)patch.agentParentSessions=parents;
 if(roots!==originalRoots)patch.agentFamilyRoots=roots;
 if(rosters!==originalRosters)patch.agentFamilyRosters=rosters;
 if(versions!==originalVersions)patch.agentFamilyVersions=versions;
 return Object.keys(patch).length?{...ui,...patch}:undefined;
}
