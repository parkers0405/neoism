import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import type {SavedSession} from "../src/session-store";
import {connectionRecovery} from "../src/workspaces/connection-recovery";
import {WindowWorkspaceSessions} from "../src/workspaces/window-sessions";
import {createAgentDaemon} from "./support/agent-daemon";

const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+5000;while(!predicate()){if(Date.now()>deadline)throw new Error("Connection workspace did not hydrate");await Bun.sleep(3);}};
test("one menu SSH/server connection adds one workspace even when that host has other saved tabs",async()=>{
 for(const requested of ["neoism-quick-ssh-home-fixture","b",undefined]){
  const daemon=createAgentDaemon();
  daemon.workspaces.push({id:"neoism-quick-ssh-home-fixture",host_id:"host",title:"Remote Home",root_dir:"/home/fixture-remote"});
  const base={...loadConfig(["--config","/nonexistent/single-connection"],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined};
  const localClient=new DaemonConnection(base),localModel=new WorkspaceModel(localClient,base);
  const config={...base,collaborative:true,serverId:"system:fixture",workspaceId:requested,transportIdentity:"ssh:fixture"};
  const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
  const saved:SavedSession={version:1,activeWorkspaceId:"a",surfaces:[{id:"old-home",kind:"welcome",title:"Home",workspaceId:"a"}],editors:{retained:{text:"unsaved remote bytes"}},providerStates:{},drafts:{},composerDrafts:{old:{text:"unsent remote draft"}},ui:{openWorkspaceIds:["a","b"],layouts:{retained:"saved layout"}}};
  const tabs=new WindowWorkspaceSessions({config:base,model:localModel},{activate:()=>{},local:async()=>({config:base,model:localModel}),dispose:async()=>{}});
  try{
   localModel.start();localClient.start();await until(()=>!!localModel.state.activeWorkspace&&!localModel.state.loading);
   const recovered=connectionRecovery(saved,requested);
   expect(recovered.editors).toBe(saved.editors);expect(recovered.composerDrafts).toBe(saved.composerDrafts);expect(recovered.surfaces).toBe(saved.surfaces);
   expect(saved.ui.openWorkspaceIds).toEqual(["a","b"]);
   model.restoreSession(recovered);model.start();client.start();const remote={config,model};tabs.add(remote);tabs.select(remote);
   await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
   expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual([requested||"a"]);
   expect(tabs.workspaces()).toHaveLength(2); // The existing local tab plus exactly one remote tab.
   expect(model.captureSession().editors).toEqual(saved.editors);expect(model.state.composerDrafts.old.text).toBe("unsent remote draft");
   expect(daemon.workspaceRequests.filter(request=>["CreateHostWorkspace","CloseHostWorkspace"].includes(request.tag))).toEqual([]);
  }finally{tabs.dispose();model.dispose();client.stop();localModel.dispose();localClient.stop();daemon.stop();}
 }
});
