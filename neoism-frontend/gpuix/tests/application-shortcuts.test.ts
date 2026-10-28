import {expect,test} from "bun:test";
import {Effect} from "effect";
import type {KeyEvent} from "@gpuix/solid";
import {applicationShortcut,handleApplicationShortcut,registerApplicationShortcuts} from "../src/commands/application-shortcuts";
import {FontZoomController} from "../src/commands/font-zoom";
import {DaemonConnection} from "../src/daemon/client";
import {expectVariant} from "../src/daemon/protocol";
import {WorkspaceModel} from "../src/model";
import {loadConfig} from "../src/config";
import {createAgentDaemon} from "./support/agent-daemon";
const event=(key:string,modifiers:Partial<NonNullable<KeyEvent["modifiers"]>>,keyChar?:string)=>{let prevented=0,stopped=0;return {value:{key,keyChar,modifiers:{shift:false,ctrl:false,alt:false,cmd:false,...modifiers},preventDefault:()=>{prevented++;},stopPropagation:()=>{stopped++;}}as KeyEvent,get prevented(){return prevented;},get stopped(){return stopped;}};};
const until=async(predicate:()=>boolean)=>{const limit=Date.now()+3000;while(!predicate()){if(Date.now()>limit)throw new Error("Shortcut fixture did not converge");await Bun.sleep(5);}};

test("global terminal and zoom chords bypass popup key handling once per event/window",()=>{
 const renderer={},other={},actions:string[]=[],release=registerApplicationShortcuts(renderer,action=>actions.push(action));
 try{
  const key=event("T",{alt:true});expect(handleApplicationShortcut(renderer,key.value)).toBe(true);expect(handleApplicationShortcut(renderer,key.value)).toBe(true);expect(actions).toEqual(["new-terminal"]);expect([key.prevented,key.stopped]).toEqual([1,1]);
  expect(handleApplicationShortcut(other,event("t",{alt:true}).value)).toBe(false);
  for(const value of [event("=",{ctrl:true}),event("=",{ctrl:true,shift:true},"+"),event("+",{cmd:true}),event("-",{ctrl:true}),event("-",{cmd:true})])expect(handleApplicationShortcut(renderer,value.value)).toBe(true);
  expect(actions.slice(1)).toEqual(["zoom-in","zoom-in","zoom-in","zoom-out","zoom-out"]);
  for(const value of [event("t",{}),event("t",{ctrl:true,alt:true}),event("t",{alt:true,shift:true}),event("+",{alt:true,ctrl:true}),event("=",{})])expect(applicationShortcut(value.value)).toBeUndefined();
 }finally{release();}expect(handleApplicationShortcut(renderer,event("t",{alt:true}).value)).toBe(false);
});

test("global AltT requests fresh PTY creation every time while retaining dirty files and Agent drafts",async()=>{
 const fixture=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/shortcut-test"],{}),endpoint:fixture.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const renderer={},pending:Promise<unknown>[]=[],release=registerApplicationShortcuts(renderer,action=>{if(action==="new-terminal")pending.push(model.newTerminal());});
 try{
  model.start();client.start();await until(()=>!!model.state.activeWorkspace);await model.openFile("README.md");const surface=model.state.surfaces.find(surface=>surface.kind==="file")!,editor=model.getEditor(surface);
  editor.call("key",{key:"i",char:"i"});editor.call("key",{key:"x",char:"X"});const dirty=editor.text();model.setComposerDraft("one",{text:"keep unsent draft",attachments:[]});
  for(let i=0;i<2;i++)expect(handleApplicationShortcut(renderer,event("t",{alt:true}).value)).toBe(true);
  await Promise.all(pending);const created=model.state.surfaces.filter(surface=>surface.kind==="terminal");expect(created).toHaveLength(2);expect(created[0].sessionId).not.toBe(created[1].sessionId);
  expect(fixture.ptyRequests.filter(request=>request.tag==="CreatePty")).toHaveLength(2);expect(editor.text()).toBe(dirty);expect(editor.frame.dirty).toBe(true);expect(model.state.composerDrafts.one.text).toBe("keep unsent draft");
 }finally{release();model.dispose();client.stop();fixture.stop();}
});

test("font zoom persists daemon-wide appearance and serializes rapid steps without replacing document state",async()=>{
 const fixture=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/font-zoom-test"],{}),endpoint:fixture.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);let remounts=0;
 const zoom=new FontZoomController(()=>config.fontSize,async size=>{expectVariant(await Effect.runPromise(client.request("Config",{SetSetting:{key:"appearance.fonts.size",value:size}})),"SettingWritten");},async size=>{
  // Applied locally and synchronously (the desktop resizes in the same frame).
  Object.assign(config,{fontSize:size,scale:size/14});remounts++;
 });
 try{
  model.start();client.start();await until(()=>!!model.state.activeWorkspace);model.setComposerDraft("one",{text:"draft survives global font change",attachments:[]});
  await Promise.all([zoom.change(1),zoom.change(1),zoom.change(-1)]);await zoom.settled();expect(config.fontSize).toBe(15);expect(config.scale).toBe(15/14);expect(remounts).toBe(3);expect(model.state.composerDrafts.one.text).toBe("draft survives global font change");
  expect(fixture.configRequests.filter(request=>request.tag==="SetSetting").map(request=>request.body)).toEqual([{key:"appearance.fonts.size",value:15},{key:"appearance.fonts.size",value:16},{key:"appearance.fonts.size",value:15}]);
  config.fontSize=42;await zoom.change(1);expect(remounts).toBe(3);config.fontSize=7;await zoom.change(-1);expect(remounts).toBe(3);
 }finally{model.dispose();client.stop();fixture.stop();}
});

test("CtrlShiftW creates a fresh workspace from the window shortcut dispatcher without creating a terminal",async()=>{
 const fixture=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/workspace-shortcut"],{}),endpoint:fixture.endpoint},client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 const renderer={},tasks:Promise<unknown>[]=[],release=registerApplicationShortcuts(renderer,action=>{if(action==="new-workspace")tasks.push(model.newWorkspace());});
 try{
  model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading);
  for(let i=0;i<2;i++){const key=event("w",{ctrl:true,shift:true});expect(handleApplicationShortcut(renderer,key.value)).toBe(true);expect(handleApplicationShortcut(renderer,key.value)).toBe(true);}
  await Promise.all(tasks);expect(model.windowWorkspaces()).toHaveLength(3);
  expect(fixture.workspaceRequests.filter(request=>request.tag==="CreateHostWorkspace")).toHaveLength(2);
  expect(fixture.ptyRequests).toEqual([]);expect(model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId)?.kind).toBe("welcome");
  expect(applicationShortcut(event("w",{ctrl:true}).value)).toBeUndefined();expect(applicationShortcut(event("w",{ctrl:true,shift:true,alt:true}).value)).toBeUndefined();
 }finally{release();model.dispose();client.stop();fixture.stop();}
});
