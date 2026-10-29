import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {coreCall} from "../../src/native-core";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
const config={...loadConfig(["--config","/nonexistent/tree-cursor-visibility"],{}),reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const frames=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{frames.add(callback);return()=>{frames.delete(callback);};});
const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"},entries=Array.from({length:200},(_,i)=>({name:`file-${String(i).padStart(3,"0")}.ts`,is_dir:false}));
model.update({activeWorkspace:workspace,workspaces:[workspace],directories:{"":{path:"",entries}},expanded:new Set([""]),loading:false,ui:{sidebar:"files",openWorkspaceIds:["a"]}});
const node=()=>host.find("file-tree")!,caret=()=>host.find("chrome-focus-cursor:sidebar");
const native=()=>coreCall<{selected:number;top:number;offset:number}>("tree.frame",{id:visuals.trees.get(JSON.stringify(["a",false,"/project/a"])),height:680,scale:1});
const pulse=async()=>{for(const callback of [...frames]){frames.delete(callback);callback();}await host.pump(root);await Bun.sleep(8);};
const until=async(check:()=>boolean)=>{const end=Date.now()+3000;while(!check()){await pulse();if(Date.now()>end)throw new Error("Tree cursor visibility did not settle");}await host.pump(root);};
const wheel=async(delta:number)=>{root.dispatch({elementId:node().id,eventType:"scroll",deltaY:delta,precise:true});await pulse();};
try{
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await host.pump(root);
 root.dispatch({elementId:node().id,eventType:"mouseDown",button:0});await until(()=>host.focused===node().id&&!!caret());
 assert.equal(native().selected,0);const originalTop=Number(caret()!.style.top);
 await wheel(-1600);await until(()=>native().top>30&&!caret());
 const down=native();assert.equal(down.selected,0,"Wheel cannot change the selected row to manufacture visibility");assert.equal(host.focused,node().id,"Tree keeps actual keyboard focus while its caret is hidden");assert.ok(!host.find("chrome-focus-paint"),"Offscreen row must not leave painted cursor ink pinned to an edge");
 for(let i=0;i<5;i++)await pulse();assert.equal(native().top,down.top,"Hiding the cursor preserves the wheel viewport");
 await wheel(1600);await until(()=>native().top===0&&!!caret());assert.equal(native().selected,0);assert.equal(host.focused,node().id);
 await until(()=>Math.abs(Number(caret()!.style.top)-originalTop)<1);
 // Both boundaries matter: select the last row, wheel toward the top, then return.
 root.dispatch({elementId:node().id,eventType:"keyDown",key:"end",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await until(()=>native().selected===199&&!!caret());
 await wheel(1600);await until(()=>!caret());assert.equal(native().selected,199);assert.equal(host.focused,node().id);
 await wheel(-1600);await until(()=>!!caret());assert.equal(native().selected,199);
 console.log(JSON.stringify({actualApp:true,noWindow:true,hiddenAbove:true,hiddenBelow:true,selectedRowPreserved:true,viewportPreserved:true,focusPreserved:true,restoredOnReturn:true}));
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();restore();}
