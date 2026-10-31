import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import type {WorkspaceModel} from "../src/model";
import type {Workspace} from "../src/daemon/protocol";
import {WindowWorkspaceSessions} from "../src/workspaces/window-sessions";
import {cloudWorkspace} from "../src/workspaces/workspace-icon";

function session(remote=false){
 const config={...loadConfig([],{}),endpoint:remote?"ws://remote:7878/session":"ws://127.0.0.1:7878/session",collaborative:remote,serverId:remote?"remote":"local"};
 const workspace={id:"same-id",host_id:"same-host",title:remote?"Remote":"Local",root_dir:remote?"/remote":"/local"} as Workspace;
 const listeners=new Set<()=>void>(),calls={new:0,closed:0},draft={text:"unsaved local edit"};
 let dirty=false;
 const state={workspaces:[workspace],activeWorkspace:workspace,ui:{openWorkspaceIds:[workspace.id]},connection:{connectedHostId:"same-host"}};
 const emit=()=>{for(const listener of listeners)listener();};
 const model={state,editors:new Map([["file",draft]]),subscribe:(fn:()=>void)=>{listeners.add(fn);return()=>listeners.delete(fn);},
  windowWorkspaces:()=>state.workspaces.filter(w=>state.ui.openWorkspaceIds.includes(w.id)),
  async activateWorkspace(w:Workspace){state.activeWorkspace=w;emit();},
  async newWorkspace(parent=workspace){calls.new++;const w={...parent,id:`new-${calls.new}`};state.workspaces=[...state.workspaces,w];state.ui={openWorkspaceIds:[...state.ui.openWorkspaceIds,w.id]};state.activeWorkspace=w;emit();return w;},
  async closeWorkspace(discard:boolean,id:string,keepOne=true){if(dirty&&!discard)return false;calls.closed++;state.ui={openWorkspaceIds:state.ui.openWorkspaceIds.filter(w=>w!==id)};if(keepOne&&!state.ui.openWorkspaceIds.length)await model.newWorkspace();emit();return true;},
 } as unknown as WorkspaceModel;
 return {config,model,calls,draft,emit,dirty:()=>{dirty=true;}};
}
function window(local=session()){
 const activated:typeof local[]=[],disposed:typeof local[]=[];
 const tabs=new WindowWorkspaceSessions(local,{activate:s=>{activated.push(s);},local:async()=>local,dispose:async s=>{disposed.push(s);}});
 return {tabs,local,activated,disposed};
}
test("joining another host retains local tabs and unsaved editor ownership, including colliding server IDs",async()=>{
 const {tabs,local,disposed}=window(),remote=session(true),editor=local.model.editors.get("file");
 const localTab=tabs.workspaces()[0];tabs.add(remote);tabs.select(remote);
 expect(tabs.workspaces()).toHaveLength(2);expect(new Set(tabs.workspaces().map(w=>w.id)).size).toBe(2);
 expect(local.model.editors.get("file")).toBe(editor);expect(disposed).toEqual([]);
 const context=tabs.context(localTab.id)!;expect(cloudWorkspace(localTab,context.config,context.connectedHostId)).toBe(false);
 await tabs.activate(localTab);expect(tabs.activeId()).toBe(localTab.id);expect(disposed).toEqual([]);
 tabs.dispose();
});
test("the window plus always creates locally while a remote connection is selected",async()=>{
 const {tabs,local,disposed}=window(),remote=session(true);tabs.add(remote);tabs.select(remote);
 await tabs.newWorkspace();expect(local.calls.new).toBe(1);expect(remote.calls.new).toBe(0);
 expect(tabs.workspaces()).toHaveLength(3);expect(disposed).toEqual([]);
 expect(tabs.context(tabs.activeId()!)!.config).toBe(local.config);tabs.dispose();
});
test("closing the last remote tab protects dirty buffers and never creates another remote workspace",async()=>{
 const {tabs,local,disposed}=window(),remote=session(true);tabs.add(remote);tabs.select(remote);remote.dirty();
 expect(await tabs.close()).toBe(false);expect(disposed).toEqual([]);
 expect(await tabs.close(true)).toBe(true);expect(disposed).toEqual([remote]);expect(remote.calls.new).toBe(0);
 expect(tabs.context(tabs.activeId()!)!.config).toBe(local.config);tabs.dispose();
});
test("reconnecting replaces only its owning connection and keeps tab identity and local buffers",()=>{
 const {tabs,local}=window(),remote=session(true);tabs.add(remote);tabs.select(remote);const id=tabs.activeId();
 const replacement=session(true);tabs.replace(remote,replacement);
 expect(tabs.activeId()).toBe(id);expect(tabs.sessions()).toEqual([local,replacement]);expect(local.model.editors.get("file") as unknown).toBe(local.draft);tabs.dispose();
});
test("chat and cursor updates do not rebuild the window workspace strip",()=>{
 const {tabs,local}=window();let updates=0;tabs.subscribe(()=>updates++);const rows=tabs.workspaces();
 for(let i=0;i<100;i++)local.emit();expect(updates).toBe(0);expect(tabs.workspaces()).toBe(rows);tabs.dispose();
});

test("plus uses the connected local host even when its catalog has another host selected",async()=>{
 const {tabs,local}=window();
 const other={id:"another-host-tab",host_id:"another-host",title:"Other host",root_dir:"/other"} as Workspace;
 Object.assign(local.model.state,{workspaces:[...local.model.state.workspaces,other],activeWorkspace:other});
 const created=await tabs.newWorkspace();expect(created?.host_id).toBe("same-host");tabs.dispose();
});
test("a window opened directly on SSH acquires one local connection for repeated plus presses",async()=>{
 const remote=session(true),local=session();let opened=0;
 remote.config={...remote.config,transportIdentity:"ssh:fixture"};
 const tabs=new WindowWorkspaceSessions(remote,{activate:()=>{},local:async()=>{opened++;await Promise.resolve();return local;},dispose:async()=>{}});
 await Promise.all([tabs.newWorkspace(),tabs.newWorkspace()]);
 expect(opened).toBe(1);expect(local.calls.new).toBe(2);expect(remote.calls.new).toBe(0);
 expect(tabs.sessions()).toEqual([remote,local]);expect(tabs.context(tabs.activeId()!)!.config).toBe(local.config);tabs.dispose();
});
