import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal,Show} from "solid-js";
import {PaneLayoutView} from "../../src/layout/PaneLayoutView";
import {WorkspaceLayout} from "../../src/layout/WorkspaceLayout";
import {loadConfig} from "../../src/config";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const config=loadConfig(["--config","/nonexistent/workspace-firstpaint"],{}),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const a=new WorkspaceLayout(),b=new WorkspaceLayout(),fresh=new WorkspaceLayout();
for(const [layout,id,kind]of [[a,"a","agent"],[b,"b","file"],[fresh,"new","welcome"]]as const)layout.open({id,title:id,kind});
a.layout(900,600,config.scale);b.layout(1100,628,config.scale);
const [selected,select]=createSignal(a),captured:Array<{id:string;width:number;height:number}>=[];
const measure=(width:number,height:number)=>{root.dispatch({elementId:host.find("pane-layout")!.id,eventType:"layout",width,height,x:0,y:60});root.flush();};
try{
 root.render(()=><Show when={selected()} keyed>{layout=><PaneLayoutView controller={layout} config={config} onClose={()=>{}} renderSurface={id=>{captured.push({id,...layout.measuredViewport});return <div testId={`surface:${id}`}/>;}}/>}</Show>);root.flush();measure(900,600);
 select(b);root.flush();assert.deepEqual(b.measuredViewport,{width:1100,height:628,scale:config.scale});measure(1100,628);
 select(a);root.flush();assert.deepEqual(a.measuredViewport,{width:900,height:600,scale:config.scale},"Returning chat must not borrow the outgoing editor's one-row-taller viewport");
 assert.deepEqual(captured.at(-1),{id:"a",width:900,height:600,scale:config.scale});
 select(fresh);root.flush();assert.deepEqual(captured.at(-1),{id:"new",width:1100,height:628,scale:config.scale},"Never-visited pages receive the size before their component captures it");
 console.log("Workspace first-frame geometry is stable for returning and new surfaces");
}finally{root.unmount();for(const layout of [a,b,fresh])layout.dispose();}
