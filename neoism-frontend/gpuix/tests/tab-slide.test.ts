import {expect,test} from "bun:test";
import {TabSlide} from "../src/layout/tab-slide";
function fixture(reducedMotion=false){
 let now=0,changes=0;const callbacks=new Set<()=>void>();
 const slide=new TabSlide<string>({reducedMotion,onChange:()=>changes++,now:()=>now,schedule:callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);}});
 return {slide,callbacks,get changes(){return changes;},tick(ms=16){now+=ms;const pending=[...callbacks];callbacks.clear();pending.forEach(callback=>callback());}};
}
test("neighbor FLIP offsets keep their old visible positions then follow the canonical critically damped spring into new slots",()=>{
 const f=fixture();try{
  f.slide.layout([{id:"a",width:100},{id:"b",width:120},{id:"c",width:80}]);
  f.slide.layout([{id:"b",width:120},{id:"a",width:100},{id:"c",width:80}]);
  expect(f.slide.logical("a")).toBe(120);expect(f.slide.position("a")).toBe(0);expect(f.slide.position("b")).toBe(100);
  f.tick();const omega=4/.12,expected=100*(1+omega*.016)*Math.exp(-omega*.016);
  expect(f.slide.offset("b")).toBeCloseTo(expected,8);expect(f.slide.position("a")).toBeGreaterThan(0);expect(f.slide.position("a")).toBeLessThan(120);
  for(let i=0;i<100&&f.callbacks.size;i++)f.tick();
  expect(f.slide.offset("a")).toBe(0);expect(f.slide.offset("b")).toBe(0);expect(f.callbacks.size).toBe(0);
 }finally{f.slide.dispose();}
});
test("drag follows the pointer independently of slots, interrupts without jumps and eases into its release position",()=>{
 const f=fixture();try{
  f.slide.layout([{id:"a",width:100},{id:"b",width:100},{id:"c",width:100}]);f.slide.begin("a",25);f.slide.move(180);
  expect(f.slide.position("a")).toBe(155);
  f.slide.layout([{id:"b",width:100},{id:"a",width:100},{id:"c",width:100}]);expect(f.slide.position("a")).toBe(155);
  f.tick();const before=f.slide.position("b");f.slide.layout([{id:"a",width:100},{id:"b",width:100},{id:"c",width:100}]);expect(f.slide.position("b")).toBeCloseTo(before,10);
  f.slide.end();expect(f.slide.position("a")).toBe(155);f.tick();expect(f.slide.position("a")).toBeGreaterThan(0);expect(f.slide.position("a")).toBeLessThan(155);
  for(let i=0;i<100&&f.callbacks.size;i++)f.tick();expect(f.slide.position("a")).toBe(0);
 }finally{f.slide.dispose();}
});
test("reduced motion applies new slots immediately, follows direct pointer input and leaves no animation timer",()=>{
 const f=fixture(true);try{
  f.slide.layout([{id:"a",width:100},{id:"b",width:100}]);f.slide.layout([{id:"b",width:100},{id:"a",width:100}]);
  expect(f.slide.offset("a")).toBe(0);expect(f.callbacks.size).toBe(0);
  f.slide.begin("a",120);f.slide.move(170);expect(f.slide.position("a")).toBe(150);f.slide.end();expect(f.slide.position("a")).toBe(100);expect(f.callbacks.size).toBe(0);
 }finally{f.slide.dispose();}
});
test("mount measurements, renamed widths and added or closed tabs do not start entrance slides",()=>{
 const f=fixture();try{
  f.slide.layout([{id:"a",width:200},{id:"b",width:200},{id:"c",width:200}]);
  f.slide.layout([{id:"a",width:90},{id:"b",width:160},{id:"c",width:110}]);
  expect(f.slide.position("b")).toBe(90);expect(f.slide.position("c")).toBe(250);
  f.slide.layout([{id:"a",width:150},{id:"b",width:160},{id:"c",width:110}]);
  expect(f.slide.position("b")).toBe(150);expect(f.slide.offset("b")).toBe(0);
  f.slide.layout([{id:"new",width:80},{id:"a",width:150},{id:"b",width:160},{id:"c",width:110}]);
  expect(f.slide.position("a")).toBe(80);expect(f.slide.offset("a")).toBe(0);
  f.slide.layout([{id:"new",width:80},{id:"b",width:160},{id:"c",width:110}]);
  expect(f.slide.position("b")).toBe(80);expect(f.slide.offset("b")).toBe(0);expect(f.callbacks.size).toBe(0);
 }finally{f.slide.dispose();}
});
test("width corrections preserve a slide already in flight without replaying inactive neighbors",()=>{
 const f=fixture();try{
  f.slide.layout([{id:"a",width:100},{id:"b",width:100},{id:"c",width:100}]);
  f.slide.layout([{id:"b",width:100},{id:"a",width:100},{id:"c",width:100}]);f.tick();
  const before=f.slide.position("a");
  f.slide.layout([{id:"b",width:140},{id:"a",width:100},{id:"c",width:100}]);
  expect(f.slide.position("a")).toBeCloseTo(before,10);expect(f.slide.offset("c")).toBe(0);
  f.tick();expect(f.slide.position("a")).toBeGreaterThan(before);
  for(let i=0;i<100&&f.callbacks.size;i++)f.tick();expect(f.slide.position("a")).toBe(140);expect(f.callbacks.size).toBe(0);
 }finally{f.slide.dispose();}
});
