import {expect,test} from "bun:test";
import {observeTitlebarInset} from "../src/chrome/window-titlebar";

test("Mac chrome follows native fullscreen changes and stops observing on disposal",async()=>{
 let fullscreen=false,reads=0;const insets:number[]=[];
 const dispose=observeTitlebarInset({isFullscreen(){reads++;return fullscreen;}},"darwin",value=>insets.push(value));
 try{
  expect(insets).toEqual([80]);
  fullscreen=true;await Bun.sleep(120);expect(insets).toEqual([80,0]);
  fullscreen=false;await Bun.sleep(120);expect(insets).toEqual([80,0,80]);
 }finally{dispose();}
 const stopped=reads;await Bun.sleep(120);expect(reads).toBe(stopped);
});

test("other platforms reserve no traffic-light space or fullscreen polling",()=>{
 for(const platform of ["linux","win32"]){
  const insets:number[]=[];const dispose=observeTitlebarInset({isFullscreen(){throw new Error("must not query");}},platform,value=>insets.push(value));
  expect(insets).toEqual([0]);dispose();
 }
});
