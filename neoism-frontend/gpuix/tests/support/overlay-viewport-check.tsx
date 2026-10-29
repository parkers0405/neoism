import assert from "node:assert/strict";
import {mkdtempSync,rmSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {createRoot,type NativeRenderer} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {CommandView} from "../../src/commands/CommandView";
import {CommandController} from "../../src/commands/CommandController";
import {InlinePicker} from "../../src/ui/InlinePicker";
import {TopBar} from "../../src/ui/TopBar";
import {DirectoryBrowserView} from "../../src/agent/DirectoryBrowserView";
import {ChromeActionsView} from "../../src/menu-actions/ChromeActionsView";
import {ChromeActionsController} from "../../src/menu-actions/ChromeActionsController";
import {setDialogViewport,setOverlayViewport,overlayViewport,type OverlayViewport} from "../../src/ui/overlay-viewport";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
interface Node{id:number;type:string;children:number[];props:Record<string,unknown>;style:Record<string,unknown>}
const nodes=new Map<number,Node>(),commands=new CommandController(),config={...loadConfig(["--config","/nonexistent/overlay-contract"],{}),reducedMotion:true};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),chrome=new ChromeActionsController({model,config,reload:()=>{}});
await chrome.open("OpenAbout");
const callbacks=new Set<()=>void>(),resetSchedule=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>{callbacks.delete(callback);};});
const rootDir=mkdtempSync(join(tmpdir(),"neoism-overlay-"));
const remove=(id:number,removed:number[])=>{const node=nodes.get(id);if(!node)return;for(const child of [...node.children])remove(child,removed);nodes.delete(id);for(const parent of nodes.values())parent.children=parent.children.filter(child=>child!==id);removed.push(id);};
const renderer={getWindowSize:()=>({width:1440,height:1000}),setWindowKeyEvents(){},getFocusedElementId:()=>undefined,focusElement(){},putPaintImage(){},dropPaintImage(){},applyBatch(json:string){const removed:number[]=[];for(const[op,id,a,b]of JSON.parse(json)){if(op==="createElement")nodes.set(id,{id,type:a,children:[],props:{},style:{}});else if(op==="destroyElement")remove(id,removed);else if(op==="setStyle")nodes.get(id)!.style=a;else if(op==="setCustomProp")nodes.get(id)!.props[a]=b;else if(op==="appendChild"||op==="insertBefore"){for(const node of nodes.values())node.children=node.children.filter(child=>child!==a);const parent=nodes.get(id)!;if(op==="appendChild")parent.children.push(a);else parent.children.splice(parent.children.indexOf(b),0,a);}}return removed;}} as unknown as NativeRenderer;
const root=createRoot(renderer),[menu,setMenu]=createSignal(false),[anchor,setAnchor]=createSignal<[number,number,number,number]>([300,690,700,100]);
const actions:string[]=[],selected:string[]=[];
function Fixture(){return <div>
 <CommandView controller={commands} config={config} onAccept={()=>{}} onClose={()=>{}}/>
 <InlinePicker config={config} kind="agent" title="Slash commands" anchor={anchor()} options={Array.from({length:30},(_,i)=>({title:`Command ${i}`,value:String(i)}))} onSelect={value=>selected.push(value)} onClose={()=>{}}/>
 <InlinePicker config={config} kind="permission" title="Allow tool?" anchor={anchor()} options={[{title:"Allow",value:"Yes"},{title:"Deny",value:"No"}]} onSelect={()=>{}} onClose={()=>{}}/>
 <InlinePicker config={config} kind="question" anchor={anchor()} question={{id:"one",sessionID:"thread",questions:[{question:"Which option?",options:[{label:"First"},{label:"Second"}]}]}} onClose={()=>{}}/>
 <ChromeActionsView controller={chrome} config={config}/>
 <DirectoryBrowserView config={config} root={rootDir} start={rootDir} onDone={()=>{}}/>
 <TopBar config={config} details={false} agent={true} search={false} connection="connected" menuOpen={menu()} onMenuChange={setMenu} onAction={action=>{actions.push(action);}} report={error=>{throw error;}}/>
 </div>;}
const byTest=(id:string)=>[...nodes.values()].find(node=>node.props.testId===id)!;
const parent=(node:Node)=>[...nodes.values()].find(candidate=>candidate.children.includes(node.id));
function origin(node:Node):{x:number;y:number}{const owner=parent(node),base=owner&&owner.type!=="anchored"?origin(owner):{x:0,y:0};return{x:base.x+Number(node.style.left||0),y:base.y+Number(node.style.top||0)};}
const dialogs=["command-dialog","agent-picker:agent","agent-picker:permission","agent-question","chrome-action-overlay","directory-browser","original-hamburger-menu"];
let checks=0;
try{
 let bounds:OverlayViewport={x:220,y:132,width:1080,height:730};setOverlayViewport(renderer,bounds);root.render(()=><Fixture/>);
 root.dispatch({elementId:byTest("topbar-menu").id,eventType:"mouseDown",button:0,x:15,y:15});
 for(const [next,composerOffset] of [[bounds,590],[{x:360,y:246,width:820,height:610},180]] as const){
  bounds=next;root.flushSync(()=>{setOverlayViewport(renderer,next);setAnchor([next.x+40,next.y+composerOffset,next.width-80,100]);});await Promise.resolve();root.flush();
  assert.equal(overlayViewport(renderer)()?.y,next.y);
  for(const id of dialogs){const node=byTest(id);assert.ok(node,`${id} must mount its real original component`);const rect=origin(node);assert.ok(rect.y>=next.y-0.1,`${id} overlaps reserved headers: ${rect.y} < ${next.y}`);assert.ok(rect.x>=(["command-dialog","chrome-action-overlay","directory-browser"].includes(id)?0:next.x)-0.1,`${id} starts outside its intended viewport`);checks++;}
  const centered=()=>{const dialog=byTest("command-dialog"),rect=origin(dialog);assert.ok(Math.abs(rect.x+Number(dialog.style.width)/2-720)<.1,"Command palette must center horizontally in the full window");assert.ok(Math.abs(rect.y-(next.y+commands.frame.rect[1]))<.1,"Command palette retains its original upper native anchor");};
  centered();commands.query("q");root.flush();centered();commands.query("");root.flush();centered();
  for(const id of ["agent-picker:agent","agent-picker:permission","agent-question"]){const node=byTest(id),rect=origin(node);assert.ok(rect.y+Number(node.style.height)<=anchor()[1]+.1,`${id} must stay above the composer`);}
  // Select the actual first picker row using its native global hit geometry.
  const picker=byTest("agent-picker:agent"),point=origin(picker);root.dispatch({elementId:picker.id,eventType:"mouseDown",button:0,x:point.x+30,y:point.y+76});
 }
 assert.deepEqual(selected,["0","0"]);
 await chrome.open("OpenThemes");root.flush();
 for(const next of [{x:0,y:132,width:1300,height:730},{x:360,y:246,width:820,height:610}]){
  root.flushSync(()=>setOverlayViewport(renderer,next));await Promise.resolve();root.flush();
  const rect=chrome.frame?.rect;assert.ok(rect,"Appearance palette publishes its original native frame");
  const paint=origin(byTest("chrome-action-paint-origin"));
  assert.ok(Math.abs(paint.x+rect[0]+rect[2]/2-720)<.1,"Appearance palette stays centered in the full window across sidebar changes");
  assert.ok(Math.abs(paint.y+rect[1]-(next.y+rect[1]))<.1,"Appearance palette keeps its original high native position");
  root.dispatch({elementId:byTest("chrome-action-overlay").id,eventType:"mouseDown",button:0,x:paint.x+rect[0]+30,y:paint.y+rect[1]+30});root.flush();
  assert.ok(chrome.frame?.active,"Centered palette input click must retain its original native hit target rather than close as an outside click");
 }
 // A bottom/right split must not drag global dialogs down or sideways.
 root.flushSync(()=>{setDialogViewport(renderer,{x:0,y:86,width:1440,height:892});setOverlayViewport(renderer,{x:950,y:600,width:490,height:250});});
 let command=byTest("command-dialog"),point=origin(command);
 assert.ok(Math.abs(point.x+Number(command.style.width)/2-720)<.1);assert.equal(point.y,86+commands.frame.rect[1]);
 root.flushSync(()=>setDialogViewport(renderer,{x:0,y:86,width:900,height:600}));
 command=byTest("command-dialog");point=origin(command);assert.ok(Math.abs(point.x+Number(command.style.width)/2-450)<.1);assert.ok(point.y>=86);
 root.flushSync(()=>{setDialogViewport(renderer,undefined);setOverlayViewport(renderer,{x:360,y:246,width:820,height:610});});
 const item=[...nodes.values()].find(node=>String(node.props.testId).startsWith("menu:"))!;assert.ok(item);const target=origin(item);root.dispatch({elementId:item.id,eventType:"mouseDown",button:0,x:target.x+5,y:target.y+5});assert.equal(actions.length,1,"Translated hamburger row must dispatch its original action");
 console.log(JSON.stringify({dialogs:dialogs.length,boundsChanges:2,checks,nativePickerSelections:selected.length,hamburgerActions:actions.length,nativeWindowsCreated:0}));
}finally{root.unmount();commands.dispose();chrome.dispose();model.dispose();setOverlayViewport(renderer,undefined);setDialogViewport(renderer,undefined);resetSchedule();rmSync(rootDir,{recursive:true,force:true});}
