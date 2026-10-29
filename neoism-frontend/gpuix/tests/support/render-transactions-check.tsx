import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {App} from "../../src/ui/App";
import {AppGeneration} from "../../src/ui/AppGeneration";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {palettes} from "../../src/design/generated";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";

const config={...loadConfig(["--config","/nonexistent/render-transactions"],{}),reducedMotion:true,statusFps:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),images=new Set<number>();
const refs=()=>new Set([...host.nodes.values()].filter(node=>node.type==="canvas").flatMap(node=>(JSON.parse(String(node.props.commands||"[]"))as Array<{k:string;id?:number}>).filter(op=>op.k==="i").map(op=>op.id!)));
let uploads=0,drops=0,checkedBatches=0,drawn=0;
Object.assign(host.renderer,{getRenderedFrameCount:()=>drawn});
const native=host.renderer as unknown as {putPaintImage:(id:number,w:number,h:number,pixels:Buffer)=>void;dropPaintImage:(id:number)=>void;applyBatch:(json:string)=>number[]};
native.putPaintImage=(id,w,h,pixels)=>{assert.equal(pixels.length,w*h*4);images.add(id);uploads++;};
native.dropPaintImage=id=>{assert.ok(!refs().has(id),`Committed canvas still needs image ${id}; cleanup ran before its replacement batch`);images.delete(id);drops++;};
const apply=native.applyBatch.bind(native);native.applyBatch=json=>{const result=apply(json);for(const id of refs())assert.ok(images.has(id),`Mutation batch referenced missing texture ${id}`);checkedBatches++;return result;};
const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};
const listing=(prefix:string,count=35)=>({"":{path:"",entries:Array.from({length:count},(_,index)=>({name:`${prefix}-${index}.ts`,is_dir:false}))}});
model.update({activeWorkspace:workspace,workspaces:[workspace],directories:listing("first"),expanded:new Set([""]),loading:false,ui:{sidebar:"files",details:false,openWorkspaceIds:["a"]}});
model.newAgentHome();
const [generation,setGeneration]=createSignal(config),root=createRoot(host.renderer);
const pending=new Set<()=>void>(),release=usePresentationScheduler(callback=>{pending.add(callback);return()=>{pending.delete(callback);};});
let themeSwaps=0,fontSwaps=0;
try{
 root.render(()=><AppGeneration value={generation()} backgroundColor={generation().theme.bg}>{current=><App model={model} config={current} visuals={visuals}/>}</AppGeneration>);
 await host.pump(root);assert.ok(host.find("file:first-0.ts"));assert.ok(host.find("agent-composer-shell"),"Must exercise real Agent input, not only isolated paint widgets");
 for(const themeName of ["tokyo_night","ayu_light","pastel_dark","tokyo_night"]as const){
  const oldColor=generation().theme.bg;
  root.flushSync(()=>setGeneration({...generation(),themeName,theme:{...palettes[themeName]}}));await host.pump(root);
  assert.ok(host.find("app-generation-pending"),"Must exercise real presented-frame staging, not a headless instantaneous swap");
  assert.equal(host.find("app-shell")!.style.backgroundColor,oldColor,"Outgoing theme ink must retain its own background until replacement promotes");
  drawn+=6;await Bun.sleep(4);await host.pump(root);assert.ok(!host.find("app-generation-pending"));
  assert.equal(host.find("app-shell")!.style.backgroundColor,palettes[themeName].bg);assert.ok(refs().size>10);themeSwaps++;
 }
 for(const fontSize of [15,16,15,14]){root.flushSync(()=>setGeneration({...generation(),fontSize,scale:fontSize/14}));await host.pump(root);drawn+=6;await Bun.sleep(4);await host.pump(root);assert.ok(host.find("agent-composer-shell"));fontSwaps++;}
 // Host-confirmed terminal cwd updates change the tree scope/list without
 // clicking or recreating its surface. Every visible new row must paint now.
 const tree=host.find("file-tree")!.id;
 root.dispatch({elementId:tree,eventType:"layout",x:0,y:90,width:300,height:1500});root.flush();
 root.flushSync(()=>model.update({activeWorkspace:{...workspace,root_dir:"/project/changed"},directories:listing("changed",80)}));
 // Do not deliver another layout/click: the existing host did not resize.
 root.flush();
 assert.equal(host.find("file-tree")!.id,tree);assert.ok(!host.find("file:first-0.ts"));assert.ok(host.find("file:changed-0.ts"));
 const rows=[...host.nodes.values()].filter(node=>String(node.props.testId).startsWith("file:changed-"));assert.ok(rows.length>50,"New cwd must retain measured1500px host height; a stale600px default leaves the lower half empty");
 for(const row of rows){const glyph=host.nodes.get(host.nodes.get(row.children[0])!.children[0])!;assert.equal(glyph.type,"canvas");const ops=JSON.parse(String(glyph.props.commands));assert.ok(ops.some((op:{k:string;id:number})=>op.k==="i"&&images.has(op.id)),"New cwd rows must already paint real live glyphs without click invalidation");}
 root.unmount();visuals.dispose();model.dispose();assert.equal(images.size,0);assert.ok(drops>0);assert.ok(uploads>0);
 console.log(JSON.stringify({noWindow:true,actualApp:true,themeSwaps,fontSwaps,cwdDirectoryReplacement:true,committedImagesNeverDropped:true,allImagesReleased:true,checkedBatches,uploads,drops}));
}finally{root.unmount();visuals.dispose();model.dispose();release();}
