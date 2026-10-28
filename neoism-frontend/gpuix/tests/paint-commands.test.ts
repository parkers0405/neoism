import { expect, test } from "bun:test";
import { sharePaintCommands } from "../src/editor/paint-command";
import type { PaintCommand } from "../src/ui/PaintLayer";

const sample: PaintCommand[] = [
  {kind:"rect",rect:[1,2,3,4],color:[1,.5,0,1],radii:[1,2,3,4],clip:[2,3,40,50],depth:2,order:3,layer:1},
  {kind:"polygon",points:[[1,2],[3,4],[5,6]],color:[.2,.3,.4,1],depth:4,layer:2},
  {kind:"text",x:3,y:4,text:"Original glyphs",size:14,width:105,color:[230,220,210,255],bold:true,italic:false,extrude:true,family:"Monocraft",clip:[0,0,500,400],layer:0,raster:{id:7,ptr:500,len:256,width:8,height:8,scale:2,dx:-1,dy:2}},
];
const clone = () => structuredClone(sample);

test("equal native paint retains command and array identities without serializing", () => {
  const next=clone();expect(sharePaintCommands(sample,next)).toBe(sample);
  for(let i=0;i<sample.length;i++)expect(next[i]).toBe(sample[i]);
});

test("each paint property invalidates only its changed command", () => {
  function changeLeaves(value:unknown,path:Array<string|number>=[]):Array<Array<string|number>> {
    if(value!==null&&typeof value==="object")return Object.entries(value).flatMap(([key,child])=>changeLeaves(child,[...path,Array.isArray(value)?Number(key):key]));
    return [path];
  }
  for(let i=0;i<sample.length;i++)for(const path of changeLeaves(sample[i])) {
    const next=clone();let target:any=next[i];for(const key of path.slice(0,-1))target=target[key];
    const key=path.at(-1)!,value=target[key];target[key]=typeof value==="number"?value+1:typeof value==="boolean"?!value:value+"!";
    const shared=sharePaintCommands(sample,next);expect(shared).not.toBe(sample);expect(shared[i]).not.toBe(sample[i]);
    for(let other=0;other<sample.length;other++)if(other!==i)expect(shared[other]).toBe(sample[other]);
  }
});

test("removed paint, clips, rasters and changed point counts cannot retain stale nodes", () => {
  for(const mutate of [
    (next:PaintCommand[])=>next.pop(),
    (next:PaintCommand[])=>{next.push(structuredClone(sample[0]));},
    (next:PaintCommand[])=>{if(next[0].kind==="rect")next[0].clip=null;},
    (next:PaintCommand[])=>{if(next[2].kind==="text")next[2].raster=null;},
    (next:PaintCommand[])=>{if(next[1].kind==="polygon")next[1].points.push([7,8]);},
  ]){const next=clone();mutate(next);expect(sharePaintCommands(sample,next)).not.toBe(sample);}
  expect(sharePaintCommands([],[])).toEqual([]);
});


test("original shell ring retains identity and invalidates contour/paint changes",()=>{
 const ring:PaintCommand={kind:"ring",outer:[[0,0],[20,0],[20,20],[0,20]],inner:[[1,1],[19,1],[19,19],[1,19]],color:[.3,.4,.5,.75],depth:2,order:4,layer:0};
 const before=[ring];expect(sharePaintCommands(before,structuredClone(before))).toBe(before);
 for(const mutate of [(next:any)=>next[0].inner[0][0]++,(next:any)=>next[0].outer.pop(),(next:any)=>next[0].order++,(next:any)=>next[0].color[3]=.4]){const next=structuredClone(before);mutate(next);expect(sharePaintCommands(before,next)[0]).not.toBe(ring);}
});
