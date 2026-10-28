import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";
import {paintOps,type PaintCommand,type PreparedCanvas} from "../src/ui/PaintLayer";
import {CanvasImages,rasterKey} from "../src/ui/canvas";
import {loadConfig} from "../src/config";

// The original agent timeline engine (shared Rust pane + recorded renderer),
// driven exactly like AgentTimelineView drives it.
type Command = { kind: string; text?: string; x?: number; y?: number; width?: number; size?: number };
type Frame = { commands: Command[]; animating: boolean; copy?: string | null; link?: string | null };
const history = [
  { id: "u1", role: "User", kind: "User", text: "Show me the code" },
  { id: "a1", role: "Assistant", kind: "Assistant", text: "Here it is:\n\n```ts\nconst answer = 42;\n```\n\nAnd a [link](https://example.com) too." },
  { id: "t1", role: "Assistant", kind: "Tool", tool: "bash", status: "completed", title: "Run tests", detail: "all green" },
];
const engine = () => {
  const { id } = coreCall<{ id: number }>("agenttimeline.new", { sessionId: "s1", theme: "", scale: 1, fontFamily: "Cascadia Code NF", fontWeight: 400 });
  coreCall("agenttimeline.event", { id, message: { HistoryChunk: { session_id: "s1", messages: history, next_cursor: null } } });
  return id;
};
const texts = (frame: Frame) => frame.commands.filter((c) => c.kind === "text");
// Reveal/scroll animations run on wall time: step real frames.
const run = (id: number, ms: number) => { let frame = coreCall<Frame>("agenttimeline.frame", { id }); const end = Date.now() + ms; while (Date.now() < end) { Bun.sleepSync(8); frame = coreCall<Frame>("agenttimeline.frame", { id }); } return frame; };
const settle = (id: number) => { let frame = coreCall<Frame>("agenttimeline.layout", { id, width: 900, height: 700 }); for (let i = 0; i < 300 && frame.animating; i++) { Bun.sleepSync(8); frame = coreCall<Frame>("agenttimeline.frame", { id }); } return frame; };

test("explicit older adapter pages prepend even without a pane loading latch", () => {
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"paging",scale:1});
  const row=(id:string)=>({id,role:"Assistant",kind:"Assistant",text:`pageToken${id}`});
  const event=(historyKind:"head"|"older",ids:string[],cursor:string|null)=>coreCall("agenttimeline.event",{id,historyKind,message:{HistoryChunk:{session_id:"paging",messages:ids.map(row),next_cursor:cursor}}});
  try {
    event("head",["Ccc","Ddd"],"before-c");
    event("older",["Aaa","Bbb"],"before-a");
    const frame=coreCall<Frame>("agenttimeline.layout",{id,width:900,height:2000});
    const body=texts(frame).map(command=>command.text).join(" ");
    for(const token of ["Aaa","Bbb","Ccc","Ddd"])expect(body).toContain(`pageToken${token}`);
    event("head",["Aaa","Bbb","Ccc","Ddd"],"before-a");
    const refresh=coreCall<Frame>("agenttimeline.frame",{id});
    expect(texts(refresh).map(command=>command.text)).toEqual(texts(frame).map(command=>command.text));
  } finally {coreCall("agenttimeline.dispose",{id});}
});

test("refining lazy row heights never reverses visible text during slow scrolling", () => {
  const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"slow-anchor",scale:1.357,fontFamily:"Cascadia Code NF",fontWeight:700});
  const messages=Array.from({length:35},(_,row)=>({id:`row-${row}`,role:"Assistant",kind:"Assistant",text:Array.from({length:35+row%7},(_,word)=>`tokenR${row.toString().padStart(2,"0")}W${word.toString().padStart(2,"0")}`).join(" ")}));
  const positions=(f:Frame)=>new Map(texts(f).filter(c=>c.text?.startsWith("token")&&c.y!>0&&c.y!<590).map(c=>[c.text!,c.y!]));
  try {
    coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"slow-anchor",messages,next_cursor:null}}});
    let frame=coreCall<Frame>("agenttimeline.layout",{id,width:1100,height:900,contentBottom:590});
    let previous=positions(frame), matched=0;
    for(let step=0;step<1200;step++){
      const delta=step<600?3:-3;
      coreCall("agenttimeline.scroll",{id,deltaY:delta,precise:true,deferFrame:true});
      frame=coreCall<Frame>("agenttimeline.frame",{id});const next=positions(frame);
      for(const [text,y] of next){const before=previous.get(text);if(before!==undefined){matched++;expect(y-before).toBeGreaterThanOrEqual(delta>0?-1:-4);expect(y-before).toBeLessThanOrEqual(delta>0?4:1);}}
      previous=next;
    }
    expect(matched).toBeGreaterThan(1000);
  } finally {coreCall("agenttimeline.dispose",{id});}
});

test("native canvas encoding matches the original JavaScript paint translation", () => {
  const id = engine(), config={...loadConfig(["--config","/nonexistent/paint-parity"],{}),fontFamily:"Cascadia Code NF",fontWeight:400};
  try {
    settle(id);
    const frame=coreCall<{commands:PaintCommand[];canvas:PreparedCanvas}>("agenttimeline.frame",{id,canvasPaint:true,includeCommands:true});
    expect(frame.commands.some(command=>command.kind==="text"&&!command.raster&&/^[ \t\r\n]+$/.test(command.text))).toBe(true);
    expect(JSON.parse(frame.canvas.commands).some((op:{k:string;t?:string})=>op.k==="t"&&/^[ \t\r\n]*$/.test(op.t||""))).toBe(false);
    const keys=new Set<number>();
    const images={use:(source:number,raster:{id:number})=>{const key=rasterKey(source,raster.id);keys.add(key);return key;},commit(){}} as unknown as CanvasImages;
    const expected=paintOps(frame.commands,config,images), actual=JSON.parse(frame.canvas.commands);
    const compare=(a:unknown,b:unknown):void=>{
      if(typeof a==="number"&&typeof b==="number"){expect(a).toBeCloseTo(b,3);return;}
      if(Array.isArray(a)&&Array.isArray(b)){expect(a.length).toBe(b.length);a.forEach((value,index)=>compare(value,b[index]));return;}
      if(a&&b&&typeof a==="object"&&typeof b==="object"){
        expect(Object.keys(a).sort()).toEqual(Object.keys(b).sort());
        for(const key of Object.keys(a))compare((a as Record<string,unknown>)[key],(b as Record<string,unknown>)[key]);return;
      }
      expect(a).toEqual(b);
    };
    compare(actual,expected);
    expect(frame.canvas.images.map(image=>rasterKey(image.source,image.raster.id)).sort((a,b)=>a-b)).toEqual([...keys].sort((a,b)=>a-b));
    const compact=coreCall<{commands:PaintCommand[];canvas:PreparedCanvas}>("agenttimeline.frame",{id});
    expect(compact.commands).toEqual([]);expect(compact.canvas.commands.length).toBeGreaterThan(0);
  } finally {coreCall("agenttimeline.dispose",{id});}
});

test("renders the original timeline from daemon history", () => {
  const id = engine(), frame = settle(id);
  const painted = texts(frame).map((c) => c.text).join(" ");
  expect(painted).toContain("Show me the code");
  expect(painted).toContain("answer");
  // Completed tool rows in reopened history stay collapsed, as on desktop.
  coreCall("agenttimeline.dispose", { id });
});

test("direct frame serialization preserves layout text and reports missing engines", () => {
  const id = engine();
  try {
    settle(id);
    const layout = coreCall<Frame>("agenttimeline.layout", { id, width: 900, height: 700 });
    const direct = coreCall<Frame>("agenttimeline.frame", { id });
    const before = texts(layout), after = texts(direct);
    expect(after.map(command => command.text)).toEqual(before.map(command => command.text));
    before.forEach((command, index) => {
      expect(after[index].x!).toBeCloseTo(command.x!, 3);
      expect(after[index].y!).toBeCloseTo(command.y!, 3);
    });
  } finally { coreCall("agenttimeline.dispose", { id }); }
  expect(() => coreCall("agenttimeline.frame", { id })).toThrow("Unknown agent timeline");
});

test("code-card Copy click copies the code, links report their target", () => {
  const id = engine(), frame = settle(id);
  const copy = texts(frame).find((c) => c.text?.trim().toLowerCase() === "copy");
  expect(copy).toBeDefined();
  const click = (x: number, y: number) => { const down = coreCall<Frame>("agenttimeline.pointer", { id, kind: "down", x, y }); coreCall("agenttimeline.pointer", { id, kind: "up", x, y }); return down; };
  const copied = click(copy!.x! + 4, copy!.y! + (copy!.size || 12) / 2);
  expect(copied.copy).toContain("const answer = 42;");
  const link = texts(frame).find((c) => c.text === "link");
  expect(link).toBeDefined();
  expect(click(link!.x! + 4, link!.y! + (link!.size || 14) / 2).link).toBe("https://example.com");
  coreCall("agenttimeline.dispose", { id });
});

test("drag selection copies text on release, like the desktop", () => {
  const id = engine(), frame = settle(id);
  const line = texts(frame).find((c) => c.text === "Here")!;
  const y = line.y! + (line.size || 14) / 2;
  coreCall("agenttimeline.pointer", { id, kind: "down", x: line.x! + 1, y });
  coreCall("agenttimeline.pointer", { id, kind: "move", x: line.x! + 200, y, deferFrame: true });
  const up = coreCall<Frame>("agenttimeline.pointer", { id, kind: "up", x: line.x! + 200, y });
  expect(up.copy).toContain("Here it is");
  expect(coreCall<{ text: string | null }>("agenttimeline.copy", { id }).text).toBe(up.copy ?? null);
  coreCall("agenttimeline.dispose", { id });
});

test("streaming updates and scroll go through the shared pane", () => {
  const id = engine(); settle(id);
  coreCall("agenttimeline.event", { id, message: { MessageUpdated: { session_id: "s1", message: { id: "a2", role: "Assistant", kind: "Assistant", text: "Streaming tail token" } } } });
  let frame = run(id, 900);
  expect(texts(frame).map((c) => c.text).join("")).toContain("Streaming tail token");
  // Another conversation's events never reach this pane.
  coreCall("agenttimeline.event", { id, message: { MessageUpdated: { session_id: "other", message: { id: "x", role: "Assistant", kind: "Assistant", text: "Foreign" } } } });
  coreCall("agenttimeline.event", { id, message: { SessionIdle: { session_id: "s1" } } });
  frame = run(id, 300);
  expect(texts(frame).map((c) => c.text).join("")).not.toContain("Foreign");
  const before = JSON.stringify(frame.commands.map((c) => c.y));
  coreCall("agenttimeline.scroll", { id, deltaY: 120, precise: true });
  expect(JSON.stringify(run(id, 400).commands.map((c) => c.y))).not.toBe(before);
  coreCall("agenttimeline.dispose", { id });
}, 20000);

test("deferred wheel input paints once and older history preserves visible text position", () => {
 const {id}=coreCall<{id:number}>("agenttimeline.new",{sessionId:"paged",scale:1,fontFamily:"Cascadia Code NF",fontWeight:400});
 const rows=(start:number,end:number)=>Array.from({length:end-start},(_,i)=>({id:`page-${start+i}`,role:"User",kind:"User",text:`Stable pagination marker ${start+i}`}));
 try {
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"paged",messages:rows(40,80),next_cursor:"older"}}});
  coreCall("agenttimeline.layout",{id,width:900,height:700});
  expect(coreCall("agenttimeline.scroll",{id,deltaY:1e7,precise:true,deferFrame:true})).toBeNull();
  const before=coreCall<Frame & {send:unknown[]}>("agenttimeline.frame",{id});
  expect(JSON.stringify(before.send)).toContain("GetHistory");
  const anchor=texts(before).find(c=>c.text?.includes("Stable pagination marker")&&c.y!>30);
  expect(anchor).toBeDefined();
  coreCall("agenttimeline.event",{id,message:{HistoryChunk:{session_id:"paged",messages:rows(0,40),next_cursor:null}}});
  const after=coreCall<Frame>("agenttimeline.frame",{id});
  const retained=texts(after).find(c=>c.text===anchor!.text);
  expect(retained).toBeDefined();
  expect(retained!.y!).toBeCloseTo(anchor!.y!,1);
 } finally {coreCall("agenttimeline.dispose",{id});}
});

test("compact paint retains raster identity, geometry and clipping with less transport",()=>{
 const id=engine();try{
  settle(id);
  const full=coreCall<{commands:any[]}>("agenttimeline.frame",{id,compactPaint:false});
  const compact=coreCall<{commands:any[]}>("agenttimeline.frame",{id,compactPaint:true});
  expect(compact.commands.length).toBe(full.commands.length);
  let rasters=0;
  full.commands.forEach((before,i)=>{const after=compact.commands[i];if(before.kind!=="text"||!before.raster)return;
   rasters++;const r=before.raster,scale=r.scale||1;
   expect(after.kind).toBe("raster-text");expect(after.raster.id).toBe(r.id);expect(after.clip).toEqual(before.clip);expect(after.layer).toBe(before.layer);
   [before.x+r.dx,before.y+r.dy,r.width/scale,r.height/scale].forEach((value,j)=>expect(after.rect[j]).toBeCloseTo(value,3));
  });
  expect(rasters).toBeGreaterThan(0);expect(JSON.stringify(compact).length).toBeLessThan(JSON.stringify(full).length);
 }finally{coreCall("agenttimeline.dispose",{id});}
});
