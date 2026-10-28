import {expect,test} from "bun:test";
import {WorkspaceLayout} from "../src/layout/WorkspaceLayout";
const file=(id:string)=>({id,title:id,kind:"file" as const});
test("canonical local tab reorder preserves selected view identities and recovery order",()=>{
 const layout=new WorkspaceLayout();let restored:WorkspaceLayout|undefined;
 try{
  for(const id of ["one","two","three","four"])layout.open(file(id));
  const original=layout.frame.leaves.map(leaf=>leaf.leaf),focus=layout.frame.focusedLeaf!;
  layout.reorder(original[0]!,original[2]!,false);
  expect(layout.frame.panes[0].tabs.map(tab=>tab.surfaceId)).toEqual(["two","three","one","four"]);
  expect(layout.frame.focusedLeaf).toBe(focus);
  layout.reorder(focus,original[1]!,true);
  expect(layout.frame.panes[0].tabs.map(tab=>tab.surfaceId)).toEqual(["four","two","three","one"]);
  expect(layout.frame.focusedLeaf).toBe(focus);expect([...layout.frame.leaves.map(leaf=>leaf.leaf)].sort()).toEqual([...original].sort());
  restored=new WorkspaceLayout(layout.snapshot());expect(restored.frame.panes[0].tabs).toEqual(layout.frame.panes[0].tabs);
 }finally{restored?.dispose();layout.dispose();}
});
test("local reorder rejects cross-pane leaves atomically and self-reorder keeps the tree intact",()=>{
 const layout=new WorkspaceLayout();
 try{
  layout.open(file("left"));const left=layout.frame.focusedLeaf!;layout.open(file("hidden"));
  layout.split("horizontal",file("right"));const right=layout.frame.focusedLeaf!,before=layout.snapshot();
  expect(()=>layout.reorder(left,right)).toThrow("different pane groups");expect(layout.snapshot()).toEqual(before);
  layout.reorder(left,left);expect(layout.snapshot()).toEqual(before);
 }finally{layout.dispose();}
});
