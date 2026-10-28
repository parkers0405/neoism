import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {tabChrome} from "../src/layout/tab-chrome";
import {paintedChromeBounds} from "../src/ui/chrome-target-bounds";
interface Frame {rect:number[]}
test("post-scroll tab cursor uses current painted bounds and lands immediately before every tab icon at fractional scales",()=>{
 for(const scale of [.75,1,20/14,2.5]){
  const chrome=tabChrome(scale).buffer,actual={x:173.25,y:47.5,width:211*scale,height:chrome.height};
  const stale={...actual,x:actual.x+48.5};let reads=0;
  const measured=paintedChromeBounds({getElementBounds(id){expect(id).toBe(17);reads++;return actual;}},17)!;
  expect(measured.x).not.toBe(stale.x);expect(reads).toBe(1);
  for(const kind of ["markdown","file","agent","welcome","terminal"]){
   const id=coreCall<{id:number}>("keyboard.cursor.new").id;
   try{
    const frame=coreCall<Frame>("keyboard.cursor.frame",{id,kind:"buffer",...measured,padding:chrome.padding,scale});
    const iconLeft=actual.x+chrome.padding;
    expect(frame.rect[0]!+frame.rect[2]!).toBeCloseTo(iconLeft,3);
    expect(frame.rect[0]).toBeLessThan(iconLeft);
    expect(frame.rect[1]!+frame.rect[3]!).toBeLessThanOrEqual(actual.y+actual.height+.001);
    expect(frame.rect[0]!+frame.rect[2]!,kind).toBeLessThan(actual.x+chrome.padding+tabChrome(scale,80*scale,(kind==="terminal"?8:kind==="markdown"?15:12)*scale).buffer.iconWidth+chrome.iconGap);
   }finally{coreCall("keyboard.cursor.dispose",{id});}
  }
 }
});
test("inline plus and a moved pane refresh their own actual host geometry without scaling it twice",()=>{
 for(const scale of [20/14,2]){
  const chrome=tabChrome(scale).buffer,pane={x:28,y:76,width:640,height:chrome.height};
  let painted={x:pane.x+pane.width-chrome.newWidth,y:pane.y,width:chrome.newWidth,height:chrome.height};
  const renderer={getElementBounds:()=>painted},id=coreCall<{id:number}>("keyboard.cursor.new").id;
  try{
   const first=paintedChromeBounds(renderer,23)!;
   const plus=coreCall<Frame>("keyboard.cursor.frame",{id,kind:"buffer",...first,scale,padding:chrome.padding,plus:true});
   expect(plus.rect[0]).toBeCloseTo(pane.x+pane.width-chrome.newWidth,3);
   painted={...painted,x:painted.x-216,y:painted.y+4};
   const moved=paintedChromeBounds(renderer,23)!;
   const next=coreCall<Frame>("keyboard.cursor.frame",{id,kind:"buffer",...moved,scale,padding:chrome.padding,plus:true});
   expect(next.rect[0]).toBeCloseTo(plus.rect[0]!-216,3);expect(next.rect[1]).toBeCloseTo(plus.rect[1]!+4,3);
  }finally{coreCall("keyboard.cursor.dispose",{id});}
 }
 expect(paintedChromeBounds({getElementBounds:()=>null},1)).toBeUndefined();
});
