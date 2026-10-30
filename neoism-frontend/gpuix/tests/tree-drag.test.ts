import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";

function tree(){
 const {id}=coreCall<{id:number}>("tree.new",{root:"/work",scale:1});
 const entries=[{name:"src",path:"/work/src",dir:true,open:true},{name:"a.ts",path:"/work/src/a.ts",dir:false},{name:"dest",path:"/work/dest",dir:true,open:false},{name:"child",path:"/work/src/child",dir:true,open:false}];
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<any>(`tree.${op}`,{id,height:500,...args});
 call("layout",{entries});return {call,entries,dispose:()=>coreCall("dispose",{id})};
}
test("file tree native bridge preserves click threshold, folder and root drops, and illegal targets",()=>{
 const {call,dispose}=tree();try{
  const begin=(index=1)=>call("drag-begin",{index,x:10,y:10});
  const move=(hovered:number|null,inside=true)=>call("drag-update",{x:30,y:60,hovered,inside});
  begin();call("drag-update",{x:11,y:11,hovered:2,inside:true});expect(call("drag-end")).toEqual({kind:"click"});
  begin();expect(move(2).drag).toMatchObject({live:true,source:1,target:2,root:false});expect(call("drag-end")).toEqual({kind:"move",source:"/work/src/a.ts",directory:"/work/dest"});
  begin();expect(move(null).drag.root).toBe(true);expect(call("drag-end").directory).toBe("/work");
  for(const [source,target,inside]of [[1,0,true],[0,3,true],[0,0,true],[1,2,false],[0,null,true]] as const){begin(source);move(target,inside);expect(call("drag-end").kind).toBe("cancel");}
  begin();move(2);call("drag-cancel");expect(move(2).drag).toBeNull();expect(call("drag-end").kind).toBe("cancel");
 }finally{dispose();}
});
test("spring-open uses the original dwell and a path-stable source through relayout",async()=>{
 const {call,entries,dispose}=tree();try{
  call("drag-begin",{index:1,x:0,y:0});const move=()=>call("drag-update",{x:20,y:60,hovered:2,inside:true});
  expect(move().open).toBeNull();await Bun.sleep(470);expect(move().open).toBe("/work/dest");expect(move().open).toBeNull();
  call("layout",{entries:[entries[2],entries[0],entries[3],entries[1]]});
  const next=call("drag-update",{x:20,y:60,hovered:0,inside:true});expect(next.drag).toMatchObject({source:3,target:0});
  expect(call("drag-end")).toEqual({kind:"move",source:"/work/src/a.ts",directory:"/work/dest"});
 }finally{dispose();}
});
