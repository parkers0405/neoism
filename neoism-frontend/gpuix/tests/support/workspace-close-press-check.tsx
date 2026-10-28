import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {WorkspaceTabs} from "../../src/ui/WorkspaceTabs";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {tabChrome} from "../../src/layout/tab-chrome";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/workspace-close-press"],{}),fontSize:28,scale:2,reducedMotion:true};
const workspaces=[{id:"a",host_id:"host",title:"A",root_dir:"/a"},{id:"b",host_id:"host",title:"B",root_dir:"/b"}],client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
const file={id:"dirty:a",title:"dirty.rs",kind:"file" as const,path:"dirty.rs",workspaceId:"a",rootDir:"/a"};
model.update({workspaces,activeWorkspace:workspaces[1],surfaces:[file],files:{[file.id]:"saved"}});model.setUI({openWorkspaceIds:["a","b"]});const editor=model.getEditor(file);editor.call("paste",{text:"dirty "});
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),spec=tabChrome(config.scale),activations:string[]=[],closes:Array<Promise<boolean>>=[],moves:unknown[]=[];
const boxes=new Map<number,{x:number;y:number;width:number;height:number}>();host.renderer.getElementBounds=id=>boxes.get(id)||null;
const emit=(testId:string,eventType:"mouseDown"|"mouseUp"|"mouseMove"|"click",x:number,y:number)=>{root.dispatch({elementId:host.find(testId)!.id,eventType,button:0,pressedButton:eventType==="mouseMove"?0:undefined,x,y});root.flush();};
try{
 root.render(()=><WorkspaceTabs config={config} workspaces={workspaces} activeId="b" onActivate={workspace=>activations.push(workspace.id)} onNew={()=>{}} onClose={id=>closes.push(model.closeWorkspace(false,id))} onReorder={(...args)=>moves.push(args)} registerHost={()=>{}} registerNewHost={()=>{}} onBounds={()=>{}} onScroll={()=>{}}/>);await host.pump(root);
 const y=90,h=spec.workspace.height,tabWidth=250,x=300,close=host.find("workspace-close:a")!,tab=host.find("workspace:a")!;
 for(const[id,box]of [["workspace-tab-scroll",{x,y,width:600,height:h}],["workspace:a",{x,y,width:tabWidth,height:h}],["workspace:b",{x:x+tabWidth,y,width:tabWidth,height:h}]] as const){boxes.set(host.find(id)!.id,box);root.dispatch({elementId:host.find(id)!.id,eventType:"layout",...box});}
 const center=x+tabWidth-spec.buffer.padding,closeBox={x:center-spec.buffer.closeHitSize/2,y:y+(h-spec.buffer.closeHitSize)/2,width:spec.buffer.closeHitSize,height:spec.buffer.closeHitSize};
 boxes.set(close.id,closeBox);root.dispatch({elementId:close.id,eventType:"layout",...closeBox});root.flush();
 assert.equal(Number(close.style.right)+Number(close.style.width)/2,spec.buffer.padding,"Workspace X center must match source buffer-tab trailing inset");
 assert.equal(Number(tab.style.paddingRight),9*config.scale+spec.buffer.closeHitSize/2+spec.buffer.padding);
 // Close immediately on press; native parent callback and synthetic click
 // follow separately. The real model rejects this inactive dirty workspace.
 emit("workspace-close:a","mouseDown",center,y+h/2);await Promise.all(closes);assert.equal(closes.length,1);assert.equal(await closes[0],false);
 emit("workspace:a","mouseDown",center,y+h/2);emit("workspace-close:a","click",center,y+h/2);emit("workspace:a","click",center,y+h/2);
 assert.equal(closes.length,1);assert.deepEqual(activations,[]);assert.equal(model.state.activeWorkspace?.id,"b");assert.equal(editor.frame.dirty,true);
 // A second rejected press releases outside the X: no child click occurs.
 emit("workspace-close:a","mouseDown",center,y+h/2);emit("workspace:a","mouseDown",center,y+h/2);emit("workspace:a","mouseUp",x+25,y+h/2);
 emit("workspace:a","click",x+25,y+h/2);assert.deepEqual(activations,[]);assert.equal(closes.length,2);
 // Fresh title press must clear the close latch even without a child click.
 emit("workspace:a","mouseDown",x+25,y+h/2);emit("workspace:a","mouseUp",x+25,y+h/2);emit("workspace:a","click",x+25,y+h/2);assert.deepEqual(activations,["a"]);
 emit("workspace:a","mouseDown",x+25,y+h/2);emit("workspace:a","mouseMove",x+360,y+h/2);assert.ok(host.find("workspace-drag-preview"),"A refused close cannot disable the next drag gesture");
 emit("workspace-drag-layer","mouseUp",x+360,y+h/2);emit("workspace:a","click",x+360,y+h/2);assert.deepEqual(activations,["a"]);assert.equal(moves.length,1);assert.equal(closes.length,2);
 // The onLayout fallback is used if native paint bounds are unavailable.
 boxes.delete(close.id);emit("workspace-close:a","mouseDown",center,y+h/2);emit("workspace:a","mouseDown",center,y+h/2);emit("workspace-close:a","click",center,y+h/2);emit("workspace:a","click",center,y+h/2);assert.equal(closes.length,3);assert.deepEqual(activations,["a"]);
 assert.ok((await Promise.all(closes)).every(result=>!result));assert.equal(editor.text(),"dirty saved");
 console.log(JSON.stringify({noWindow:true,actualSolidPressClosesImmediately:true,clickDeduplicated:true,inactiveDirtyCloseNeverActivates:true,outsideReleaseCannotStickLatch:true,freshDragStillWorks:true,sourceTrailingInsetAtZoom:true,onLayoutBoundsFallback:true,dirtyBytesRetained:true}));
}finally{root.unmount();model.dispose();}
