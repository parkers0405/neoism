import {expect,test} from "bun:test";
import {WorkspaceLayout} from "../src/layout/WorkspaceLayout";
import {TabDropPreview,tabDropPoint} from "../src/layout/TabDropPreview";

test("live split preview contains actual panes without publishing or persisting a hover",()=>{
 const layout=new WorkspaceLayout(),preview=new TabDropPreview();
 try{
  layout.open({id:"a",title:"a",kind:"file"});layout.open({id:"b",title:"b",kind:"terminal"});layout.layout(1000,700);
  const source=layout.frame.focusedLeaf!,saved=layout.snapshot();let published=0;layout.subscribe(()=>published++);
  const right=preview.update(layout,source,990,350,32)!;
  expect(right.panes.map(p=>p.surfaceId)).toEqual(["a","b"]);
  expect(right.panes[0].rect[2]).toBeLessThan(500);
  expect(right.panes[1].rect[0]).toBeGreaterThan(500);
  expect(preview.update(layout,source,980,350,32)).toBe(right);
  const bottom=preview.update(layout,source,500,690,32)!;
  expect(bottom.dividers[0].axis).toBe("vertical");
  expect(layout.snapshot()).toEqual(saved);expect(published).toBe(0);
  preview.clear();expect(layout.snapshot()).toEqual(saved);
  layout.drop(source,500,690);expect(layout.frame.tree).toEqual(bottom.tree);
 }finally{layout.dispose();}
});

test("dropping a split onto the original tab strip previews and commits adoption",()=>{
 const layout=new WorkspaceLayout(),preview=new TabDropPreview();
 try{
  layout.open({id:"a",title:"a",kind:"file"});layout.split("vertical",{id:"b",title:"b",kind:"terminal"});layout.layout(1000,700);
  const source=layout.frame.focusedLeaf!,saved=layout.snapshot();
  const point=tabDropPoint(layout.frame,source,100,12,32);
  const merged=preview.update(layout,source,100,12,32)!;
  expect(merged.panes).toHaveLength(1);expect(merged.dividers).toHaveLength(0);
  expect(merged.panes[0].tabs.map(tab=>tab.surfaceId)).toEqual(["a","b"]);
  expect(layout.snapshot()).toEqual(saved);
  layout.drop(source,point.x,point.y);
  expect(layout.frame.tree).toEqual(merged.tree);
  expect(preview.update(layout,source,-20,-20,32)).toBeUndefined();
 }finally{layout.dispose();}
});
