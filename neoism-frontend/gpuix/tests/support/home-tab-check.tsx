import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {PaneLayoutView} from "../../src/layout/PaneLayoutView";
import {createRecordingAppRenderer,type RecordedAppNode} from "./full-app-recording-renderer";
const config={...loadConfig(["--config","/nonexistent/home-tab"],{}),reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),restored=new WorkspaceModel(client,config),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const below=(node:RecordedAppNode):RecordedAppNode[]=>[node,...node.children.flatMap(id=>below(host.nodes.get(id)!))];
try{
 const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};model.update({activeWorkspace:workspace,workspaces:[workspace]});model.ensureWelcome();const first=model.state.surfaces[0],second=model.newWelcome()!;const agent=model.newAgentHome()!;
 assert.equal(first.title,"Home");assert.equal(second.title,"Home");
 const saved=model.captureSession();for(const surface of saved.surfaces)if(surface.kind==="welcome")surface.title="Welcome";
 const layouts=saved.ui.layouts as Record<string,{routes:Record<string,{kind:string;title:string}>}>;for(const route of Object.values(layouts.a.routes))if(route.kind==="welcome")route.title="Welcome";
 restored.restoreSession(saved);restored.update({activeWorkspace:workspace,workspaces:[workspace]});
 assert.deepEqual(restored.state.surfaces.filter(surface=>surface.kind==="welcome").map(surface=>[surface.id,surface.title]),[[first.id,"Home"],[second.id,"Home"]]);
 const layout=restored.getLayout();layout.layout(600,400);layout.focusSurface(first.id);
 const tabs=layout.frame.panes[0].tabs;assert.ok(tabs.filter(tab=>tab.kind==="welcome").every(tab=>tab.title==="Home"));
 root.render(()=><PaneLayoutView controller={layout} config={config} onClose={()=>{}} onNewWelcome={()=>{}} renderSurface={()=><div/>}/>);await host.pump(root);
 root.dispatch({elementId:host.find("pane-layout")!.id,eventType:"layout",x:10,y:50,width:600,height:400});
 for(let i=0;i<tabs.length;i++)root.dispatch({elementId:host.find(`pane-tab:${tabs[i].leaf}`)!.id,eventType:"layout",x:10+i*100,y:50,width:100,height:28});root.flush();
 const home=tabs.find(tab=>tab.surfaceId===first.id)!,agentTab=tabs.find(tab=>tab.surfaceId===agent.id)!;
 const icon=host.find(`pane-tab-icon:${home.leaf}`)!;assert.equal(icon.type,"text");assert.ok(below(icon).some(node=>node.text==="\uf015"));assert.equal(host.find(`pane-tab:${home.leaf}`)!.props["aria-label"],"Home");
 assert.equal(host.find(`pane-tab-icon:${agentTab.leaf}`)!.type,"img","Agent keeps its own Neoism logo");assert.equal(host.find(`pane-new-welcome:${layout.frame.focusedLeaf}`)!.props["aria-label"],"New Home");
 const tab=host.find(`pane-tab:${home.leaf}`)!;root.dispatch({elementId:tab.id,eventType:"mouseDown",button:0,x:35,y:64});root.dispatch({elementId:tab.id,eventType:"mouseMove",pressedButton:0,x:70,y:64});root.flush();
 const ghost=host.find("buffer-tab-drag-ghost")!;assert.ok(ghost);assert.ok(below(ghost).some(node=>node.type==="text"&&node.text==="\uf015"));assert.ok(below(ghost).some(node=>node.type==="text"&&node.text==="Home"));assert.ok(!below(ghost).some(node=>node.type==="img"));
 console.log(JSON.stringify({noWindow:true,newHomeTitles:true,legacyTitlesMigratedWithoutNewIds:true,homeGlyphOnTabAndDrag:true,agentLogoUnchanged:true,newHomeLabel:true}));
}finally{root.unmount();model.dispose();restored.dispose();client.stop();}
