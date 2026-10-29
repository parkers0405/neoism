import assert from"node:assert/strict";
import{createRoot}from"@gpuix/solid";
import{App}from"../../src/ui/App";
import{AppVisualState}from"../../src/ui/AppVisualState";
import{loadConfig}from"../../src/config";
import{DaemonConnection}from"../../src/daemon/client";
import{WorkspaceModel}from"../../src/model";
import{createAgentDaemon}from"./agent-daemon";
import{createRecordingAppRenderer,type RecordedAppNode}from"./full-app-recording-renderer";
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/composer-responsive-app"],{}),endpoint:daemon.endpoint,scale:1.67,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
let stage="startup";
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await host.pump(root);if(Date.now()>end)throw new Error("Responsive composer App did not settle: "+stage);await Bun.sleep(3);}await host.pump(root);};
const number=(id:string,key:string)=>Number(host.find(id)!.style[key]);
const parent=(node:RecordedAppNode)=>[...host.nodes.values()].find(candidate=>candidate.children.includes(node.id));
const descendants=(node:RecordedAppNode):RecordedAppNode[]=>node.children.flatMap(id=>{const child=host.nodes.get(id)!;return[child,...descendants(child)];});
/** Retained-policy proof, not a replacement GPUI hit-test engine. The pinned
 * renderer style::should_occlude explicitly returns false for none and true
 * for auto; default absolute boxes occlude even without a click listener.
 * Requiring explicit policy throughout each real visual subtree avoids
 * guessing native glyph bounds or simulating a GPU scene in this recorder. */
const pointerOwner=(kind:string,region:"logo"|"label"|"caret"|"padding")=>{
 const button=host.find(`select-${kind}`)!,row=host.find("composer-chip-row")!,card=host.find("agent-input-card")!;
 assert.equal(button.style.pointerEvents,"auto");assert.ok(button.events.has("click"));assert.equal(parent(button)?.id,row.id);assert.equal(row.style.pointerEvents,"none");assert.equal(parent(row)?.id,card.id);
 for(const child of descendants(button)){if(child.type==="text"&&!Object.keys(child.style).length){assert.equal(child.events.size,0,"Raw text content has no independent hit listener");continue;}assert.equal(child.style.pointerEvents,"none",`${kind} ${child.type} decorative descendant must not create an independent native hitbox`);}
 const label=parent(host.find(`agent-chip-label:${kind}`)!)!,logo=kind==="model"?parent(host.find("composer-provider-logo")!):undefined;
 const caret=button.children.map(id=>host.nodes.get(id)!).find(node=>node.style.position==="absolute"&&node.id!==label.id&&node.id!==logo?.id)!;
 const wrapper=region==="logo"?logo:region==="label"?label:region==="caret"?caret:button;assert.ok(wrapper,`Missing ${kind} ${region} visual`);
 const width=Number(button.style.width),height=Number(button.style.height);
 let x=Number(wrapper.style.left||0)+Number(wrapper.style.width)/2,y=Number(wrapper.style.top||0)+Number(wrapper.style.height)/2;
 if(region==="padding"){
  const intervals=button.children.map(id=>host.nodes.get(id)!).filter(node=>node.style.position==="absolute").map(node=>[Number(node.style.left),Number(node.style.left)+Number(node.style.width)]).sort((a,b)=>a[0]-b[0]);let end=0;const gaps:number[][]=[];
  for(const[left,right]of intervals){if(left>end)gaps.push([end,left]);end=Math.max(end,right);}if(end<width)gaps.push([end,width]);gaps.sort((a,b)=>(b[1]-b[0])-(a[1]-a[0]));assert.ok(gaps[0]?.[1]>gaps[0]?.[0],`${kind} retains a real padding/gap hit area`);x=(gaps[0][0]+gaps[0][1])/2;y=height/2;
 }
 assert.ok(x>=0&&x<width&&y>=0&&y<height,`${kind} ${region} source geometry lies inside the button`);
 let owner:RecordedAppNode|undefined=wrapper;
 while(owner&&owner.id!==button.id){assert.equal(owner.style.pointerEvents,"none",`${kind} ${region} native decorative policy`);owner=parent(owner);}
 assert.equal(owner?.id,button.id,`${kind} ${region} first occluding ancestor is the real control`);return button;
};
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
 // Cover the visible provider logo, all three labels/carets and actual hit
 // padding. Resolve ownership from retained native pointer policy before
 // dispatching the resolved control's real handler; never dispatch a child ID
 // directly and call that a physical pointer test.
 for(const kind of["agent","model","thinking"]){
  for(const region of[...(kind==="model"?["logo"as const]:[]),"label"as const,"caret"as const,"padding"as const]){
   stage=`pointer policy ${kind} ${region}`;const control=pointerOwner(kind,region);root.dispatch({elementId:control.id,eventType:"click",button:0});await until(()=>!!host.find(`agent-picker:${kind}`));
   assert.equal(host.find("composer")!.id,originalInput);assert.equal(host.find("composer")!.props.value,"Keep my draft while resizing");await closePicker(kind);assert.equal(host.find("composer")!.props.value,"Keep my draft while resizing");
  }
 }
 const attachment=host.find("attach-files")!;assert.equal(parent(attachment)?.style.pointerEvents,"auto","Attachment retains an explicit native pointer target");
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
