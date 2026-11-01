import {expect,test} from "bun:test";
import {readingScrollY,wheelPixels} from "../src/ui/wheel";

test("wheel movement relative to logical text is independent of monitor scale",()=>{
 for(const scaleFactor of [1,1.25,5/3,2,3]){
  expect(wheelPixels({deltaX:1,deltaY:-2,precise:false,scaleFactor},14)).toEqual({x:18,y:-36});
  expect(wheelPixels({deltaX:-1.25,deltaY:-7.75,precise:true,scaleFactor},14)).toEqual({x:-1.25,y:-7.75});
 }
 expect(wheelPixels({deltaY:-1},28).y).toBe(-36);
});


test("conversation and diff trackpads travel farther without changing wheels or sideways gestures",()=>{
 for(const scaleFactor of [1,1.25,2,3]) {
  for(const deltaY of [-12.25,-0.125,0,0.125,12.25]) {
   expect(readingScrollY({deltaY,precise:true,scaleFactor})).toBe(deltaY*2);
   expect(readingScrollY({deltaY,precise:false,scaleFactor})).toBe(deltaY);
   expect(readingScrollY({deltaY,scaleFactor})).toBe(deltaY);
   expect(readingScrollY({deltaY,precise:true,modifiers:{shift:true}})).toBe(deltaY);
   expect(readingScrollY({deltaX:20,deltaY,precise:true})).toBe(deltaY);
  }
 }
 // Splitting a gesture into smaller events preserves total travel, including
 // tiny trailing inertia deltas. There is no per-event rounding or minimum.
 const burst=[-12.25,-8.5,-2,-0.125];
 expect(burst.reduce((sum,deltaY)=>sum+readingScrollY({deltaY,precise:true}),0)).toBe(readingScrollY({deltaY:burst.reduce((a,b)=>a+b,0),precise:true}));
});
