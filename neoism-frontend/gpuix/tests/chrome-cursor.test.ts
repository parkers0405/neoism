import {expect,test} from "bun:test";
import {createRoot} from "solid-js";
import type {HostElement} from "@gpuix/solid";
import {coreCall} from "../src/native-core";
import {registerSurfaceCaret,surfaceCaret,suppressSurfaceCursor,surfaceCursorSuppressed} from "../src/ui/cursor-handoff";
interface Frame {rect:number[];quad:number[][];animating:boolean;hold:boolean}
const center=(quad:number[][])=>quad.reduce((sum,point)=>sum+point[0]!,0)/quad.length;
test("chrome trail starts at its visible source after idle, crosses intermediate geometry, then settles",async()=>{
 const id=coreCall<{id:number}>("keyboard.cursor.new").id;
 try{
  const initial=coreCall<Frame>("keyboard.cursor.frame",{id,kind:"buffer",x:20,y:80,width:150,height:28,padding:12,scale:1});
  await Bun.sleep(180);
  const args={id,kind:"workspace",x:700,y:30,width:400,height:32,scale:1};
  const first=coreCall<Frame>("keyboard.cursor.frame",args);
  expect(first.quad).toEqual(initial.quad);expect(first.animating).toBe(true);expect(first.hold).toBe(true);
  const start=center(first.quad),end=first.rect[0]!+first.rect[2]!/2,positions:number[]=[];
  let frame=first;
  for(let index=0;index<120&&frame.animating;index++){await Bun.sleep(5);frame=coreCall<Frame>("keyboard.cursor.frame",args);positions.push(center(frame.quad));}
  expect(positions.filter(value=>value>start+1&&value<end-1).length).toBeGreaterThan(2);
  expect(positions[0]!).toBeLessThan(end-1);expect(frame.animating).toBe(false);expect(center(frame.quad)).toBeCloseTo(end,1);
  await Bun.sleep(160);
  const seed=[390,440,10,22];coreCall("keyboard.cursor.seed",{id,rect:seed});
  const tree=coreCall<Frame>("keyboard.cursor.frame",{id,kind:"tree",x:10,y:100,width:280,height:200,rowY:126,rowHeight:26,scale:1});
  expect(tree.quad).toEqual([[390,440],[400,440],[400,462],[390,462]]);
  expect(tree.rect[0]).toBeCloseTo(14.2,4);expect(tree.rect[1]).toBe(129);expect(tree.rect[2]).toBeCloseTo(7.8,4);expect(tree.rect[3]).toBe(20);expect(tree.animating).toBe(true);
 }finally{coreCall("keyboard.cursor.dispose",{id});}
});
test("surface handoff preserves published window coordinates and suppresses local carets only during travel",()=>{
 createRoot(dispose=>{
  const renderer={},host={id:7,mounted:true} as HostElement;
  const release=registerSurfaceCaret(renderer,host,()=>[[320,180],[330,180],[330,202],[320,202]]);
  expect(surfaceCaret(renderer,7)?.bounds).toEqual({x:320,y:180,width:10,height:22});
  expect(surfaceCursorSuppressed(renderer)).toBe(false);suppressSurfaceCursor(renderer,true);expect(surfaceCursorSuppressed(renderer)).toBe(true);suppressSurfaceCursor(renderer,false);expect(surfaceCursorSuppressed(renderer)).toBe(false);
  release();expect(surfaceCaret(renderer,7)).toBeUndefined();dispose();
 });
});
