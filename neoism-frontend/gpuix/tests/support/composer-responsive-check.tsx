import assert from"node:assert/strict";
import{createRoot}from"@gpuix/solid";
import{App}from"../../src/ui/App";
import{AppVisualState}from"../../src/ui/AppVisualState";
import{loadConfig}from"../../src/config";
import{DaemonConnection}from"../../src/daemon/client";
import{WorkspaceModel}from"../../src/model";
import{createAgentDaemon}from"./agent-daemon";
import{createRecordingAppRenderer}from"./full-app-recording-renderer";
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/composer-responsive-app"],{}),endpoint:daemon.endpoint,scale:1.67,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
let stage="startup";
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await host.pump(root);if(Date.now()>end)throw new Error("Responsive composer App did not settle: "+stage);await Bun.sleep(3);}await host.pump(root);};
const number=(id:string,key:string)=>Number(host.find(id)!.style[key]);
try{
 model.start();client.start();await until(()=>!!model.state.activeWorkspace&&!model.state.loading&&!model.state.catalogsLoading);model.newAgentHome();model.setSelection("model","openai/gpt-6.1-sol");model.setSelection("thinking","medium");
 root.render(()=><App model={model} config={config} visuals={visuals}/>);await host.pump(root);
 assert.equal(String(host.find("agent-input-card")!.style.backgroundColor).slice(-2),"d9","Composer material stays translucent while its foreground remains fully visible");
 root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:"Keep my draft while resizing"});await host.pump(root);
 const originalInput=host.find("composer")!.id;
 for(const logicalWidth of [240,310,360,440,800]){
  root.dispatch({elementId:host.find("agent-pane")!.id,eventType:"layout",x:0,y:0,width:logicalWidth*config.scale,height:600*config.scale});await host.pump(root);
  const rects=["select-agent","select-model","select-thinking"].map(id=>({id,left:number(id,"left"),width:number(id,"width")}));
  assert.ok(rects[0].left+rects[0].width<=rects[1].left+.001);assert.ok(rects[1].left+rects[1].width<=rects[2].left+.001);
  for(const rect of rects){assert.ok(rect.left>=0&&rect.left+rect.width<=number("composer-chip-row","width")+.001);assert.equal(host.find(rect.id)!.style.overflow,"hidden");}
  assert.equal(host.find("composer")!.id,originalInput);assert.equal(host.find("composer")!.props.value,"Keep my draft while resizing");
  const label=host.find("agent-chip-label:model")!;assert.equal(label.style.overflow,"hidden");assert.ok(Number(label.style.width)<=rects[1].width);
 }
 root.dispatch({elementId:host.find("select-thinking")!.id,eventType:"click",button:0});await until(()=>!!host.find("agent-picker:thinking"));
 const closePicker=async(kind:string)=>{stage="close "+kind;root.dispatch({elementId:host.find(`agent-picker:${kind}`)!.id,eventType:"keyDown",key:"escape",modifiers:{shift:false,ctrl:false,alt:false,cmd:false}});await until(()=>!host.find(`agent-picker:${kind}`));};
 await closePicker("thinking");
 model.setUI({recentModels:[{title:"Recent base",value:"openai/base",section:"Recent"}]});
 const rows=()=>[...host.nodes.values()].filter(node=>node.props.role==="option").map(node=>[node.props.testId,node.props["aria-label"]]);
 for(const [kind,command,title]of[["model","/models","Select model"],["agent","/agents","Agents"]]){
  stage="click "+kind;root.dispatch({elementId:host.find(`select-${kind}`)!.id,eventType:"click",button:0});await until(()=>!!host.find(`agent-picker:${kind}`)&&!model.state.catalogsLoading);
  const clicked=rows();assert.ok(clicked.length>1);assert.equal(host.find(`agent-picker:${kind}`)!.props["aria-label"],title);await closePicker(kind);
  stage="slash "+command;root.dispatch({elementId:host.find("composer")!.id,eventType:"change",value:command});await host.pump(root);root.dispatch({elementId:host.find("composer")!.id,eventType:"submit"});
  await until(()=>!!host.find(`agent-picker:${kind}`)&&!model.state.catalogsLoading);
  assert.equal(host.find(`agent-picker:${kind}`)!.props["aria-label"],title);assert.deepEqual(rows(),clicked,"Slash and clicked chips must expose identical current/recent/catalog rows");
  await closePicker(kind);assert.equal(host.find("composer")!.id,originalInput);
 }
 assert.equal(daemon.ptyRequests.length,0);assert.equal(daemon.requests.filter(request=>request.tag==="SubmitPrompt").length,0);
 console.log(JSON.stringify({actualApp:true,noWindow:true,narrowMeasuredControls:true,nonOverlappingChips:true,boundedInk:true,draftAndInputRetained:true,reasoningPickerWorks:true,slashAndChipPickersIdentical:true,noBackendSideEffects:true}));
}finally{root.unmount();visuals.dispose();model.dispose();client.stop();daemon.stop();}
