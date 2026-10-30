import {expect,test} from "bun:test";
import {NativeEditor} from "../src/native-core";
import {NativeMarkdown} from "../src/markdown/NativeMarkdown";
import {loadConfig} from "../src/config";
import {paintOps} from "../src/ui/PaintLayer";
import {CanvasImages} from "../src/ui/canvas";
test("native Markdown canvas retains original paint geometry and image identities",()=>{
 const config=loadConfig(["--config","/nonexistent/markdown-canvas"],{}),editor=new NativeEditor("ink.md","# Heading\n\n**Bold**, `code`, a link [here](https://example.test).\n\n> quoted\n\n- one\n- two",true);
 const markdown=new NativeMarkdown(editor,config,"/ink.md"),images=new CanvasImages({putPaintImage(){},dropPaintImage(){}});
 try{
  const raw=markdown.call("layout",{width:800,height:600});
  const expected=paintOps(raw.commands,config,images);
  const prepared=markdown.call("canvas-paint",{fontFamily:config.fontFamily,fontWeight:config.fontWeight},false);
  expect(prepared.commands).toEqual([]);expect(prepared.canvas).toBeDefined();
  const actual=JSON.parse(prepared.canvas!.commands);
  // Native f32 serialization may round a coordinate more than JS's formatter.
  const normalized=(value:unknown):unknown=>typeof value==="number"?Math.round(value*1000)/1000:Array.isArray(value)?value.map(normalized):value&&typeof value==="object"?Object.fromEntries(Object.entries(value).map(([k,v])=>[k,normalized(v)])):value;
  expect(normalized(actual)).toEqual(normalized(expected));
  expect(prepared.canvas!.images.length).toBeGreaterThan(0);
  const moved=markdown.call("goto",{line:2,col:4});expect(moved.canvas).toBeDefined();expect(moved.presence?.cursor.line).toBe(2);
 }finally{images.dispose();markdown.dispose();editor.dispose();}
});
