import {canonicalPath} from "./canonical-path";
import assert from "node:assert/strict";
import {mkdtempSync,mkdirSync,rmSync,writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {join,sep} from "node:path";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {coreCall} from "../../src/native-core";
import {statusInfo} from "../../src/chrome/status-info";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const temporary=canonicalPath(mkdtempSync(join(tmpdir(),"neoism-status-directory-"))),start=join(temporary,"start"),selected=join(start,"selected"),other=join(temporary,"other"),configPath=join(temporary,"config.json");
mkdirSync(selected,{recursive:true});mkdirSync(other);writeFileSync(configPath,"{}");
const daemon=createAgentDaemon({deferCreations:false});daemon.threads.splice(0);daemon.workspaces[0]!.root_dir=start;daemon.workspaces[1]!.root_dir=other;
const config={...loadConfig(["--config",configPath],{}),endpoint:daemon.endpoint,authToken:undefined,pairingToken:undefined,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const until=async(predicate:()=>boolean)=>{const end=Date.now()+3000;while(!predicate()){await host.pump(root);if(Date.now()>end)throw new Error("Status directory actual App did not settle");await Bun.sleep(2);}await host.pump(root);};
const key=async(key:string,char=key.length===1?key:"")=>{root.dispatch({elementId:host.find("directory-browser")!.id,eventType:"keyDown",key,keyChar:char,modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await host.pump(root);};
async function clickDirectory(){
 const id=coreCall<{id:number}>("chrome.new",{kind:"status",family:config.fontFamily,weight:config.fontWeight,fonts:[config.iconFontFamily]}).id;
 try{
  const active=model.state.surfaces.find(surface=>surface.id===model.state.activeSurfaceId);
  const frame=coreCall<{height:number}>("chrome.frame",{id,width:1440,scale:config.scale,theme:config.themeName,info:statusInfo(active,model.state.activeWorkspace?.root_dir,{mode:"Normal",line:0,col:0,dirty:false})});
  let x:number|undefined;
  for(let candidate=0;candidate<700;candidate+=2){const hit=coreCall<{action?:{kind:string}}>("chrome.hit",{id,x:candidate,y:frame.height/2});if(hit.action?.kind==="open_directory_palette"){x=candidate;break;}}
  assert.notEqual(x,undefined,"Canonical native cwd pill must expose the original hit action");
  root.dispatch({elementId:host.find("original-statusline")!.id,eventType:"mouseDown",button:0,x:300+x!,y:90+frame.height/2});await host.pump(root);
 }finally{coreCall("chrome.dispose",{id});}
}
try{
 model.start();client.start();await until(()=>model.state.activeWorkspace?.id==="a"&&!model.state.loading);
 root.render(()=><App model={model} config={config}/>);await host.pump(root);
 const shell=host.nativeRoot,beforeReads=daemon.fileRequests.length;
 const splash=host.find("splash-action:change-directory");assert.ok(splash,"Welcome exposes its original Change Directory action");
 root.dispatch({elementId:splash.id,eventType:"click",button:0});await until(()=>!!host.find("directory-browser"));
 assert.equal(host.find("chrome-action-overlay"),undefined,"Welcome opens the same folder browser as the status cwd pill");await key("escape");
 root.dispatch({elementId:host.find("welcome-page")!.id,eventType:"keyDown",key:"d",keyChar:"d",modifiers:{shift:false,ctrl:false,alt:true,cmd:false}});await until(()=>!!host.find("directory-browser"));
 assert.equal(host.find("chrome-action-overlay"),undefined,"The displayed Alt+D shortcut opens the same folder browser");await key("escape");
 assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot").length,0);
 await clickDirectory();await until(()=>!!host.find("directory-browser"));assert.equal(host.find("chrome-action-overlay"),undefined,"Status path must open the actual FileBrowserModal rather than command-directory search");
 for(const char of `${sep}selected`)await key(char);await key("enter");await key("enter");
 await until(()=>model.state.activeWorkspace?.root_dir===selected&&!model.state.loading&&!host.find("directory-browser"));
 assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot").length,1);assert.equal(daemon.workspaceRequests.find(request=>request.tag==="SetWorkspaceRoot")!.body.root_dir,selected);
 assert.ok(daemon.fileRequests.length>beforeReads,"Explorer must refresh after the scoped directory commit");assert.equal(daemon.ptyRequests.length,0);assert.equal(host.nativeRoot,shell);
 await clickDirectory();await until(()=>!!host.find("directory-browser"));await key("escape");assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot").length,1);
 await clickDirectory();await until(()=>!!host.find("directory-browser"));await model.activateWorkspace(model.state.workspaces.find(workspace=>workspace.id==="b")!);await host.pump(root);
 await key("enter");await key("enter");await until(()=>!host.find("directory-browser"));assert.equal(model.state.activeWorkspace?.root_dir,other);assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot").length,1,"Stale A selection cannot change B");
 const remote={...model.state.activeWorkspace!,host_kind:"tailscale"};model.update({workspaces:model.state.workspaces.map(workspace=>workspace.id===remote.id?remote:workspace)});await model.activateWorkspace(remote);await host.pump(root);
 await clickDirectory();assert.equal(host.find("directory-browser"),undefined);assert.ok(model.state.error?.includes("remote folder browser"));assert.equal(daemon.workspaceRequests.filter(request=>request.tag==="SetWorkspaceRoot").length,1);
 console.log(JSON.stringify({noWindow:true,actualStatusLineNativeHit:true,originalFolderModal:true,selectedDirectoryCommitsAndRefreshes:true,noImplicitPty:true,cancelHasNoWrites:true,workspaceSwitchDropsStaleSelection:true,remoteNeverUsesClientFilesystem:true,persistentRoot:true}));
}finally{root.unmount();model.dispose();client.stop();daemon.stop();rmSync(temporary,{recursive:true,force:true});}
