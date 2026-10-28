import { expect, test } from "bun:test";
import { toArrayBuffer, type Pointer } from "bun:ffi";
import { coreCall, NativeEditor, type EditorFrame } from "../src/native-core";
import { documentPaintRows } from "../src/editor/paint-frame";
import { palettes, type Palette } from "../src/design/generated";

type Raster = {id:number;ptr:number;len:number;width:number;height:number;scale:number;x:number;y:number};
type Row = EditorFrame["lines"][number] & {raster?:Raster|null};
const colors = (theme:Palette=palettes.pastel_dark) => ({Plain:theme.fg,Punct:theme.fg,Keyword:theme.syn_keyword,Type:theme.syn_type,String:theme.syn_string,Number:theme.syn_number,Comment:theme.syn_comment,Function:theme.syn_func,Property:theme.syn_property,Constructor:theme.syn_constructor,Special:theme.syn_special});
const settings = (extra:Record<string,unknown>={}) => ({fontFamily:"Cascadia Code NF",fontWeight:400,fontSize:14,colors:colors(),...extra});
const configure = (editor:NativeEditor,extra:Record<string,unknown>={}) => editor.call("raster.configure",settings(extra));
const rows = (editor:NativeEditor) => editor.frame.lines as Row[];
function raster(row:Row):Raster {expect(row.raster).toBeDefined();expect(row.raster).not.toBeNull();return row.raster!;}
function pixels(value:Raster) {expect(value.len).toBe(value.width*value.height*4);return new Uint8Array(new Uint8Array(toArrayBuffer(value.ptr as unknown as Pointer,0,value.len)));}
const digest = (row:Row) => Bun.hash(pixels(raster(row))).toString();
const key = (editor:NativeEditor,char:string) => editor.call("key",{key:char,char});
const layout = (editor:NativeEditor,width=1000) => editor.call("layout",{width,height:300,cellWidth:9,rowHeight:21});

// Match the original syntax palette in the actual native RGBA, restricted to
// each run's original display-column interval rather than merely checking IDs.
function hasRunInk(row:Row,run:Row["runs"][number],hex:string) {
 const image=raster(row),bytes=pixels(image),rgb=hex.slice(1,7).match(/../g)!.map(part=>parseInt(part,16));
 for(let y=0;y<image.height;y++)for(let x=0;x<image.width;x++){
  const localX=image.x+x/image.scale;
  if(localX<run.x-1||localX>=run.x+(run.width||0)+1)continue;
  const at=(y*image.width+x)*4;
  if(bytes[at+3]>230&&rgb.every((n,c)=>Math.abs(bytes[at+c]-n)<=2))return true;
 }
 return false;
}

test("code row raster preserves original syntax colors and display-column positions",()=>{
 const editor=new NativeEditor("raster.rs",'fn main() { let n: usize = 42; let s = "hello"; } // comment',true);
 try{
  layout(editor);configure(editor);const row=rows(editor)[0],image=raster(row),map=colors();
  expect(image.scale).toBe(2);expect(image.width).toBeGreaterThan(50);expect(image.height).toBeGreaterThan(5);
  expect(row.runs.map(run=>run.text).join("")).toBe(editor.text());
  for(const token of ["Keyword","Function","Number","String","Comment"]){
   const run=row.runs.find(run=>run.token===token&&run.text.trim());expect(run).toBeDefined();
   expect(hasRunInk(row,run!,map[token as keyof typeof map])).toBe(true);
  }
  let column=0;for(const run of row.runs){expect(run.x).toBeCloseTo(editor.frame.textX+column*9,4);column+=run.text.length;}
  const bytes=pixels(image);expect(bytes.some((n,i)=>i%4===3&&n===0)).toBe(true);
  const again=editor.call("frame").lines[0] as Row;expect(raster(again).id).toBe(image.id);expect(digest(again)).toBe(digest(row));
  const frame=editor.frame,hosts=documentPaintRows([],frame);editor.call("frame");
  expect(editor.frame.lines).toBe(frame.lines);expect(documentPaintRows(hosts,editor.frame)).toBe(hosts);
  configure(editor,{fontSize:16});expect(documentPaintRows(hosts,editor.frame)[0]).not.toBe(hosts[0]);
 }finally{editor.dispose();}
});

test("selection and diagnostics stay separate from cached code glyph pixels",()=>{
 const editor=new NativeEditor("selection.rs",'let count = 42; // selected words',true);
 try{
  layout(editor);configure(editor);const before=rows(editor)[0],bytes=digest(before),id=raster(before).id;
  key(editor,"v");for(let i=0;i<9;i++)key(editor,"l");
  expect(editor.frame.selection).not.toBeNull();expect(rows(editor)[0].runs.some(run=>run.selected)).toBe(true);
  const selected=rows(editor)[0];expect(digest(selected)).toBe(bytes);expect(raster(selected).id).toBe(id);
  const selectedRuns=selected.runs.filter(run=>run.selected);expect(selectedRuns.every(run=>(run.width||0)>0)).toBe(true);
  editor.call("key",{key:"escape"});expect(rows(editor)[0].runs.some(run=>run.selected)).toBe(false);expect(digest(rows(editor)[0])).toBe(bytes);
  editor.call("lsp.enable",{surface:"raster-test",path:"/project/selection.rs",openPaths:["/project/selection.rs"]});
  editor.call("lsp.reply",{message:{Diagnostics:{file_path:"/project/selection.rs",items:[{severity:"Error",message:"Fixture diagnostic",line:0,col:5,end_line:0,end_col:8,lnum:1,source:"raster-test"}]}}});
  expect(rows(editor)[0].runs.some(run=>run.severity==="Error")).toBe(true);
  expect(digest(rows(editor)[0])).toBe(bytes);expect(raster(rows(editor)[0]).id).toBe(id);
 }finally{editor.dispose();}
});

test("horizontal scroll translates cached row pixels without stale glyph placement",()=>{
 const editor=new NativeEditor("horizontal.rs",'let value = "original"; '+"value + ".repeat(40),true);
 try{
  editor.call("layout",{width:300,height:300,cellWidth:8.4,rowHeight:21});editor.call("wrap");editor.call("goto",{line:0,col:12});configure(editor);expect(editor.frame.wrap).toBe(false);
  const before=rows(editor)[0],image=raster(before),bytes=digest(before),snapshot=editor.snapshot();
  for(const offset of [27,27.1,27.125,35.7]){
   snapshot.scrollX=offset;editor.restore(snapshot);const after=rows(editor)[0],scrolled=raster(after);
   expect(scrolled.id).toBe(image.id);expect(scrolled.x).toBeCloseTo(image.x-offset,4);expect(digest(after)).toBe(bytes);
   expect(after.runs[0].x).toBeCloseTo(before.runs[0].x-offset,4);
  }
 }finally{editor.dispose();}
});

test("theme, font settings and same-width edits invalidate row textures while undo restores content",()=>{
 const editor=new NativeEditor("fresh.rs",'let value = "first";',true);
 try{
  layout(editor);configure(editor);const before=rows(editor)[0],original=digest(before),firstId=raster(before).id;
  editor.call("saved",{revision:editor.frame.revision});expect(raster(rows(editor)[0]).id).toBe(firstId);
  editor.call("replace-source",{text:'let value = "other";'});const changed=rows(editor)[0];expect(raster(changed).id).not.toBe(firstId);expect(digest(changed)).not.toBe(original);
  key(editor,"u");expect(editor.text()).toBe('let value = "first";');expect(digest(rows(editor)[0])).toBe(original);
  const old=raster(rows(editor)[0]).id;configure(editor,{colors:colors(palettes.tokyo_night)});
  const themed=rows(editor)[0];expect(raster(themed).id).not.toBe(old);expect(digest(themed)).not.toBe(original);
  const string=themed.runs.find(run=>run.token==="String")!;expect(hasRunInk(themed,string,palettes.tokyo_night.syn_string)).toBe(true);
  for(const change of [{fontWeight:700},{fontSize:18},{fontFamily:"Monocraft"}]){
   const id=raster(rows(editor)[0]).id;configure(editor,change);expect(raster(rows(editor)[0]).id).not.toBe(id);
  }
 }finally{editor.dispose();}
});

test("Unicode tabs and wrapped segments keep distinct row images and remain fresh after reflow",()=>{
 const source='\tlet café = "界 e\u0301 👩🏽‍💻"; '+"value + ".repeat(20),editor=new NativeEditor("unicode.rs",source,true);
 try{
  layout(editor,240);configure(editor);const wrapped=rows(editor).filter(row=>row.line===0);
  expect(wrapped.length).toBeGreaterThan(2);expect(wrapped[0].continuation).toBe(false);expect(wrapped[1].continuation).toBe(true);
  expect(wrapped.flatMap(row=>row.runs).map(run=>run.text).join("")).toContain('café');
  expect(wrapped.flatMap(row=>row.runs).some(run=>run.text.includes("界"))).toBe(true);
  expect(new Set(wrapped.map(row=>raster(row).id)).size).toBeGreaterThan(1);
  for(const row of wrapped){const image=raster(row);expect(Number.isFinite(image.x+image.y)).toBe(true);expect(pixels(image).some((n,i)=>i%4===3&&n>0)).toBe(true);}
  const first=wrapped[0],oldPixels=digest(first);layout(editor,1000);
  const wide=rows(editor).filter(row=>row.line===0);expect(wide.length).toBeLessThan(wrapped.length);expect(digest(wide[0])).not.toBe(oldPixels);
  editor.call("replace-source",{text:source.replace("café","cafe")});expect(digest(rows(editor)[0])).not.toBe(digest(wide[0]));
 }finally{editor.dispose();}
});

test("oversized rows retain token fallback while whitespace and disabled textures cannot leave stale images",()=>{
 const editor=new NativeEditor("fallback.rs","x".repeat(12000),true);
 try{
  layout(editor,300);editor.call("wrap");configure(editor);
  expect(rows(editor)[0].raster).toBeUndefined();expect(rows(editor)[0].runs.map(run=>run.text).join("")).toBe("x".repeat(12000));
  const stats=()=>coreCall<{fallbackRows:number}>("editor.raster.stats",{id:editor.id});
  expect(stats().fallbackRows).toBe(1);editor.call("frame");expect(stats().fallbackRows).toBe(1);
  editor.call("replace-source",{text:"let visible = 1;"});expect(raster(rows(editor)[0]).width).toBeGreaterThan(0);
  configure(editor,{enabled:false});expect(rows(editor)[0].raster).toBeUndefined();expect(rows(editor)[0].runs.map(run=>run.text).join("")).toBe("let visible = 1;");
  configure(editor);expect(rows(editor)[0].raster).toBeDefined();
  editor.call("replace-source",{text:"    "});expect(rows(editor)[0].raster).toBeNull();expect(rows(editor)[0].runs.map(run=>run.text).join("")).toBe("    ");
 }finally{editor.dispose();}
});
