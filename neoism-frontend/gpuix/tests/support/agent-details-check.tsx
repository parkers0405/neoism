import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {dimensions as d} from "../../src/design/generated";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const config={...loadConfig(["--config","/nonexistent/agent-details"],{}),reducedMotion:true};
for(const width of [1440,700,400]){
  const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),host=createRecordingAppRenderer();
  host.renderer.getWindowSize=()=>({width,height:900});
  const root=createRoot(host.renderer),workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};
  model.update({activeWorkspace:workspace,workspaces:[workspace],loading:false,ui:{sidebar:"files",openWorkspaceIds:["a"]}});
  const surface=model.newAgentHome()!;
  model.update({surfaces:model.state.surfaces.map(item=>item.id===surface.id?{...item,sessionId:"details"}:item),history:{details:[{id:"answer",role:"Assistant",kind:"Assistant",text:"A conversation with session details."}]}});
  const toggle=()=>{
    const button=host.find("toggle-details");assert.ok(button,"The details toggle must stay available");
    root.dispatch({elementId:button.id,eventType:"mouseDown",button:0});
  };
  try{
    root.render(()=><App model={model} config={config}/>);await host.pump(root);
    assert.equal(model.state.ui.details,false);assert.ok(!host.find("agent-details"),"Details must start collapsed");
    toggle();await host.pump(root);
    const rail=host.find("agent-details");assert.ok(rail,`Details must open at window width ${width}`);
    const available=width-Math.min(d.sidebarWidth,width*.42);
    assert.ok(Number(rail.style.width)>0&&Number(rail.style.width)<=available,"Rail must fit beside the visible sidebar");
    assert.equal(rail.style.position,width===1440?undefined:"absolute","Narrow details must overlay rather than squeeze the chat");
    toggle();await host.pump(root);assert.ok(!host.find("agent-details"),"The same toggle must close the overlay");
  }finally{root.unmount();model.dispose();client.stop();}
}