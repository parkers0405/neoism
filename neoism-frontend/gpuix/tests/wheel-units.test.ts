import {expect,test} from "bun:test";
import {wheelPixels} from "../src/ui/wheel";

test("wheel movement relative to logical text is independent of monitor scale",()=>{
 for(const scaleFactor of [1,1.25,5/3,2,3]){
  expect(wheelPixels({deltaX:1,deltaY:-2,precise:false,scaleFactor},14)).toEqual({x:18,y:-36});
  expect(wheelPixels({deltaX:-1.25,deltaY:-7.75,precise:true,scaleFactor},14)).toEqual({x:-1.25,y:-7.75});
 }
 expect(wheelPixels({deltaY:-1},28).y).toBe(-36);
});
