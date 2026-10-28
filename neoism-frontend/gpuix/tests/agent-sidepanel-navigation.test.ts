import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
interface Frame{cursor:number[]|null;focused:boolean;selected:string;result?:{action?:string;anchor?:number[];open?:{id:string}};send:unknown[]}
test("original Agent sidebar keyboard traverses New Chat, Search, Projects and real session rows with exact native cursor rectangles",()=>{
 const{id}=coreCall<{id:number}>("agentsidepanel.new",{directory:"/project/a",theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF",fontWeight:400});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<Frame>(`agentsidepanel.${op}`,{id,...args});
 try{
  call("frame",{width:300,height:650});call("event",{message:{ThreadList:{threads:[{session_id:"one",title:"One",directory:"/project/a",updated_at:2},{session_id:"two",title:"Two",directory:"/project/a",updated_at:1}],requested_cursor:null,next_cursor:null}}});
  let frame=call("focus",{focused:true});expect(frame.focused).toBe(true);expect(frame.cursor?.every(Number.isFinite)).toBe(true);
  for(let i=0;i<6&&frame.selected!=="new-chat";i++)frame=call("key",{key:"up"});expect(frame.selected).toBe("new-chat");const top=frame.cursor!;expect(call("key",{key:"enter"}).result).toEqual({action:"new-chat"});
  frame=call("key",{key:"down"});expect(frame.selected).toBe("search");expect(frame.cursor![1]).toBeGreaterThan(top[1]!);expect(call("key",{key:"enter"}).result).toEqual({action:"search"});
  frame=call("key",{key:"down"});expect(frame.selected).toBe("projects");expect(call("key",{key:"enter"}).result).toMatchObject({action:"projects",root:false,anchor:expect.any(Array)});
  frame=call("key",{key:"down"});expect(frame.selected).toBe("row:one");expect(call("key",{key:"enter"}).result?.open?.id).toBe("one");
  frame=call("key",{key:"down"});expect(frame.selected).toBe("row:two");expect(call("key",{key:"enter"}).result?.open?.id).toBe("two");
  expect(call("focus",{focused:false}).cursor).toBeNull();expect(call("focus",{focused:true}).selected).toBe("row:two");expect(call("key",{key:"escape"}).result?.action).toBe("focus-content");
 }finally{call("dispose");}
});
test("loading Agent catalog preserves Search and Projects keyboard selection across native focus callbacks and reset",()=>{
 const{id}=coreCall<{id:number}>("agentsidepanel.new",{directory:"/project/a",theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF"});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<Frame>(`agentsidepanel.${op}`,{id,...args});
 try{
  call("frame",{width:300,height:650});expect(call("focus",{focused:true}).selected).toBe("new-chat");
  for(const selected of ["search","projects"]){
   expect(call("key",{key:"down"}).selected).toBe(selected);
   expect(call("focus",{focused:true}).selected).toBe(selected);
   call("focus",{focused:false});
   expect(call("focus",{focused:true}).selected).toBe(selected);
   call("event",{message:{CatalogReset:{directory:"/project/a"}}});
   expect(call("focus",{focused:true}).selected).toBe(selected);
   const frame=call("key",{key:"enter"});expect(frame.result?.action).toBe(selected);expect(frame.cursor?.every(Number.isFinite)).toBe(true);
  }
 }finally{call("dispose");}
});
