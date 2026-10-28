import { expect, test } from "bun:test";
import { WorkspaceLayout } from "../src/layout/WorkspaceLayout";
import { paneHostKey } from "../src/layout/pane-host";
// Use Solid's client graph, as GPUIX does, instead of Node's SSR export.
const solid = await import(["solid-js","dist","solid.js"].join("/")) as typeof import("solid-js");

const file = (id:string) => ({id,title:`${id}.rs`,kind:"file" as const});

test("native tab selection retains its pane host across leaf and tab-path changes",()=>{
 const layout=new WorkspaceLayout();
 try{
  layout.layout(1000,700);layout.open(file("first"));
  const single=layout.frame.panes[0],host=paneHostKey(layout.frame,single);
  layout.open(file("second"));const second=layout.frame.panes[0];
  expect(second.leaf).not.toBe(single.leaf);expect(second.path).not.toEqual(single.path);
  expect(paneHostKey(layout.frame,second)).toBe(host);
  layout.focusSurface("first");const first=layout.frame.panes[0];
  expect(first.path).not.toEqual(second.path);expect(paneHostKey(layout.frame,first)).toBe(host);
  expect(first.tabs.map(tab=>tab.leaf)).toEqual(second.tabs.map(tab=>tab.leaf));
  layout.closeSurface("first");expect(paneHostKey(layout.frame,layout.frame.panes[0])).toBe(host);
 }finally{layout.dispose();}
});

test("independent split groups keep distinct hosts when their active buffers change",()=>{
 const layout=new WorkspaceLayout();
 try{
  layout.layout(1000,700);layout.open(file("left-one"));layout.open(file("left-two"));
  layout.split("horizontal",file("right-one"));layout.open(file("right-two"));
  const keys=layout.frame.panes.map(pane=>paneHostKey(layout.frame,pane));expect(new Set(keys).size).toBe(2);
  const before=layout.frame.panes.map(pane=>pane.leaf);
  layout.focusSurface("left-one");layout.focusSurface("right-one");
  expect(layout.frame.panes.map(pane=>pane.leaf)).not.toEqual(before);
  expect(layout.frame.panes.map(pane=>paneHostKey(layout.frame,pane))).toEqual(keys);
  layout.split("vertical",file("bottom"));
  expect(new Set(layout.frame.panes.map(pane=>paneHostKey(layout.frame,pane))).size).toBe(3);
 }finally{layout.dispose();}
});

test("Solid retains the actual pane host across repeated native buffer selections",()=>{
 const layout=new WorkspaceLayout();
 try{
  layout.layout(1000,700);layout.open(file("first"));layout.open(file("second"));
  solid.createRoot(dispose=>{
   const [frame,setFrame]=solid.createSignal(layout.frame);let mounts=0,unmounts=0;
   const keys=solid.createMemo(()=>frame().panes.map(pane=>paneHostKey(frame(),pane)));
   const hosts=solid.mapArray(keys,key=>{mounts++;solid.onCleanup(()=>unmounts++);return {key};});
   const original=hosts()[0];
   for(let i=0;i<20;i++){layout.focusSurface(i%2?"first":"second");setFrame(layout.frame);expect(hosts()[0]).toBe(original);}
   expect(mounts).toBe(1);expect(unmounts).toBe(0);dispose();expect(unmounts).toBe(1);
  });
 }finally{layout.dispose();}
});
