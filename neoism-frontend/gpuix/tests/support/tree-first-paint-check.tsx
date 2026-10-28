import assert from "node:assert/strict";
import {createRoot,type NativeRenderer} from "@gpuix/solid";
import {createSignal} from "solid-js";
import {App} from "../../src/ui/App";
import {AppGeneration} from "../../src/ui/AppGeneration";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {WorkspaceModel} from "../../src/model";
import {DaemonConnection} from "../../src/daemon/client";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
interface Node{id:number;type:string;children:number[];props:Record<string,unknown>;style:Record<string,unknown>}
const nodes=new Map<number,Node>(),snapshots:Map<number,Node>[]=[],pixels=new Set<number>();let focused:number|null=null;
const remove=(id:number,destroyed:number[])=>{for(const child of [...(nodes.get(id)?.children||[])])remove(child,destroyed);nodes.delete(id);for(const parent of nodes.values())parent.children=parent.children.filter(child=>child!==id);destroyed.push(id);};
const renderer={getWindowSize:()=>({width:1440,height:900}),setWindowKeyEvents(){},getFocusedElementId:()=>focused,getFocusedElementIdSnapshot:()=>focused,focusElement(id:number){focused=id;},putPaintImage(id:number,width:number,height:number,bytes:Buffer){assert.equal(bytes.length,width*height*4);pixels.add(id);},dropPaintImage(id:number){pixels.delete(id);},applyBatch(json:string){const destroyed:number[]=[];for(const[op,id,a,b]of JSON.parse(json)){
 if(op==="createElement")nodes.set(id,{id,type:a,children:[],props:{},style:{}});else if(op==="destroyElement")remove(id,destroyed);else if(op==="setStyle")nodes.get(id)!.style=a;else if(op==="setCustomProp")nodes.get(id)!.props[a]=b;else if(op==="appendChild"||op==="insertBefore"){for(const node of nodes.values())node.children=node.children.filter(child=>child!==a);const parent=nodes.get(id)!;if(op==="appendChild")parent.children.push(a);else parent.children.splice(parent.children.indexOf(b),0,a);}
 }snapshots.push(new Map([...nodes].map(([id,node])=>[id,structuredClone(node)])));return destroyed;}}as unknown as NativeRenderer;
const config={...loadConfig(["--config","/nonexistent/tree-first-paint"],{}),reducedMotion:false},client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState();
const workspace={id:"a",host_id:"host",title:"A",root_dir:"/project/a"};
const entries=[{name:"src",is_dir:true},{name:"界-café.ts",is_dir:false},...Array.from({length:18},(_,index)=>({name:`file-${index}.ts`,is_dir:false}))];
const directories={"":{path:"",entries},src:{path:"src",entries:[{name:"nested.ts",is_dir:false}]}};
model.update({activeWorkspace:workspace,workspaces:[workspace],directories,expanded:new Set(["","src"]),loading:false,ui:{sidebar:null,details:false,openWorkspaceIds:["a"]}});
const [generation,setGeneration]=createSignal(config),root=createRoot(renderer),callbacks=new Map<number,()=>void>();let next=0;
const reset=usePresentationScheduler(callback=>{const id=++next;callbacks.set(id,callback);return()=>{callbacks.delete(id);};});
const find=(snapshot:Map<number,Node>,testId:string)=>[...snapshot.values()].find(node=>node.props.testId===testId);
const descend=(snapshot:Map<number,Node>,node:Node):Node[]=>node.children.flatMap(id=>{const child=snapshot.get(id);return child?[child,...descend(snapshot,child)]:[];});
const toggle=()=>root.dispatch({elementId:find(nodes,"toggle-files")!.id,eventType:"mouseDown",button:0,x:50,y:15});
const motion=(node:Node)=>typeof node.props.motion==="string"?JSON.parse(node.props.motion):node.props.motion as {initial:unknown;transition:{duration:number;delay?:number}};
let glyphs=0;
function checkPaint(snapshot:Map<number,Node>,slide:boolean){
 const tree=find(snapshot,"file-tree")!;assert.ok(tree,"Tree host must be in its first opening transaction");
 const rows=descend(snapshot,tree).filter(node=>String(node.props.testId).startsWith("file:"));assert.equal(rows.length,21);
 for(const row of rows){assert.equal(row.props.motion,undefined,"Rows must not have independent delayed opacity animations");assert.notEqual(row.style.opacity,0);
  const canvas=descend(snapshot,row).find(node=>node.type==="canvas")!;assert.ok(canvas);const ops=JSON.parse(String(canvas.props.commands||"[]"));assert.equal(ops.length,1,"Every visible cached row must paint a glyph image before any layout event");assert.equal(ops[0].k,"i");assert.ok(ops[0].b[2]>0);assert.ok(pixels.has(ops[0].id));glyphs++;
 }
 const animation=motion(tree);assert.ok(animation);
 if(slide){assert.deepEqual(animation.initial,{left:-6});assert.equal(animation.transition.duration,.12);}else assert.equal(animation.initial,false,"Zoom must not replay the panel opening slide");
}
try{
 root.render(()=><AppGeneration value={generation()}>{current=><App model={model} config={current} visuals={visuals}/>}</AppGeneration>);
 assert.ok(!find(nodes,"file-tree"));let before=snapshots.length;toggle();await Promise.resolve();root.flush();
 const opened=snapshots.slice(before).find(snapshot=>find(snapshot,"file-tree"))!;checkPaint(opened,true);
 const originalTimes=[...visuals.treeReveals("a",false,"/project/a")];
 before=snapshots.length;root.flushSync(()=>setGeneration({...config,fontSize:15,scale:15/14}));await Promise.resolve();root.flush();
 const zoomed=snapshots.slice(before).find(snapshot=>find(snapshot,"file-tree"))!;checkPaint(zoomed,false);assert.deepEqual([...visuals.treeReveals("a",false,"/project/a")],originalTimes);
 toggle();await Promise.resolve();root.flush();assert.ok(!find(nodes,"file-tree"));before=snapshots.length;toggle();await Promise.resolve();root.flush();
 const reopened=snapshots.slice(before).find(snapshot=>find(snapshot,"file-tree"))!;const reopenedMotion=motion(find(reopened,"file-tree")!);assert.ok(reopenedMotion.initial!==false,"An actual close/reopen keeps the short coherent slide");
 // A pending uncached root must paint a visible placeholder immediately even
 // when unrelated startup work has already cleared the global loading bit.
 before=snapshots.length;root.flushSync(()=>model.update({directories:{},loading:false,error:undefined}));
 const pending=snapshots.slice(before).find(snapshot=>find(snapshot,"loading-skeleton:tree"))!,skeleton=find(pending,"loading-skeleton:tree")!;assert.ok(skeleton);
 const bars=descend(pending,skeleton).filter(node=>typeof node.style.backgroundColor==="string");assert.ok(bars.length>0,"Cold remote directories must paint real skeleton quads before layout");assert.ok(bars.every(bar=>Number(String(bar.style.backgroundColor).match(/,([\d.]+)\)$/)?.[1])>0));
 root.flushSync(()=>model.update({directories}));assert.ok(!find(nodes,"loading-skeleton:tree"));assert.ok(find(nodes,"file:src/nested.ts"));
 before=snapshots.length;root.flushSync(()=>setGeneration({...config,reducedMotion:true}));await Promise.resolve();root.flush();
 const reduced=snapshots.slice(before).find(snapshot=>find(snapshot,"file-tree"))!;assert.equal(motion(find(reduced,"file-tree")!).initial,false);assert.equal(motion(find(reduced,"file-tree")!).transition.duration,0);
 console.log(JSON.stringify({firstPaintRows:21,glyphsBeforeLayout:glyphs,coldSkeletonBars:bars.length,openingMs:120,zeroOpacityRows:0,zoomReplay:false,closeReopenSlide:true,reducedMotion:true,nativeWindowsCreated:0}));
}finally{root.unmount();visuals.dispose();model.dispose();reset();assert.equal(callbacks.size,0);assert.equal(pixels.size,0);}
