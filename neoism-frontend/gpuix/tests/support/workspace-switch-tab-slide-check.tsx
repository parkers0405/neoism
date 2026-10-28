import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal,Show} from "solid-js";
import {PaneLayoutView} from "../../src/layout/PaneLayoutView";
import {WorkspaceLayout} from "../../src/layout/WorkspaceLayout";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const config={...loadConfig(["--config","/nonexistent/workspace-switch-slide"],{}),reducedMotion:false};
const a=new WorkspaceLayout(),b=new WorkspaceLayout(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
for(const layout of [a,b])for(const title of ["short.rs","a-much-longer-buffer-name.ts","notes.md"])layout.open({id:`${layout.id}:${title}`,title,kind:"file"});
const [active,setActive]=createSignal(a),offsets:number[]=[],applyBatch=host.renderer.applyBatch.bind(host.renderer);
host.renderer.applyBatch=(json:string)=>{
 const result=applyBatch(json);
 for(const[op,id,style]of JSON.parse(json))if(op==="setStyle"&&String(host.nodes.get(id)?.props.testId||"").startsWith("pane-tab:"))offsets.push(Number(style.left)||0);
 return result;
};
async function hydrate(layout:WorkspaceLayout){
 await host.pump(root);
 root.dispatch({elementId:host.find("pane-layout")!.id,eventType:"layout",x:300,y:90,width:960,height:680});
 // Real title/parent measurements can arrive separately, after fallback widths.
 const tabs=layout.frame.panes[0]!.tabs;
 for(let i=0;i<tabs.length;i++){
  const tab=host.find(`pane-tab:${tabs[i]!.leaf}`)!;
  root.dispatch({elementId:tab.id,eventType:"layout",x:300+i*140,y:90,width:[90,230,120][i]!,height:28});root.flush();
  assert.equal(Number(host.nodes.get(tab.id)!.style.left)||0,0);
 }
 assert.ok(tabs.every(tab=>(Number(host.find(`pane-tab:${tab.leaf}`)!.style.left)||0)===0));
}
try{
 root.render(()=><Show when={active()} keyed>{layout=><PaneLayoutView controller={layout} config={config} renderSurface={()=><div/>} onClose={()=>{}}/>}</Show>);
 await hydrate(a);const firstHost=host.find("pane-layout")!.id,selected=a.frame.focusedLeaf;
 setActive(b);await hydrate(b);assert.notEqual(host.find("pane-layout")!.id,firstHost);
 setActive(a);await hydrate(a);assert.notEqual(host.find("pane-layout")!.id,firstHost);assert.equal(a.frame.focusedLeaf,selected);
 assert.ok(offsets.length>0);assert.ok(offsets.every(offset=>offset===0),"Workspace restoration must never publish a tab entrance offset");
 console.log(JSON.stringify({noWindow:true,actualSolidKeyedWorkspaceSwitch:true,initialAndRestoredMeasurementsSnap:true,noTabEntranceOffsets:true,selectionPreserved:true}));
}finally{root.unmount();a.dispose();b.dispose();}
