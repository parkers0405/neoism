import {expect,test} from "bun:test";
import {reorderWindowWorkspace} from "../src/workspaces/window-tabs";
import {workspaceDropTarget} from "../src/workspaces/drag-target";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel} from "../src/model";
import {createAgentDaemon} from "./support/agent-daemon";
const until=async(predicate:()=>boolean)=>{const deadline=Date.now()+2500;while(!predicate()){if(Date.now()>deadline)throw new Error("Workspace order did not converge");await Bun.sleep(5);}};
test("membership reorder moves before/after stable IDs while preserving hidden membership",()=>{
 const ui={openWorkspaceIds:["a","offline","b","c"],unrelated:true};
 expect(reorderWindowWorkspace(ui,"c","a","before")).toEqual(["c","a","offline","b"]);
 expect(reorderWindowWorkspace(ui,"a","c","after")).toEqual(["offline","b","c","a"]);
 expect(reorderWindowWorkspace(ui,"c","b","after")).toEqual(ui.openWorkspaceIds);
 expect(reorderWindowWorkspace(ui,"a","a")).toEqual(ui.openWorkspaceIds);expect(reorderWindowWorkspace(ui,"missing","a")).toEqual(ui.openWorkspaceIds);
 expect(ui.openWorkspaceIds).toEqual(["a","offline","b","c"]);
});
test("drop midpoint uses painted horizontal coordinates after scrolling and cancels outside the strip",()=>{
 const clip={x:300,y:52,width:250,height:30};
 const tabs=[{id:"a",bounds:{x:235,y:52,width:100,height:30}},{id:"b",bounds:{x:335,y:52,width:130,height:30}},{id:"c",bounds:{x:465,y:52,width:140,height:30}}];
 expect(workspaceDropTarget(tabs,clip,"a",399,65)).toMatchObject({id:"b",position:"before"});expect(workspaceDropTarget(tabs,clip,"a",400,65)).toMatchObject({id:"b",position:"after"});
 expect(workspaceDropTarget(tabs,clip,"a",535,65)).toMatchObject({id:"c",position:"after"});expect(workspaceDropTarget(tabs,clip,"b",320,65)).toMatchObject({id:"a",position:"after"});
 expect(workspaceDropTarget(tabs,clip,"a",320,65)).toBeUndefined();expect(workspaceDropTarget(tabs,clip,"a",399,90)).toBeUndefined();expect(workspaceDropTarget(tabs,clip,"a",560,65)).toBeUndefined();
 const shifted=tabs.map(tab=>({...tab,bounds:{...tab.bounds,x:tab.bounds.x-80}}));
 expect(workspaceDropTarget(shifted,clip,"a",399,65)).toMatchObject({id:"c",position:"before"});
 const fractional=tabs.map(tab=>({...tab,bounds:{x:tab.bounds.x*1.5,y:tab.bounds.y*1.5,width:tab.bounds.width*1.5,height:tab.bounds.height*1.5}}));
 expect(workspaceDropTarget(fractional,{x:450,y:78,width:375,height:45},"a",600,97.5)).toMatchObject({id:"b",position:"after"});
});
test("model reorder persists only this window's order without switching, discarding, or writing daemon state",async()=>{
 const daemon=createAgentDaemon();daemon.threads.splice(0);const config={...loadConfig(["--config","/nonexistent/workspace-reorder"],{}),endpoint:daemon.endpoint,authToken:undefined};
 const client=new DaemonConnection(config),model=new WorkspaceModel(client,config);model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
 try{
  await model.openFile("README.md");const surface=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)!;const editor=model.getEditor(surface);editor.call("paste",{text:"keep dirty "});
  model.setComposerDraft("home:a",{text:"keep draft",attachments:[]});await model.activateWorkspace(daemon.workspaces[1]!);model.setUI({openWorkspaceIds:["a","offline","b"]});
  const catalog=model.state.workspaces,active=model.state.activeWorkspace,surfaces=model.state.surfaces,layout=model.getLayout().snapshot(),requests=daemon.workspaceRequests.length;
  expect(model.reorderWorkspace("b","a","before")).toBe(true);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["b","a"]);expect(model.captureSession().ui.openWorkspaceIds).toEqual(["b","a","offline"]);
  expect(model.state.workspaces).toBe(catalog);expect(model.state.activeWorkspace).toBe(active);expect(model.state.surfaces).toBe(surfaces);expect(model.getLayout().snapshot()).toEqual(layout);expect(daemon.workspaceRequests).toHaveLength(requests);expect(daemon.ptyRequests).toEqual([]);
  expect(editor.frame.dirty).toBe(true);expect(editor.text()).toStartWith("keep dirty ");expect(model.state.composerDrafts["home:a"]?.text).toBe("keep draft");
  expect(model.reorderWorkspace("offline","a")).toBe(false);expect(model.reorderWorkspace("b","a","before")).toBe(false);expect(model.reorderWorkspace("a","a")).toBe(false);
  daemon.pushWorkspace({HostWorkspaceTree:{hosts:[{id:"host",label:"Host"}],workspaces:[...daemon.workspaces].reverse(),tabs:[]}});await Bun.sleep(10);expect(model.windowWorkspaces().map(workspace=>workspace.id)).toEqual(["b","a"]);
 }finally{model.dispose();client.stop();daemon.stop();}
});
