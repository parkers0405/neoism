import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {treeContextItems} from "../src/ui/tree-context-actions";

function menu(args:Record<string,unknown>){const result=coreCall<{id:number;frame:any}>("sidebarcontext.new",{width:640,height:480,scale:1,theme:"pastel_dark",fontFamily:"Cascadia Code NF",...args});return{frame:result.frame,call:(op:string,extra:Record<string,unknown>={})=>coreCall<any>(`sidebarcontext.${op}`,{id:result.id,...extra}),dispose:()=>coreCall("sidebarcontext.dispose",{id:result.id})};}
test("file/folder menus retain desktop action order and background targets only the root",()=>{
 expect(treeContextItems("src/a.ts",true).map(row=>row.label)).toEqual(["Edit / Open","Copy","Paste Here","New File","New Folder","Rename","Delete","Hide Dotfiles"]);
 expect(treeContextItems(undefined,false).map(row=>row.label)).toEqual(["Paste Here","New File","New Folder","Show Dotfiles"]);
});
test("original context menu clamps to window and dispatches keyboard, pointer, dismissal",()=>{
 const m=menu({x:630,y:475,title:"src/file.ts",items:treeContextItems("src/file.ts",true)});
 try{
  const [x,y,w,h]=m.frame.rect;expect(x+w).toBeLessThanOrEqual(640);expect(y+h).toBeLessThanOrEqual(480);
  expect(m.call("key",{key:"r",char:"r"}).result).toEqual({command:"rename"});
  expect(m.call("key",{key:"escape"}).result).toEqual({cancelled:true});
  expect(m.call("pointer",{kind:"down",x:0,y:0}).result).toEqual({cancelled:true});
  const painted=m.frame.commands.find((c:any)=>c.text==="Delete");expect(painted).toBeDefined();
  expect(m.call("pointer",{kind:"down",x:painted.x+3,y:painted.y+5}).result).toEqual({command:"delete"});
 }finally{m.dispose();}
});
test("rename dialog edits the supplied title and deletion is a separate confirmation",()=>{
 const form=menu({kind:"form",title:"Rename Chat",input:"Old",accept:"Rename"});
 try{form.call("key",{key:"end"});form.call("key",{key:"backspace"});form.call("text",{text:"der"});expect(form.call("key",{key:"enter"}).result).toEqual({command:"accept",value:"Older"});}finally{form.dispose();}
 const confirm=menu({kind:"form",title:"Delete Chat?",body:"Delete example?",accept:"Delete"});
 try{expect(confirm.call("key",{key:"escape"}).result).toEqual({cancelled:true});expect(confirm.call("key",{key:"enter"}).result).toEqual({command:"accept"});}finally{confirm.dispose();}
});
test("agent context hit selects the clicked chat without opening it or consuming header clicks",()=>{
 const{id}=coreCall<{id:number}>("agentsidepanel.new",{directory:"/project/a",theme:"pastel_dark",scale:1,fontFamily:"Cascadia Code NF"});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<any>(`agentsidepanel.${op}`,{id,...args});
 try{
  call("frame",{width:300,height:650});call("event",{message:{ThreadList:{threads:[{session_id:"one",title:"One",directory:"/project/a",updated_at:2,pinned:true}],requested_cursor:null,next_cursor:null}}});
  let frame=call("focus",{focused:true});for(let i=0;i<8&&frame.selected!=="row:one";i++)frame=call("key",{key:"down"});expect(frame.selected).toBe("row:one");
  // Find the painted row after the cursor spring settles; hit testing uses row slots.
  const label=frame.commands.find((c:any)=>c.text==="One");expect(label).toBeDefined();
  const result=call("pointer",{kind:"context",x:label.x+3,y:label.y+5}).result;
  expect(result.context).toMatchObject({id:"one",title:"One",pinned:true});expect(result.open).toBeUndefined();
  expect(call("pointer",{kind:"context",x:20,y:640}).result).toBeNull();
 }finally{call("dispose");}
});
