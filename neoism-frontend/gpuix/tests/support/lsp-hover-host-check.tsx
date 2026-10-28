import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot,type NativeRenderer,type EventPayload} from "@gpuix/solid";
import {NativeEditor} from "../../src/native-core";
import {createEditorLsp} from "../../src/editor/lsp";
import {loadConfig} from "../../src/config";
import type {DaemonConnection} from "../../src/daemon/client";
import {setOverlayViewport} from "../../src/ui/overlay-viewport";
const copied:string[]=[];mock.module("../../src/clipboard",()=>({writeClipboard:async(text:string)=>{copied.push(text);}}));
const{LspOverlay}=await import("../../src/editor/LspOverlay");
interface Node{id:number;type:string;children:number[];props:Record<string,unknown>;style:Record<string,unknown>;events:Set<string>}
const nodes=new Map<number,Node>(),images=new Map<number,Buffer>();let focused:number|undefined;
const remove=(id:number,destroyed:number[])=>{const node=nodes.get(id);if(!node)return;for(const child of[...node.children])remove(child,destroyed);for(const parent of nodes.values())parent.children=parent.children.filter(child=>child!==id);nodes.delete(id);destroyed.push(id);};
const renderer={getWindowSize:()=>({width:1400,height:900}),setWindowKeyEvents(){},getFocusedElementId:()=>focused,focusElement(id:number){focused=id;},getSelectedText:()=>"WRONG DOM SELECTION",
 putPaintImage(id:number,width:number,height:number,pixels:Buffer){assert.equal(pixels.length,width*height*4);images.set(id,Buffer.from(pixels));},dropPaintImage(id:number){images.delete(id);},
 applyBatch(json:string){const destroyed:number[]=[];for(const[op,id,a,b]of JSON.parse(json)){
  if(op==="createElement")nodes.set(id,{id,type:a,children:[],props:{},style:{},events:new Set()});
  else if(op==="destroyElement")remove(id,destroyed);
  else if(op==="setStyle")nodes.get(id)!.style=a;
  else if(op==="setCustomProp")nodes.get(id)!.props[a]=b;
  else if(op==="setEventListener"){if(b)nodes.get(id)!.events.add(a);else nodes.get(id)!.events.delete(a);}
  else if(op==="appendChild"||op==="insertBefore"){if(op==="insertBefore"&&a===b)continue;for(const node of nodes.values())node.children=node.children.filter(child=>child!==a);const parent=nodes.get(id)!;if(op==="appendChild")parent.children.push(a);else parent.children.splice(parent.children.indexOf(b),0,a);}
 }return destroyed;}} as unknown as NativeRenderer;
const root=createRoot(renderer),editor=new NativeEditor("main.rs","fn café(参数: usize) -> bool { true }\n"+Array.from({length:100},(_,i)=>`// source line ${i}`).join("\n"),true),requests:unknown[]=[],errors:unknown[]=[];
let requestId=0,restoredFocus=0;
let forwardedEditorWheels=0;
const client={state:{phase:"connected"},subscribe:()=>()=>{},subscribeStatus:(listener:(state:unknown)=>void)=>{listener({phase:"connected"});return()=>{};},send:(_family:unknown,message:unknown)=>{requests.push(message);return ++requestId;}} as unknown as DaemonConnection;
const controller=createEditorLsp(editor,{client,path:"/project/main.rs",root:"/project",openPaths:()=>["/project/main.rs"],openLocation:()=>{},applyEdits:()=>{}},{save:async()=>{},focus:()=>restoredFocus++,report:error=>errors.push(error)});
const area={x:300,y:110,width:640,height:400},config=loadConfig(["--config","/nonexistent/lsp-hover-host"],{});
const byId=(id:string)=>[...nodes.values()].find(node=>node.props.testId===id);
const get=(id:string)=>{const node=byId(id);assert.ok(node,`missing ${id}`);return node;};
const dispatch=(id:number,event:Partial<EventPayload>&{eventType:string})=>root.dispatch({elementId:id,...event});
const settle=async()=>{await Promise.resolve();root.flush();};
try{
 editor.call("layout",{width:area.width,height:area.height,cellWidth:9,rowHeight:21});controller.geometry(area,{width:1400,height:900},1);setOverlayViewport(renderer,area);
 root.render(()=><div testId="hover-editor-parent" onScroll={event=>{if(controller.hoverCaptures(event.x,event.y))return;forwardedEditorWheels++;editor.queueScroll(event.deltaY||0);}}><LspOverlay controller={controller} config={config} anchor={area}/></div>);
 controller.action("hover");const query=requests.flatMap((request:any)=>request.LspQueryAt?[request.LspQueryAt]:[]).find((request:any)=>request.action==="hover")!;
 editor.call("lsp.reply",{message:{LspHoverResult:{seq:query.seq,line:query.line,character:query.character,contents:"```rust\nfn café(参数: usize) -> bool { true }\n```\n\n"+Array.from({length:50},(_,i)=>`Documentation paragraph ${i}`).join("\n\n")}}});await settle();
 const hover=get("lsp-hover"),frame=controller.state()!.hover!;assert.equal(hover.style.left,frame.rect[0]);assert.equal(hover.style.top,frame.rect[1]);assert.ok(Number(hover.style.top)>=area.y);
 const paint=get("lsp-hover-paint");assert.equal(paint.style.left,area.x);assert.equal(paint.style.top,area.y);assert.equal(paint.style.width,area.width);assert.equal(paint.style.height,area.height);
 assert.equal([...nodes.values()].some(node=>node.type==="markdown"),false,"hover must not use the native alternative Markdown widget");
 assert.ok(images.size>0);assert.ok([...images.values()].some(pixels=>pixels.some((value,index)=>index%4===3&&value>0)),"original glyph pixels uploaded to real CanvasImages path");
 const canvas=[...nodes.values()].find(node=>node.type==="canvas")!,ops=JSON.parse(String(canvas.props.commands));assert.ok(ops.some((op:any)=>op.k==="i"));assert.ok(ops.some((op:any)=>op.k==="cl"));
 controller.pointer();dispatch(hover.id,{eventType:"mouseEnter"});await Bun.sleep(220);assert.ok(controller.state()!.hover,"pointer may enter popup without dismissing it");
 const beforeScroll=editor.frame.scrollY,parent=get("hover-editor-parent");
 dispatch(hover.id,{eventType:"scroll",deltaY:-1,precise:false});dispatch(parent.id,{eventType:"scroll",deltaY:-1,precise:false});await settle();
 assert.equal(controller.state()!.hover!.scrollY,34);assert.equal(forwardedEditorWheels,0,"coordinate-less bubbled wheel must stay owned by hover");assert.equal(editor.frame.scrollY,beforeScroll);
 dispatch(hover.id,{eventType:"scroll",deltaY:1,precise:false});dispatch(parent.id,{eventType:"scroll",deltaY:1,precise:false});await settle();assert.equal(controller.state()!.hover!.scrollY,0);assert.equal(forwardedEditorWheels,0);
 const code=frame.commands.find((command:any)=>command.kind==="text"&&command.text==="fn")! as any,x=frame.viewport[0]!,y=frame.paintOrigin[1]!+code.y+2;
 dispatch(hover.id,{eventType:"mouseDown",button:0,x,y});await settle();assert.equal(focused,hover.id);assert.equal(controller.hoverCaptures(10,10),true,"captured selection owns input outside the popup and editor");
 const capture=get("lsp-hover-selection-capture");dispatch(capture.id,{eventType:"mouseMove",pressedButton:0,x:x+frame.viewport[2]!+200,y});dispatch(capture.id,{eventType:"mouseUp",button:0,x:x+frame.viewport[2]!+200,y});await settle();
 assert.equal(byId("lsp-hover-selection-capture"),undefined);assert.equal(controller.state()!.hover!.selectedText,"fn café(参数: usize) -> bool { true }");assert.equal(editor.frame.dirty,false);assert.equal(controller.hoverCaptures(10,10),false);
 const keys=[...nodes.values()].find(node=>node.type==="anchored"&&node.events.has("keyDown"))!;assert.ok(keys);
 dispatch(keys.id,{eventType:"keyDown",key:"c",modifiers:{ctrl:true,shift:false,alt:false,cmd:false},keystrokeId:1});await settle();assert.deepEqual(copied,["fn café(参数: usize) -> bool { true }"]);
 dispatch(keys.id,{eventType:"keyDown",key:"escape",keystrokeId:2});await settle();assert.equal(byId("lsp-hover"),undefined);assert.equal(restoredFocus,1);assert.deepEqual(errors,[]);
 console.log(JSON.stringify({windowsCreated:0,actualSolidAndFfi:true,originalPaintGlyphUpload:true,bodyScopedPaintAndInput:true,nativeMarkdownWidgetAbsent:true,pointerEntryRetainsHover:true,coordinateLessWheelDoesNotScrollEditor:true,selectionCaptureOutsideCard:true,unicodeCanonicalCopy:true,documentUnchanged:true,escapeRestoresEditorFocus:true}));
}finally{root.unmount();controller.dispose();editor.dispose();}
