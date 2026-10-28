import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {PaneLayoutView} from "../../src/layout/PaneLayoutView";
import {WorkspaceLayout} from "../../src/layout/WorkspaceLayout";
import {WorkspaceTabs} from "../../src/ui/WorkspaceTabs";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer,type RecordedAppNode} from "./full-app-recording-renderer";

const config={...loadConfig(["--config","/nonexistent/close-hover"],{}),reducedMotion:true};
const layout=new WorkspaceLayout(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
layout.open({id:"a",title:"A",kind:"welcome"});layout.open({id:"b",title:"B",kind:"welcome"});
const inactive=layout.frame.leaves.find(tab=>tab.surfaceId==="a")!.leaf;
const workspaces=[{id:"a",host_id:"host",title:"A",root_dir:"/a"},{id:"b",host_id:"host",title:"B",root_dir:"/b"}];
const textBelow=(node:RecordedAppNode):RecordedAppNode|undefined=>node.type==="text"?node:node.children.map(id=>textBelow(host.nodes.get(id)!)).find(Boolean);
function verify(tabId:string,closeId:string){
 const tab=host.find(tabId)!,close=host.find(closeId)!;
 const glyph=()=>textBelow(close)!;
 const event=(target:RecordedAppNode,eventType:"mouseEnter"|"mouseLeave")=>{root.dispatch({elementId:target.id,eventType});root.flush();};
 assert.equal(glyph().style.opacity,0);const normalSize=Number(glyph().style.fontSize);
 event(tab,"mouseEnter");assert.equal(glyph().style.color,config.theme.muted);assert.equal(glyph().style.opacity,1);
 event(close,"mouseEnter");assert.equal(glyph().style.color,config.theme.red);assert.equal(glyph().style.fontSize,normalSize*1.16);
 // Native parent leave can arrive after the pointer has entered its child.
 event(tab,"mouseLeave");assert.equal(glyph().style.color,config.theme.red);assert.equal(glyph().style.opacity,1);
 event(close,"mouseLeave");assert.equal(glyph().style.color,config.theme.muted);assert.equal(glyph().style.opacity,0);assert.equal(glyph().style.fontSize,normalSize);
 // Also cover parent leave before child enter, and child exit back onto tab.
 event(tab,"mouseEnter");event(tab,"mouseLeave");event(close,"mouseEnter");assert.equal(glyph().style.color,config.theme.red);assert.equal(glyph().style.opacity,1);
 event(close,"mouseLeave");assert.equal(glyph().style.opacity,0);
 event(tab,"mouseEnter");event(close,"mouseEnter");event(close,"mouseLeave");assert.equal(glyph().style.color,config.theme.muted);assert.equal(glyph().style.opacity,1);
 event(tab,"mouseLeave");assert.equal(glyph().style.opacity,0);
}
try{
 root.render(()=><div>
  <WorkspaceTabs config={config} workspaces={workspaces} activeId="b" onActivate={()=>{}} onNew={()=>{}} onClose={()=>{}} registerHost={()=>{}} registerNewHost={()=>{}} onBounds={()=>{}} onScroll={()=>{}}/>
  <PaneLayoutView controller={layout} config={config} renderSurface={()=><div/>} onClose={()=>{}}/>
 </div>);await host.pump(root);
 verify(`pane-tab:${inactive}`,`pane-close:${inactive}`);verify("workspace:a","workspace-close:a");
 console.log(JSON.stringify({noWindow:true,actualSolidBufferAndWorkspaceTabs:true,parentLeaveCannotClearChildHover:true,bothEventOrdersRemainRed:true,childLeaveRestoresMutedAndHidden:true,childExitOntoTabRemainsVisible:true}));
}finally{root.unmount();layout.dispose();}
