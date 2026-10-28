import {expect,test} from "bun:test";
import {agentFamily,agentFamilyRoot,noteAgentFamily} from "../src/agent/family";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {createAgentDaemon} from "./support/agent-daemon";
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw Error("Family state did not converge");await Bun.sleep(5);}};
test("unrelated empty catalog families retain routing identities and revision authority without filling the UI with empty rosters",()=>{
 const parents=Object.freeze({child:"root"}),roots=Object.freeze({child:"root"});
 const row=Object.freeze({id:"child",parent:"root",title:"Child",status:"running",depth:1});
 const roster=Object.freeze([row]),rosters=Object.freeze({root:roster});
 let ui:Record<string,unknown>={agentParentSessions:parents,agentFamilyRoots:roots,agentFamilyRosters:rosters};
 for(let index=0;index<825;index++)ui=noteAgentFamily(ui,{CatalogFamily:{session_id:`other-${index}`,snapshot:{executionId:"run",familyRevision:8,branches:[]}}})!;
 expect(ui.agentParentSessions).toBe(parents);expect(ui.agentFamilyRoots).toBe(roots);expect(ui.agentFamilyRosters).toBe(rosters);
 expect(noteAgentFamily(ui,{CatalogFamily:{session_id:"other-824",snapshot:{executionId:"run",familyRevision:7,branches:[{sessionId:"stale",parentSessionId:"other-824"}]}}})).toBeUndefined();
 const changed=noteAgentFamily(ui,{SubagentMetadata:{session_id:"child",parent_session_id:"root",title:"Renamed"}})!;
 expect(agentFamily(changed,"child").roster[0].title).toBe("Renamed");expect(row.title).toBe("Child");expect(changed.agentParentSessions).toBe(parents);
});
test("original family identity survives nested metadata and authoritative roster reorders without stale completed children",()=>{
 let ui=noteAgentFamily({}, {CatalogFamily:{session_id:"root",snapshot:{directory:"/project/a",branches:[{sessionId:"first",parentSessionId:"root",title:"First",status:"running",depth:1},{sessionId:"nested",parentSessionId:"first",title:"Nested",status:"running",depth:2}]}}})!;
 expect(agentFamilyRoot(ui,"nested")).toBe("root");expect(agentFamily(ui,"nested").roster.map(row=>[row.id,row.depth])).toEqual([["first",1],["nested",2]]);
 ui=noteAgentFamily(ui,{SubagentMetadata:{session_id:"nested",parent_session_id:"first",title:"Better name"}})!;expect(agentFamily(ui,"first").roster[1].title).toBe("Better name");
 ui=noteAgentFamily(ui,{CatalogFamily:{session_id:"root",snapshot:{branches:[{sessionId:"nested",parentSessionId:"first",status:"completed",depth:2},{sessionId:"first",parentSessionId:"root",status:"running",depth:1}]}}})!;expect(agentFamily(ui,"nested").roster.map(row=>row.id)).toEqual(["nested","first"]);expect(agentFamily(ui,"nested").roster[0].title).toBe("Better name");
 ui=noteAgentFamily(ui,{CatalogFamily:{session_id:"root",snapshot:{branches:[]}}})!;expect(agentFamily(ui,"nested").roster).toEqual([]);expect(agentFamilyRoot(ui,"nested")).toBe("root");
});
test("sidebar child and sibling activation reuse one parent family tab; pointer toggles active child back, Enter keeps it and parent draft stays parked",async()=>{
 const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/gpuix-family-test"],{}),endpoint:daemon.endpoint,authToken:undefined},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 try{model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a");const rootThread=daemon.threads[0];const rootId=model.openThread(rootThread)!;model.setComposerDraft(rootThread.session_id,{text:"parked parent draft",attachments:[]});
  daemon.push({CatalogFamily:{session_id:rootThread.session_id,snapshot:{directory:"/project/a",executionId:"exec",familyRevision:1,branches:[{sessionId:"child",parentSessionId:rootThread.session_id,title:"Child",status:"running",depth:1},{sessionId:"sibling",parentSessionId:rootThread.session_id,title:"Sibling",status:"running",depth:1}]}}});await until(()=>model.agentFamily("child").root===rootThread.session_id);
  const child={session_id:"child",title:"Child",directory:"/project/a"},sibling={session_id:"sibling",title:"Sibling",directory:"/project/a"};
  expect(model.openThread(child,false,rootThread.session_id,true)).toBe(rootId);expect(model.state.activeSurfaceId).toBe(rootId);expect(model.state.surfaces.find(s=>s.id===rootId)).toMatchObject({sessionId:"child",agentRootSessionId:rootThread.session_id,title:rootThread.title});
  expect(model.openThread(sibling,false,rootThread.session_id,true)).toBe(rootId);expect(model.state.surfaces.filter(s=>s.kind==="agent")).toHaveLength(1);expect(model.state.surfaces.find(s=>s.id===rootId)?.sessionId).toBe("sibling");
  model.openThread(sibling,false,rootThread.session_id,false);expect(model.state.surfaces.find(s=>s.id===rootId)?.sessionId).toBe("sibling");
  model.openThread(sibling,false,rootThread.session_id,true);expect(model.state.surfaces.find(s=>s.id===rootId)?.sessionId).toBe(rootThread.session_id);expect(model.state.composerDrafts[rootThread.session_id]?.text).toBe("parked parent draft");
  model.openThread(daemon.threads[1]);expect(model.state.surfaces.filter(s=>s.kind==="agent")).toHaveLength(2);expect(daemon.requests.some(r=>["CreateThread","SubmitPrompt"].includes(r.tag))).toBe(false);expect(daemon.ptyRequests).toHaveLength(0);
 }finally{model.dispose();client.stop();daemon.stop();}
});

test("family routing rejects retired/stale catalog snapshots and maps canonical outstanding lifecycle without promoting metadata-only children",()=>{
 const snapshot={directory:"/project/a",executionId:"run",familyRevision:8,branches:[{sessionId:"child",parentSessionId:"root",status:"outstanding",permissionPending:true,title:"Child"}]};
 const ui=noteAgentFamily({}, {CatalogFamily:{session_id:"root",snapshot}})!;expect(agentFamily(ui,"child").roster[0].status).toBe("blocked");
 expect(noteAgentFamily(ui,{CatalogFamily:{session_id:"root",snapshot:{...snapshot,familyRevision:7,branches:[]}}})).toBeUndefined();
 const newer=noteAgentFamily(ui,{CatalogFamily:{session_id:"root",snapshot:{...snapshot,executionId:"new-run",familyRevision:1}}})!;
 expect(noteAgentFamily(newer,{CatalogFamily:{session_id:"root",snapshot}})).toBeUndefined();expect(noteAgentFamily(ui,{SubagentMetadata:{session_id:"unknown",parent_session_id:"root",title:"Unknown"}})).toBeUndefined();
});
