import { expect, test } from "bun:test";
import {homedir} from "node:os";
import { coreCall } from "../src/native-core";
import {paintOps,type PaintCommand,type PreparedCanvas} from "../src/ui/PaintLayer";
import {rasterKey,type CanvasImages} from "../src/ui/canvas";
import {loadConfig} from "../src/config";

const thread = (id: string, directory: string, updated: number) => ({ session_id: id, title: id, directory, updated_at: updated });
const panel = (directory: string) => coreCall<{ id: number }>("agentsidepanel.new", { directory, theme: "pastel_dark", scale: 1, fontFamily: "Cascadia Code NF", fontWeight: 400 }).id;
const frame = (id: number) => coreCall<{ send: Record<string, any>[] }>("agentsidepanel.frame", { id, width: 300, height: 600, viewedSession: null });
const debug = (id: number) => coreCall<{ sessions: number; scrollTop: number; nextCursor: string | null }>("agentsidepanel.debug", { id });

test("a settled catalog does not animate the pane's invisible home wordmark", () => {
  const id = panel("/proj/app");
  try {
    const loading = coreCall<{ animating: boolean }>("agentsidepanel.frame", { id, width: 300, height: 600 });
    expect(loading.animating).toBe(true);
    coreCall("agentsidepanel.event", { id, message: { ThreadList: { threads: [thread("a", "/proj/app", 2)], requested_cursor: null, next_cursor: null } } });
    const settled = coreCall<{ animating: boolean }>("agentsidepanel.frame", { id, width: 300, height: 600 });
    expect(settled.animating).toBe(false);
  } finally { coreCall("agentsidepanel.dispose", { id }); }
});

test("a declared-workspace sidebar ignores another consumer's ThreadList and keeps its own reply", () => {
  const id = panel("/proj/app");
  try {
    expect(frame(id).send).toEqual([{ ListThreads: { directory: "/proj/app", limit: 50, cursor: null } }]);
    // The model's startup listing for a different workspace arrives first.
    coreCall("agentsidepanel.event", { id, message: { ThreadList: { threads: [thread("other", "/proj/other", 9)], requested_cursor: null, next_cursor: null } } });
    expect(debug(id).sessions).toBe(0);
    coreCall("agentsidepanel.event", { id, message: { ThreadList: { threads: [thread("a", "/proj/app", 2), thread("b", "/proj/app", 1)], requested_cursor: null, next_cursor: "c1" } } });
    expect(debug(id)).toMatchObject({ sessions: 2, nextCursor: "c1" });
  } finally { coreCall("agentsidepanel.dispose", { id }); }
});

test("the filesystem root and home directory are the all-workspace dashboard", () => {
  for (const directory of ["/", homedir()]) {
    const id = panel(directory);
    try { expect(Object.keys(frame(id).send[0] || {})).toEqual(["ListSessionProjects"]); }
    finally { coreCall("agentsidepanel.dispose", { id }); }
  }
});

test("an unknown workspace directory waits instead of showing the dashboard, then scopes when known", () => {
  const id = panel("");
  try {
    expect(frame(id).send).toEqual([]);
    const scoped = coreCall<{ send: Record<string, any>[] }>("agentsidepanel.scope", { id, directory: "/proj/app" });
    expect(scoped.send).toEqual([{ ListThreads: { directory: "/proj/app", limit: 50, cursor: null } }]);
  } finally { coreCall("agentsidepanel.dispose", { id }); }
});

test("prepared sidebar paint preserves geometry, clipping, glyph images and catalog requests",()=>{
 const id=panel("/proj/app"),config={...loadConfig([],{}),fontFamily:"Cascadia Code NF",fontWeight:400};
 try{
  const first=coreCall<{send:unknown[]}>("agentsidepanel.frame",{id,canvasPaint:true});
  expect(first.send).toEqual([{ListThreads:{directory:"/proj/app",limit:50,cursor:null}}]);
  coreCall("agentsidepanel.event",{id,message:{ThreadList:{threads:Array.from({length:30},(_,i)=>thread(`Conversation with a long clipped title ${i}`,"/proj/app",30-i)),requested_cursor:null,next_cursor:null}}});
  const frame=coreCall<{commands:PaintCommand[];canvas:PreparedCanvas;send:unknown[]}>("agentsidepanel.frame",{id,width:270,height:420,canvasPaint:true,includeCommands:true});
  const keys=new Set<number>();
  const images={use:(source:number,raster:{id:number})=>{const key=rasterKey(source,raster.id);keys.add(key);return key;},commit(){}} as unknown as CanvasImages;
  const expected=paintOps(frame.commands,config,images),actual=JSON.parse(frame.canvas.commands);
  const compare=(a:unknown,b:unknown):void=>{
   if(typeof a==="number"&&typeof b==="number"){expect(a).toBeCloseTo(b,3);return;}
   if(Array.isArray(a)&&Array.isArray(b)){expect(a.length).toBe(b.length);a.forEach((value,index)=>compare(value,b[index]));return;}
   if(a&&b&&typeof a==="object"&&typeof b==="object"){expect(Object.keys(a).sort()).toEqual(Object.keys(b).sort());for(const key of Object.keys(a))compare((a as Record<string,unknown>)[key],(b as Record<string,unknown>)[key]);return;}
   expect(a).toEqual(b);
  };
  expect(expected.length).toBeGreaterThan(10);compare(actual,expected);
  expect(new Set(frame.canvas.images.map(image=>rasterKey(image.source,image.raster.id)))).toEqual(keys);
  expect(frame.send).toEqual([]);
  const compact=coreCall<{commands:unknown[];canvas:PreparedCanvas}>("agentsidepanel.frame",{id,canvasPaint:true});
  expect(compact.commands).toEqual([]);expect(compact.canvas.images.length).toBeGreaterThan(0);
 }finally{coreCall("agentsidepanel.dispose",{id});}
});

test("deferred sidebar pointer moves do not build discarded frames or consume outbound requests",()=>{
 const id=panel("/proj/app");
 try{
  for(let x=0;x<50;x++)expect(coreCall("agentsidepanel.pointer",{id,kind:"move",x,y:100,deferFrame:true})).toBeNull();
  expect(frame(id).send).toEqual([{ListThreads:{directory:"/proj/app",limit:50,cursor:null}}]);
 }finally{coreCall("agentsidepanel.dispose",{id});}
});

test("streamed tool-input JSON does not become a sidebar transcript or request a sidebar repaint",()=>{
 const id=panel("/proj/app");
 try{
  expect(coreCall<{needsPaint:boolean}>("agentsidepanel.event",{id,message:{SessionEvent:{session_id:"chat",kind:"message.part.delta",properties:{messageID:"message",partID:"part",field:"raw",partType:"tool",delta:"large patch input".repeat(1000)}}}})).toEqual({needsPaint:false});
 }finally{coreCall("agentsidepanel.dispose",{id});}
});
