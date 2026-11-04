import {expect,test} from "bun:test";
import {horizontalScrollEvent,readingScrollY,wheelPixels,type WheelEvent} from "../src/ui/wheel";
import {verticalTabScroll} from "../src/layout/tab-scroll";

const horizontalPixels=(event:WheelEvent,fontSize:number)=>wheelPixels(horizontalScrollEvent(event),fontSize);

test("wheel movement relative to logical text is independent of monitor scale",()=>{
 for(const scaleFactor of [1,1.25,5/3,2,3]){
  expect(wheelPixels({deltaX:1,deltaY:-2,precise:false,scaleFactor},14)).toEqual({x:18,y:-36});
  expect(wheelPixels({deltaX:-1.25,deltaY:-7.75,precise:true,scaleFactor},14)).toEqual({x:-1.25,y:-7.75});
 }
 expect(wheelPixels({deltaY:-1},28).y).toBe(-36);
 // Unit conversion stays neutral for vertical-only consumers such as the editor.
 expect(wheelPixels({deltaX:8,deltaY:2,precise:true},14)).toEqual({x:8,y:2});
});

test("horizontal precise gestures match vertical gain without rounding or monitor scaling",()=>{
 for(const scaleFactor of [1,1.25,2,3])for(const deltaX of [-12.25,-0.125,0.125,12.25]){
  const event={deltaX,deltaY:deltaX/2,precise:true,scaleFactor};
  expect(horizontalPixels(event,14)).toEqual({x:deltaX*2,y:deltaX});
  expect(horizontalPixels(event,28)).toEqual(horizontalPixels(event,14));
  expect(horizontalPixels(event,14).x).toBe(readingScrollY({deltaY:deltaX,precise:true}));
  expect(event.deltaX).toBe(deltaX);
  expect(horizontalPixels({...event,precise:false},14)).toEqual({x:deltaX*18,y:deltaX/2*18});
  expect(horizontalPixels({...event,precise:undefined},14)).toEqual({x:deltaX*18,y:deltaX/2*18});
 }
 const burst=[-12.25,-8.5,-2,-0.125];
 expect(burst.reduce((sum,deltaX)=>sum+horizontalPixels({deltaX,precise:true},14).x,0)).toBe(horizontalPixels({deltaX:burst.reduce((a,b)=>a+b,0),precise:true},14).x);
});

test("horizontal normalization preserves dominance, ties, Shift routing and existing vertical gain",()=>{
 for(const [deltaX,deltaY] of [[-4,-3],[3,-4],[-4,4],[0,-.125],[-.125,0]]){
  for(const shift of [false,true]){
   const event={deltaX,deltaY,precise:true,modifiers:{shift}};
   const horizontal=horizontalScrollEvent(event);
   expect(Math.abs(horizontal.deltaX)>Math.abs(horizontal.deltaY)).toBe(Math.abs(deltaX)>Math.abs(deltaY));
   expect(horizontal.modifiers).toBe(event.modifiers);
   // Markdown converts after normalization; timeline normalizes before reading Y;
   // Git review reads Y first, then converts. Neither reading path gains 4x.
   const timeline={x:horizontal.deltaX,y:readingScrollY(horizontal)};
   const review=horizontalPixels({...event,deltaY:readingScrollY(event)},14);
   expect(review).toEqual(timeline);
   expect(horizontalPixels(event,14)).toEqual({x:horizontal.deltaX,y:horizontal.deltaY});
   if(shift){expect(review).toEqual({x:deltaX*2,y:deltaY*2});}
   else if(Math.abs(deltaX)<=Math.abs(deltaY)){
    expect(review).toEqual({x:deltaX,y:deltaY*2});
    expect(horizontalPixels(event,14)).toEqual({x:deltaX,y:deltaY});
   }
  }
 }
 expect(horizontalPixels({deltaY:-.125,precise:true,modifiers:{shift:true}},14)).toEqual({x:0,y:-.25});
 expect(horizontalPixels({deltaY:-1,precise:false,modifiers:{shift:true}},14)).toEqual({x:0,y:-18});
});

test("tab-strip vertical-to-horizontal mapping shares touchpad gain but not native X scrolling",()=>{
 expect(verticalTabScroll({deltaY:-12.25,precise:true},-100,100,1000)).toBe(-124.5);
 expect(verticalTabScroll({deltaY:12.25,precise:true},-100,100,1000)).toBe(-75.5);
 expect(verticalTabScroll({deltaY:-1,precise:false},-100,100,1000)).toBe(-160);
 expect(verticalTabScroll({deltaX:-20,deltaY:-12,precise:true},-100,100,1000)).toBeUndefined();
 expect(verticalTabScroll({deltaX:-1,deltaY:-2,precise:false},-100,100,1000)).toBeUndefined();
 expect(verticalTabScroll({deltaY:-.25,precise:true},-100,100,1000)).toBeUndefined();
 expect(verticalTabScroll({deltaY:-.5,precise:true},-100,100,1000)).toBe(-101);
 expect(verticalTabScroll({deltaY:100,precise:true},-100,100,1000)).toBe(0);
 expect(verticalTabScroll({deltaY:-1000,precise:true},-100,100,1000)).toBe(-900);
 expect(verticalTabScroll({deltaY:-12,precise:true},0,100,50)).toBe(0);
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
