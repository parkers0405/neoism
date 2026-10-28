import {expect,test} from "bun:test";
import {toArrayBuffer,type Pointer} from "bun:ffi";
import {NativeEditor} from "../src/native-core";
import type {LspHover} from "../src/editor/lsp";
import type {PaintCommand} from "../src/ui/PaintLayer";
const area={x:300,y:90,width:640,height:400};
const textCommands=(h:LspHover)=>h.commands.filter((command):command is Extract<PaintCommand,{kind:"text"}>=>command.kind==="text");
function fixture(contents:string,options:{width?:number;height?:number;scale?:number}={}){
 const editor=new NativeEditor("hover.rs","fn café(参数: usize) -> bool { true }",true);
 const viewport={...area,width:options.width??area.width,height:options.height??area.height};
 editor.call("layout",{width:viewport.width,height:viewport.height,cellWidth:9,rowHeight:21});
 editor.call("lsp.enable",{surface:"hover-test",path:"/project/hover.rs",openPaths:["/project/hover.rs"]});
 editor.call("lsp.geometry",{...viewport,windowWidth:1400,windowHeight:900,scale:options.scale??1});
 editor.call("lsp.hover-configure",{fontFamily:"Cascadia Code NF",fontWeight:400,theme:"pastel_dark",viewport:[viewport.x,viewport.y,viewport.width,viewport.height]});
 const open=()=>{
  const request=editor.call("lsp.action",{action:"hover"}).lsp!.requests.flatMap(request=>typeof request==="object"&&"LspQueryAt"in request?[request.LspQueryAt]:[]).find(request=>request.action==="hover")!;
  return editor.call("lsp.reply",{message:{LspHoverResult:{seq:request.seq,line:request.line,character:request.character,contents}}}).lsp!.hover!;
 };
 return{editor,viewport,open};
}
test("hover paints original measured prose/code/inline chips and exact source glyph rasters inside the pane body",()=>{
 const source="### Documentation\n\nProportional **bold** prose and `inline`.\n\n```rust\nfn café(参数: usize) -> bool { true }\n```\n\n[Guide](https://example.com/docs)";
 const {editor,viewport,open}=fixture(source);
 try{
  const h=open(),texts=textCommands(h);expect(h.commands.some(command=>command.kind==="rect"&&command.radii[0]===8)).toBe(true);
  expect(texts.some(command=>command.text.includes("```"))).toBe(false);expect(texts.some(command=>command.text==="fn")).toBe(true);expect(texts.some(command=>command.bold&&command.text.includes("Documentation"))).toBe(true);
  expect(texts.some(command=>command.family?.includes("Geist"))).toBe(true);expect(texts.some(command=>command.family!==texts.find(text=>text.text.includes("Proportional"))?.family)).toBe(true);
  expect(new Set(texts.map(command=>command.color.join(","))).size).toBeGreaterThan(3);expect(h.commands.some(command=>command.kind==="rect"&&command.radii[0]===4)).toBe(true);
  expect(h.rect[0]).toBeGreaterThanOrEqual(viewport.x);expect(h.rect[1]).toBeGreaterThanOrEqual(viewport.y);expect(h.rect[0]!+h.rect[2]!).toBeLessThanOrEqual(viewport.x+viewport.width+.01);expect(h.rect[1]!+h.rect[3]!).toBeLessThanOrEqual(viewport.y+viewport.height+.01);
  expect(h.paintOrigin).toEqual([viewport.x,viewport.y]);expect(h.paintSize).toEqual([viewport.width,viewport.height]);
  const raster=texts.find(command=>command.text.includes("Documentation"))!.raster!;expect(raster).toBeDefined();expect(raster.scale).toBe(2);expect(raster.len).toBe(raster.width*raster.height*4);
  const pixels=Buffer.from(new Uint8Array(toArrayBuffer(raster.ptr as Pointer,0,raster.len)));expect(pixels.some((value,index)=>index%4===3&&value>0)).toBe(true);
 }finally{editor.dispose();}
});
test("canonical hover selection captures outside the card and copies Unicode source without Markdown delimiters",()=>{
 const {editor,open}=fixture("```rust\nfn café(参数: usize) -> bool { true }\n```");
 try{
  const h=open(),code=textCommands(h).find(command=>command.text==="fn")!,y=h.paintOrigin[1]!+code.y+2;
  const down=editor.call("lsp.hover-input",{phase:"down",x:h.viewport[0],y}).lsp!.hover!;expect(down.dragging).toBe(true);
  const dragged=editor.call("lsp.hover-input",{phase:"move",x:h.viewport[0]!+h.viewport[2]!+200,y}).lsp!.hover!;expect(dragged.dragging).toBe(true);
  const up=editor.call("lsp.hover-input",{phase:"up",x:h.viewport[0]!+h.viewport[2]!+200,y}).lsp!.hover!;
  expect(up.dragging).toBe(false);expect(up.selectedText).toBe("fn café(参数: usize) -> bool { true }");expect(up.commands.some(command=>command.kind==="rect"&&command.order===42)).toBe(true);
  expect(editor.text()).toBe("fn café(参数: usize) -> bool { true }");expect(editor.frame.dirty).toBe(false);
 }finally{editor.dispose();}
});
test("original hover scroll is clipped, exact, bounded and never turns a wheel gesture into a link click",()=>{
 const {editor,open}=fixture("[Guide](https://example.com/docs)\n\n"+Array.from({length:100},(_,i)=>`Paragraph ${i} with documentation text.`).join("\n\n"));
 try{
  const h=open();expect(h.maxScroll).toBeGreaterThan(100);const guide=textCommands(h).find(command=>command.text==="Guide")!,x=h.paintOrigin[0]!+guide.x+guide.width/2,y=h.paintOrigin[1]!+guide.y+3;
  editor.call("lsp.hover-input",{phase:"down",x,y});expect(editor.call("lsp.hover-input",{phase:"wheel",x,y,delta:34}).lsp!.hover!.scrollY).toBe(34);
  expect(editor.call("lsp.hover-input",{phase:"wheel",x,y,delta:-34}).lsp!.hover!.scrollY).toBe(0);
  expect(editor.call("lsp.hover-input",{phase:"up",x,y}).lsp!.events.some(event=>event.kind==="open-url")).toBe(false);
  const bottom=editor.call("lsp.hover-input",{phase:"wheel",x,y,delta:1e6}).lsp!.hover!;expect(bottom.scrollY).toBe(bottom.maxScroll);expect(textCommands(bottom).every(command=>command.clip&&command.clip[1]!>=0)).toBe(true);
 }finally{editor.dispose();}
});
test("safe link activation is emitted once by canonical hit testing; unsafe links never activate",()=>{
 const {editor,open}=fixture("[Guide](https://example.com/docs) [Unsafe](javascript:alert(1))");
 try{
  let h=open();for(const label of["Guide","Unsafe"]){const text=textCommands(h).find(command=>command.text===label)!,x=h.paintOrigin[0]!+text.x+text.width/2,y=h.paintOrigin[1]!+text.y+3;
   editor.call("lsp.hover-input",{phase:"down",x,y});const up=editor.call("lsp.hover-input",{phase:"up",x,y});expect(up.lsp!.events.filter(event=>event.kind==="open-url")).toEqual(label==="Guide"?[{kind:"open-url",url:"https://example.com/docs"}]:[]);h=up.lsp!.hover!;
  }
  expect(editor.call("frame").lsp!.events).toEqual([]);
 }finally{editor.dispose();}
});
test("theme, body size and scale invalidate hover paint without stale placement or selection",()=>{
 const {editor,open}=fixture("Documentation with `inline` and **bold**.\n\n```rust\nfn value() -> bool { true }\n```");
 try{
  const first=open(),before=textCommands(first).map(command=>command.color.join(","));
  editor.call("lsp.geometry",{...area,windowWidth:1400,windowHeight:900,scale:1.5});
  const h=editor.call("lsp.hover-configure",{fontFamily:"Geist Mono",fontWeight:700,theme:"ayu_light",viewport:[600,210,260,190]}).lsp!.hover!;
  expect(h.paintOrigin).toEqual([600,210]);expect(h.rect[0]).toBeGreaterThanOrEqual(600);expect(h.rect[1]).toBeGreaterThanOrEqual(210);expect(h.rect[0]!+h.rect[2]!).toBeLessThanOrEqual(860.01);expect(h.rect[1]!+h.rect[3]!).toBeLessThanOrEqual(400.01);
  expect(textCommands(h).some(command=>command.size===19.5)).toBe(true);expect(textCommands(h).map(command=>command.color.join(","))).not.toEqual(before);
  expect(h.commands.some(command=>command.kind==="rect"&&command.radii[0]===12)).toBe(true);
 }finally{editor.dispose();}
});
test("a mouse hover survives the original source-to-card corridor and entering the card at nonzero pane origins",async()=>{
 const {editor}=fixture("unused");
 try{
  const localX=editor.frame.textX+2;editor.call("lsp.pointer",{x:localX,y:10});await Bun.sleep(430);
  const request=editor.call("frame").lsp!.requests.flatMap(request=>typeof request==="object"&&"LspQueryAt"in request?[request.LspQueryAt]:[]).find(request=>request.action==="hover")!;expect(request).toBeDefined();
  const h=editor.call("lsp.reply",{message:{LspHoverResult:{seq:request.seq,line:request.line,character:request.character,contents:"Mouse documentation"}}}).lsp!.hover!;
  const corridorY=h.rect[1]!-1.5;expect(editor.call("lsp.pointer",{x:localX,y:corridorY-area.y}).lsp!.hover).not.toBeNull();
  expect(editor.call("lsp.pointer",{x:h.rect[0]!-area.x+15,y:h.rect[1]!-area.y+15}).lsp!.hover).not.toBeNull();
  expect(editor.call("lsp.pointer",{x:1000,y:700}).lsp!.hover).toBeNull();
 }finally{editor.dispose();}
});
