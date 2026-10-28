import {expect,test} from "bun:test";
import {toArrayBuffer,type Pointer} from "bun:ffi";
import {coreCall} from "../src/native-core";
interface Row{raster:{id:number;ptr:number;len:number;width:number;height:number;scale:number};label:string;labelX:number;labelY:number;iconX:number;rowHeight:number}
const newHost=(weight=400)=>coreCall<{id:number}>("treeglyphs.new",{family:"Cascadia Code NF",weight,fonts:["Cascadia Code NF","Symbols Nerd Font Mono"]}).id;
const parameters={name:"glyph-row.ts",width:300,scale:1,chevron:"\uf054",icon:"\uf15b",labelColor:"#00ff00",iconColor:"#ff0000",chevronColor:"#0000ff"};
const row=(id:number,extra:Record<string,unknown>={})=>coreCall<Row>("treeglyphs.row",{id,...parameters,...extra});
const pixels=(r:Row)=>new Uint8Array(new Uint8Array(toArrayBuffer(r.raster.ptr as unknown as Pointer,0,r.raster.len)));
const ink=(r:Row,channel:number)=>{
 const bytes=pixels(r),positions:number[][]=[];
 for(let y=0;y<r.raster.height;y++)for(let x=0;x<r.raster.width;x++){
  const at=(y*r.raster.width+x)*4;
  if(bytes[at+3]>200&&bytes[at+channel]>250&&[0,1,2].filter(c=>c!==channel).every(c=>bytes[at+c]<3))positions.push([x/2,y/2]);
 }
 return positions;
};
test("tree row image preserves original FileTree icon boxes, label baseline and color pixels",()=>{
 const id=newHost();
 try{
  const r=row(id);expect(r.label).toBe(parameters.name);expect(r.labelX).toBe(35);expect(r.iconX).toBe(14);expect(r.labelY).toBe(6.5);expect(r.rowHeight).toBe(26);
  expect(r.raster.scale).toBe(2);expect(r.raster.width).toBe(600);expect(r.raster.height).toBe(52);expect(r.raster.len).toBe(600*52*4);
  const label=ink(r,1),icon=ink(r,0),chevron=ink(r,2);
  expect(label.length).toBeGreaterThan(30);expect(icon.length).toBeGreaterThan(5);expect(chevron.length).toBeGreaterThan(1);
  expect(Math.min(...label.map(p=>p[0]))).toBeGreaterThanOrEqual(34);
  expect(Math.min(...icon.map(p=>p[0]))).toBeGreaterThanOrEqual(13);expect(Math.max(...icon.map(p=>p[0]))).toBeLessThanOrEqual(28);
  expect(Math.min(...chevron.map(p=>p[0]))).toBeGreaterThanOrEqual(0);expect(Math.max(...chevron.map(p=>p[0]))).toBeLessThanOrEqual(11);
  expect(pixels(r).some((n,i)=>i%4===3&&n===0)).toBe(true);
 }finally{coreCall("treeglyphs.dispose",{id});}
});
test("tree glyph cache retains repeated row pixels and invalidates width, selection ink, folder state and scale",()=>{
 const id=newHost();
 try{
  const first=row(id),digest=Bun.hash(pixels(first));expect(row(id).raster.id).toBe(first.raster.id);expect(Bun.hash(pixels(row(id)))).toBe(digest);
  for(const change of [{width:180},{labelColor:"#eeeeee"},{chevron:"\uf078"},{icon:"\uf07b"},{name:"other-name.ts"},{scale:1.25}])expect(row(id,change).raster.id).not.toBe(first.raster.id);
  expect(row(id).raster.id).toBe(first.raster.id);
  const scaled=row(id,{scale:2});expect(scaled.labelX).toBe(70);expect(scaled.labelY).toBe(13);expect(scaled.rowHeight).toBe(52);
 }finally{coreCall("treeglyphs.dispose",{id});}
});
test("long Unicode tree names use the original shaped ellipsis and clip within the row",()=>{
 const id=newHost();
 try{
  const name="界-café-e\u0301-".repeat(20)+".tsx",r=row(id,{name,width:120});expect(r.label).toEndWith("…");expect(r.label).not.toContain("\ufffd");expect(r.label.length).toBeLessThan(name.length);
  const label=ink(r,1);expect(label.length).toBeGreaterThan(5);expect(Math.max(...label.map(p=>p[0]))).toBeLessThanOrEqual(117);
  const empty=row(id,{width:20});expect(empty.label).toBe("");
 }finally{coreCall("treeglyphs.dispose",{id});}
});
