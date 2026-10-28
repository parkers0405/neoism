import { expect,test } from "bun:test";
import { toArrayBuffer,type Pointer } from "bun:ffi";
import { loadConfig } from "../src/config";
import { NativeEditor } from "../src/native-core";
import { NativeMarkdown } from "../src/markdown/NativeMarkdown";
import type { PaintCommand } from "../src/ui/PaintLayer";
const config={...loadConfig(["--config","/nonexistent/markdown-test-config"],{}),vim:true};
const source="# Rich document\n\n- [ ] Task body\n\n| Name | Value |\n| --- | --- |\n| café | **bold** |\n\n> [!NOTE] Preserve formatting\n\n```ts\nconst answer = 42;\n```\n\n[Neoism](README.md)\n";
const texts=(commands:PaintCommand[])=>commands.filter((c):c is Extract<PaintCommand,{kind:"text"}>=>c.kind==="text");
test("original rich renderer emits native geometry and cached original glyph pixels",()=>{
 const editor=new NativeEditor("README.md",source,true),markdown=new NativeMarkdown(editor,config,"/project/README.md");
 try{
  const frame=markdown.call("layout",{width:900,height:700});
  const runs=texts(frame.commands);
  expect(runs.map(run=>run.text).join(" ")).toContain("Rich document");
  expect(runs.map(run=>run.text).join(" ")).toContain("Task body");
  expect(runs.some(run=>run.text.includes("Name"))).toBe(true);
  const raster=runs.find(run=>run.raster)?.raster!;expect(raster).toBeDefined();
  const bytes=new Uint8Array(toArrayBuffer(raster.ptr as unknown as Pointer,0,raster.len));
  const alpha=Array.from({length:bytes.length/4},(_,i)=>bytes[i*4+3]);
  expect(alpha.some(a=>a>0&&a<255)).toBe(true);expect(alpha.some(a=>a===0)).toBe(true);
  const first=runs.find(run=>run.text==="Task")!;
  const again=texts(markdown.call("frame").commands).find(run=>run.text===first.text)!;
  expect(again.raster?.id).toBe(first.raster?.id);
 }finally{markdown.dispose();editor.dispose();}
});
test("rich tasks, Vim editing, undo, canonical save state and snapshots use the real document",()=>{
 const editor=new NativeEditor("README.md",source,true),markdown=new NativeMarkdown(editor,config,"/project/README.md");
 try{
  markdown.call("goto",{line:2,col:0});markdown.call("key",{key:"enter"});
  expect(editor.text()).toContain("- [x] Task body");expect(editor.frame.dirty).toBe(true);
  markdown.call("key",{key:"u",char:"u"});expect(editor.text()).toContain("- [ ] Task body");
  markdown.call("goto",{line:0,col:0});markdown.call("key",{key:"i",char:"i"});markdown.call("key",{key:"x",char:"X"});markdown.call("key",{key:"escape"});
  expect(editor.text()).toStartWith("# XRich document");expect(markdown.text()).toBe(editor.text().replace(/\r\n/g,"\n").replace(/\n$/,""));
  const snapshot=markdown.snapshot();markdown.call("goto",{line:5,col:0});markdown.restore(snapshot);
  expect(markdown.frame.cursor.line).toBe(snapshot.line);expect(markdown.frame.mode).toBe(snapshot.mode as typeof markdown.frame.mode);
  editor.call("saved",{revision:editor.frame.revision});expect(editor.frame.dirty).toBe(false);
  editor.call("replace-source",{text:"# External update\n\nNew content"});expect(markdown.text()).toBe(editor.text().replace(/\r\n/g,"\n").replace(/\n$/,""));
 }finally{markdown.dispose();editor.dispose();}
});

test("original Mermaid scene and daemon-supplied ink assets produce real diagram primitives",async()=>{
 const editor=new NativeEditor("README.md","# Diagrams\n\n```mermaid\ngraph TD\n A[Start] --> B[Done]\n```\n\n```draw\n./scene.neodraw\n```",true);
 const requested:string[]=[],scene=JSON.stringify({version:1,shapes:[{id:1,type:"rect",x:0,y:0,w:100,h:60,style:{fill:"#ff0000",stroke:"#ff0000",roughness:0,width:1}}]});
 const markdown=new NativeMarkdown(editor,config,"/project/README.md",async path=>{requested.push(path);if(path.endsWith("/.README.ink.neodraw"))return new TextEncoder().encode(scene);throw new Error("Missing fixture asset");});
 try{
  markdown.call("layout",{width:900,height:1000});
  const initial=texts(markdown.frame.commands).map(run=>run.text);
  expect(initial).toContain("Start");expect(initial).toContain("Done");
  const end=Date.now()+2000;while(!markdown.frame.commands.some(command=>command.kind!=="text"&&command.kind!=="image"&&command.color[0]===1&&command.color[1]===0)){if(Date.now()>end)throw new Error("Original ink layer did not load through its host service");await Bun.sleep(5);}
  expect(requested.some(path=>path.endsWith("/.README.ink.neodraw"))).toBe(true);
  expect(markdown.frame.commands.filter(command=>command.kind==="polygon").length).toBeGreaterThan(10);
  expect(editor.frame.dirty).toBe(false);
 }finally{markdown.dispose();editor.dispose();}
});

test("original compact context menu filters slash blocks and applies its actual selected template",()=>{
 const editor=new NativeEditor("blank.md","",true),markdown=new NativeMarkdown(editor,config,"/project/blank.md");
 try{
  markdown.call("layout",{width:900,height:700});markdown.call("key",{key:"i",char:"i"});
  const opened=markdown.call("key",{key:"/",char:"/"});expect(opened.menu).toBeDefined();
  expect(texts(opened.menu!.commands).map(run=>run.text).join(" ")).toContain("Add block");
  for(let i=0;i<4;i++)markdown.call("key",{key:"down"});
  markdown.call("key",{key:"enter"});expect(editor.text()).toStartWith("# ");expect(markdown.frame.menu).toBeNull();
 }finally{markdown.dispose();editor.dispose();}
});

test("cold rich restore uses the actual viewport and original virtual scroll anchor",async()=>{
 const longSource="# Recovered document\n\n"+Array.from({length:80},(_,i)=>`Paragraph ${i+1} with **rich text** and a [local link](README.md).`).join("\n\n");
 const editor=new NativeEditor("README.md",longSource,true),markdown=new NativeMarkdown(editor,config,"/project/README.md");
 let restoredEditor:NativeEditor|undefined,restored:NativeMarkdown|undefined;
 try{
  markdown.call("layout",{width:451.8,height:801.6});markdown.call("goto",{line:160,col:2});
  for(let i=0;i<150;i++){await Bun.sleep(8);markdown.call("frame");if(!markdown.frame.animating)break;}
  expect(markdown.frame.cursor.rect).not.toBeNull();const snapshot=markdown.snapshot();expect(snapshot.anchor).toBeDefined();
  restoredEditor=new NativeEditor("README.md",editor.text(),true);restoredEditor.restore(editor.snapshot());
  restored=new NativeMarkdown(restoredEditor,config,"/project/README.md");restored.restore(snapshot,true);
  expect(restored.snapshot().scrollY).toBe(snapshot.scrollY);
  restored.call("layout",{width:451.8,height:801.6});
  for(let i=0;i<150;i++){await Bun.sleep(8);restored.call("frame");if(!restored.frame.animating)break;}
  expect(restored.frame.cursor.line).toBe(snapshot.line);expect(restored.frame.scrollY).toBeGreaterThan(1000);
  expect(restored.frame.cursor.rect).not.toBeNull();expect(Math.abs(restored.frame.cursor.rect![1]-markdown.frame.cursor.rect![1])).toBeLessThan(40);
 }finally{restored?.dispose();restoredEditor?.dispose();markdown.dispose();editor.dispose();}
},10000);
