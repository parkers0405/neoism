import assert from "node:assert/strict";
import {createRoot,motion,type NativeRenderer,type HostElement} from "@gpuix/solid";
import {createSignal,onCleanup} from "solid-js";
import {AppGeneration} from "../../src/ui/AppGeneration";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {SplashView} from "../../src/ui/SplashView";
import {TreeRowGlyphs} from "../../src/ui/TreeRowGlyphs";
import {treeReveal} from "../../src/ui/tree-viewport";
import {loadConfig,type AppConfig} from "../../src/config";
import {coreCall} from "../../src/native-core";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import type {CanvasOp} from "../../src/ui/canvas";

interface Node {id:number;type:string;children:number[];props:Record<string,unknown>;style:Record<string,unknown>}
interface Snapshot {root?:number;nodes:Map<number,Node>;images:Set<number>}
function recordingRenderer(){
 const nodes=new Map<number,Node>(),snapshots:Snapshot[]=[],pixels=new Map<number,{hash:bigint|number;length:number}>(),uploads:number[]=[],drops:number[]=[];
 let root:number|undefined,attachments=0;
 const remove=(id:number,destroyed:number[])=>{const node=nodes.get(id);if(!node)return;for(const child of [...node.children])remove(child,destroyed);for(const parent of nodes.values())parent.children=parent.children.filter(child=>child!==id);nodes.delete(id);destroyed.push(id);};
 const renderer={
  // Deliberately no init/createWindow: this renderer only records GPUIX's
  // real Solid mutation transactions; its texture calls copy real FFI data.
  setWindowKeyEvents(enabled:boolean){if(enabled)attachments++;},
  putPaintImage(id:number,width:number,height:number,bytes:Buffer){assert.equal(bytes.length,width*height*4);assert.ok(bytes.some((byte,index)=>index%4===3&&byte>0));pixels.set(id,{hash:Bun.hash(bytes),length:bytes.length});uploads.push(id);},
  dropPaintImage(id:number){pixels.delete(id);drops.push(id);},
  applyBatch(json:string){
   const destroyed:number[]=[];
   for(const [op,id,a,b] of JSON.parse(json)){
    if(op==="createElement")nodes.set(id,{id,type:a,children:[],props:{},style:{}});
    else if(op==="destroyElement")remove(id,destroyed);
    else if(op==="setRoot")root=id;
    else if(op==="setStyle")nodes.get(id)!.style=a;
    else if(op==="setCustomProp")nodes.get(id)!.props[a]=b;
    else if(op==="appendChild"||op==="insertBefore"){
     for(const node of nodes.values())node.children=node.children.filter(child=>child!==a);
     const parent=nodes.get(id)!;if(op==="appendChild")parent.children.push(a);else parent.children.splice(parent.children.indexOf(b),0,a);
    }
   }
   const reachable=new Set<number>();const visit=(id:number)=>{if(reachable.has(id))return;const node=nodes.get(id);if(!node)return;reachable.add(id);node.children.forEach(visit);};if(root!==undefined)visit(root);
   snapshots.push({root,nodes:new Map([...nodes].filter(([id])=>reachable.has(id)).map(([id,node])=>[id,structuredClone(node)])),images:new Set(pixels.keys())});return destroyed;
  },
 };
 return {renderer:renderer as unknown as NativeRenderer,nodes,snapshots,pixels,uploads,drops,get attachments(){return attachments;}};
}
const tagged=(snapshot:Snapshot,id:string)=>[...snapshot.nodes.values()].find(node=>node.props.testId===id);
const children=(snapshot:Snapshot,node:Node):Node[]=>node.children.flatMap(id=>{const child=snapshot.nodes.get(id);return child?[child,...children(snapshot,child)]:[];});
const ops=(node:Node)=>JSON.parse(String(node.props.commands||"[]")) as CanvasOp[];
const canvasUnder=(snapshot:Snapshot,node:Node)=>children(snapshot,node).find(child=>child.type==="canvas")!;
const visuals=new AppVisualState(),host=recordingRenderer(),root=createRoot(host.renderer);
const callbacks=new Set<()=>void>(),restoreSchedule=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>{callbacks.delete(callback);};});
const base=loadConfig(["--config","/nonexistent/headless-zoom"],{}),context="welcome:headless",rowKey=JSON.stringify(["workspace",false,"/project","file.ts"]);
const config=(fontSize:number):AppConfig=>({...base,fontSize,scale:fontSize/14,reducedMotion:false,splashAutoAnimate:true});
const [generation,setGeneration]=createSignal({config:config(14)});
const revealedAt=performance.now()-10_000;
visuals.treeReveals("workspace",false,"/project").set("file.ts",revealedAt);
const mounted:number[]=[],disposed:number[]=[],rowRefs:HostElement[]=[];
function Generation(props:{config:AppConfig}){
 const cfg=props.config,glyphs=visuals.treeGlyphHost(cfg),revealStart=visuals.treeReveals("workspace",false,"/project").get("file.ts")!;
 const reveal=treeReveal(performance.now()-revealStart,0,360,12);mounted.push(cfg.fontSize);onCleanup(()=>disposed.push(cfg.fontSize));
 return <div testId="zoom-content">
  <SplashView config={cfg} contextId={context} wordmarkHosts={visuals.wordmarks} active={false} onAction={()=>{}}/>
  <motion.div ref={element=>rowRefs.push(element)} testId="zoom-tree-row" data-reveal-start={revealStart} initial={reveal.opacity>=1?false:{opacity:reveal.opacity}} animate={{opacity:1}} transition={{duration:reveal.duration,delay:reveal.delay}}>
   <TreeRowGlyphs host={glyphs} config={cfg} name="file.ts" chevron="" icon="\uf15b" labelColor={cfg.theme.dim} iconColor={cfg.theme.fg} chevronColor={cfg.theme.muted}
    initialWidth={visuals.treeWidths.get(rowKey)} onWidth={width=>visuals.treeWidths.set(rowKey,width)} pinImage={raster=>visuals.pinTreeRaster(host.renderer,rowKey,raster)}/>
  </motion.div>
 </div>;
}
let swaps=0,firstPaints=0,stableRasterReuse=0;
try{
 root.render(()=><AppGeneration value={generation()} background={{path:"/fixture/persistent-wallpaper.png",opacity:.3}}>{current=><Generation config={current.config}/>}</AppGeneration>);
 let snapshot=host.snapshots.at(-1)!,welcome=tagged(snapshot,"welcome-page")!,row=tagged(snapshot,"zoom-tree-row")!;
 const rowHost=children(snapshot,row).find(node=>node.props["aria-label"]==="file.ts")!;
 assert.ok(welcome&&rowHost,"Cold actual components must be mounted before layout");
 root.dispatch({elementId:welcome.id,eventType:"layout",x:220,y:80,width:900,height:600});
 root.dispatch({elementId:rowHost.id,eventType:"layout",x:20,y:80,width:260,height:26});
 await Promise.resolve();root.flush();
 snapshot=host.snapshots.at(-1)!;
 const shell=tagged(snapshot,"app-shell")!.id,wallpaper=tagged(snapshot,"app-wallpaper")!.id;
 const nativeWordmark=visuals.wordmarks.acquire(context,"splash");visuals.wordmarks.release(context);coreCall("wordmark.cancel",{id:nativeWordmark});
 // A random startup intro may deliberately hide every tile at progress zero.
 // Cancelling the native engine does not replace the canvas already recorded
 // by Solid. Paint the settled state through the real event/frame path before
 // checking that subsequent font generations never publish an empty logo.
 root.dispatch({elementId:tagged(snapshot,"splash-wordmark")!.id,eventType:"mouseLeave"});
 const settledFrame=[...callbacks];callbacks.clear();for(const callback of settledFrame)callback();
 await Promise.resolve();root.flush();snapshot=host.snapshots.at(-1)!;
 const check=(frame:Snapshot)=>{
  assert.equal(frame.root,shell);assert.equal(tagged(frame,"app-shell")?.id,shell);assert.equal(tagged(frame,"app-wallpaper")?.id,wallpaper);
  assert.equal(tagged(frame,"app-wallpaper")?.props.src,"/fixture/persistent-wallpaper.png");
  const page=tagged(frame,"welcome-page")!,logo=tagged(frame,"splash-wordmark")!,tree=tagged(frame,"zoom-tree-row")!;
  assert.ok(page&&logo&&tree,"Every committed font generation must contain all actual component hosts");
  const menu=children(frame,page).find(node=>node.props.testId==="original-paint-layer")!;
  assert.ok(ops(menu).length>0,"Actual splash menu must be painted in the first transaction without onLayout");
  assert.ok(ops(canvasUnder(frame,logo)).length>0,"Actual wordmark must be painted in the first transaction without onLayout");
  const treeOps=ops(canvasUnder(frame,tree));assert.equal(treeOps.length,1);assert.equal(treeOps[0].k,"i");
  assert.equal(rowRefs.find(row=>row.id===tree.id)?.props.get("data-reveal-start"),revealedAt);
  const motion=typeof tree.props.motion==="string"?JSON.parse(tree.props.motion):tree.props.motion as {initial:unknown;transition:{delay:number;duration:number}};
  assert.equal(motion.initial,false,"Settled tree rows must not replay their original intro");assert.equal(motion.transition.duration,0);assert.equal(motion.transition.delay,0);
  for(const node of frame.nodes.values())if(node.type==="canvas")for(const op of ops(node))if(op.k==="i")assert.ok(frame.images.has(op.id),`Canvas references unavailable GPU texture ${op.id}`);
  return (treeOps[0] as Extract<CanvasOp,{k:"i"}>).id;
 };
 check(snapshot);
 for(const size of [15,16,17,17,16,15,14,14]){
  const previous=host.snapshots.at(-1)!,previousMenu=children(previous,tagged(previous,"welcome-page")!).find(node=>node.props.testId==="original-paint-layer")!;
  const oldMenuImages=new Set(ops(previousMenu).flatMap(op=>op.k==="i"?[op.id]:[])),dropsBefore=host.drops.length;
  const pinnedUploads=()=>host.uploads.filter(id=>id%8===0||id%8===3).length;
  const before=host.snapshots.length,uploads=pinnedUploads(),previousSize=generation().config.fontSize,previousRaster=check(host.snapshots.at(-1)!);
  root.flushSync(()=>setGeneration({config:config(size)}));await Promise.resolve();root.flush();
  const committed=host.snapshots.slice(before);assert.ok(committed.length>0);
  for(const transaction of committed){check(transaction);firstPaints++;}
  assert.ok(host.drops.slice(dropsBefore).every(id=>!oldMenuImages.has(id)),"Old splash menu textures must remain valid until the replacement generation has painted");
  const raster=check(host.snapshots.at(-1)!);
  if(size===previousSize){assert.equal(raster,previousRaster);assert.equal(pinnedUploads(),uploads,"Same-font remount must retain the actual pinned tree/wordmark GPU textures");stableRasterReuse++;}
  assert.equal(visuals.wordmarks.acquire(context,"splash"),nativeWordmark);visuals.wordmarks.release(context);
  const nativeFrame=coreCall<{clickAnimating:boolean}>("wordmark.frame",{id:nativeWordmark,width:360,height:58,autoAnimate:true,reducedMotion:false,fg:base.theme.fg});assert.equal(nativeFrame.clickAnimating,false,"Font zoom must not replay the once-only startup effect");
  assert.equal(visuals.treeReveals("workspace",false,"/project").get("file.ts"),revealedAt);swaps++;
 }
 assert.equal(host.attachments,1);assert.deepEqual(mounted,[14,15,16,17,17,16,15,14,14]);assert.deepEqual(disposed,mounted.slice(0,-1));
 console.log(JSON.stringify({swaps,firstPaints,stableRasterReuse,attachments:host.attachments,persistentWallpaper:true,emptyWarmTransactions:0,startupReplays:0,treeRevealReplays:0,prematureMenuDrops:0,nativeWindowsCreated:0}));
}finally{root.unmount();visuals.dispose();restoreSchedule();assert.equal(callbacks.size,0);assert.equal(host.pixels.size,0);}
