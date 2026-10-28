import assert from "node:assert/strict";
import {createRoot,type HostElement,type NativeRenderer} from "@gpuix/solid";
import {ComposerCaret} from "../../src/agent/ComposerCaret";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {surfaceCaret,suppressSurfaceCursor} from "../../src/ui/cursor-handoff";
interface Node{id:number;type:string;children:number[];props:Record<string,unknown>;style:Record<string,unknown>}
const nodes=new Map<number,Node>(),events=new Set<string>();
const remove=(id:number,removed:number[])=>{for(const child of [...(nodes.get(id)?.children||[])])remove(child,removed);nodes.delete(id);for(const event of events)if(event.startsWith(`${id}:`))events.delete(event);removed.push(id);};
const renderer={setWindowKeyEvents(){},applyBatch(json:string){const removed:number[]=[];for(const[op,id,a,b]of JSON.parse(json)){
 if(op==="createElement")nodes.set(id,{id,type:a,children:[],props:{},style:{}});
 else if(op==="destroyElement")remove(id,removed);
 else if(op==="setStyle")nodes.get(id)!.style=a;
 else if(op==="setCustomProp")nodes.get(id)!.props[a]=b;
 else if(op==="setEventListener"){if(b)events.add(`${id}:${a}`);else events.delete(`${id}:${a}`);}
 else if(op==="appendChild"||op==="insertBefore"){for(const node of nodes.values())node.children=node.children.filter(child=>child!==a);const parent=nodes.get(id)!;if(op==="appendChild")parent.children.push(a);else parent.children.splice(parent.children.indexOf(b),0,a);}
 }return removed;}}as unknown as NativeRenderer;
const root=createRoot(renderer),config={...loadConfig(["--config","/nonexistent/composer-caret"],{}),cursorBlink:true},callbacks=new Map<number,()=>void>();let next=0,input:HostElement|undefined;
const reset=usePresentationScheduler(callback=>{const id=++next;callbacks.set(id,callback);return()=>{callbacks.delete(id);};});
const flush=()=>{const pending=[...callbacks.values()];callbacks.clear();for(const callback of pending)callback();root.flush();};
const byTest=(id:string)=>[...nodes.values()].find(node=>node.props.testId===id);
const quad=()=>JSON.parse(String(byTest("agent-composer-cursor-paint")!.props.commands))[0].p[0] as number[];
const center=()=>{const points=quad();return[points.filter((_,i)=>i%2===0).reduce((sum,n)=>sum+n,0)/4,points.filter((_,i)=>i%2===1).reduce((sum,n)=>sum+n,0)/4];};
const report=(x:number,y:number,visible=true,offset=0,remainingWidth=100)=>root.dispatch({elementId:input!.id,eventType:"caretGeometry",x,y,width:2,height:22,value:JSON.stringify({visible,fontSize:16,remainingWidth,offset,selectionStart:offset,selectionEnd:offset})});
let stage="mount";
try{
 root.render(()=><div><textarea ref={input} testId="actual-composer-input" theme={{caret:"transparent"}} value="界🙂é\nsecond line"/><ComposerCaret config={config} input={()=>input}/></div>);
 assert.ok(input?.id);assert.ok(events.has(`${input.id}:caretGeometry`));assert.ok(!byTest("agent-composer-cursor-paint"));
 stage="first event";report(120,240,true,0);assert.equal(callbacks.size,1);flush();
 const first=byTest("agent-composer-cursor")!;assert.ok(first);assert.equal(JSON.parse(String(byTest("agent-composer-cursor-paint")!.props.commands))[0].c&255,255,"Agent block must preserve original opaque cursor ink");assert.ok(Math.abs(Number(first.style.width)-9.6)<.001);assert.equal(Number(first.style.height),20);assert.ok(Math.abs(center()[0]-124.8)<.001);assert.equal(center()[1],250);
 stage="idle blink";await Bun.sleep(550);root.flush();assert.ok(!byTest("agent-composer-cursor-paint"),"Agent must hide after the original530ms visible half-period without input or native animation frames");await Bun.sleep(530);root.flush();assert.ok(byTest("agent-composer-cursor-paint"),"Agent must resume its next visible half-period while idle");config.cursorBlink=false;
 stage="motion";const oldCenter=center();await Bun.sleep(85);report(160,262,true,3);report(220,284,true,11);assert.equal(callbacks.size,1,"Native repeat events must share one display callback");flush();
 assert.ok(Math.abs(center()[0]-oldCenter[0])<.001,"First motion after idle starts at the painted cursor");
 await Bun.sleep(6);flush();assert.ok(center()[0]>oldCenter[0]);assert.ok(center()[0]<224.8);
 stage="settling";const limit=performance.now()+1000;while(callbacks.size){await Bun.sleep(6);flush();assert.ok(performance.now()<limit,"Canonical trail must settle");}
 assert.ok(Math.abs(center()[0]-224.8)<.1);assert.ok(Math.abs(center()[1]-294)<.1);
 stage="handoff";const handoff=surfaceCaret(renderer,input.id);assert.ok(handoff);assert.ok(Math.abs(handoff.bounds.x-220)<.1);
 root.flushSync(()=>suppressSurfaceCursor(renderer,true));assert.ok(!byTest("agent-composer-cursor-paint"));
 root.flushSync(()=>suppressSurfaceCursor(renderer,false));assert.ok(byTest("agent-composer-cursor-paint"));
 stage="hidden";report(220,284,false,11);assert.ok(!byTest("agent-composer-cursor-paint"));assert.equal(callbacks.size,0);
 stage="clipped";report(419,284,true,11,1);flush();const clipLimit=performance.now()+1000;while(callbacks.size){await Bun.sleep(6);flush();assert.ok(performance.now()<clipLimit,"Clipped cursor must settle");}const clipped=byTest("agent-composer-cursor")!;assert.ok(Number(clipped.style.width)<=1.001);
 console.log(JSON.stringify({nativeEventRegistered:true,sourceBlockWidth:9.6,sourceBlockHeight:20,burstFrames:1,visibleSpring:true,unicodeOffset:11,windowHandoff:true,selectionAndBlurHidden:true,cursorAlpha:255,idleBlinkRestored:true,nativeWindowsCreated:0}));
}catch(error){console.error(JSON.stringify({stage,inputId:input?.id,inputMounted:input?.mounted,handoff:surfaceCaret(renderer,input?.id)?.bounds,cursor:byTest("agent-composer-cursor"),quad:byTest("agent-composer-cursor-paint")?quad():null,callbacks:callbacks.size}));throw error;}finally{root.unmount();reset();assert.equal(callbacks.size,0);assert.equal(surfaceCaret(renderer,input?.id),undefined);assert.ok(!events.has(`${input?.id}:caretGeometry`));}
