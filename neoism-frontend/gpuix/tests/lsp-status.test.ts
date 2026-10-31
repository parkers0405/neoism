import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {coreCall,NativeEditor} from "../src/native-core";
import {statusInfo} from "../src/chrome/status-info";
const server=(name:string,state="connected")=>({name,state,binary:`/bin/${name}`,filetype:"typescript",source:"managed",runtime_source:"yarn_sdk",runtime_path:"/project/.yarn/sdks/typescript/lib",runtime_version:"5.8.2"});
function fixture(){
 const editor=new NativeEditor("/project/main.ts","const x = 1;",false);
 editor.call("lsp.enable",{surface:"status-test",path:"/project/main.ts"});
 const snapshot=(servers:unknown[],extra:Record<string,unknown>={})=>editor.call("lsp.reply",{message:{LspSnapshot:{surface_id:"status-test",file_path:"/project/main.ts",filetype:"typescript",servers,...extra}}});
 return{editor,snapshot};
}
test("LSP status tracks the scoped daemon snapshot and clears on disconnect",()=>{
 const{editor,snapshot}=fixture();try{
  expect(editor.frame.lsp?.status).toBe("Initializing");
  expect(snapshot([server("typescript","available")]).lsp).toMatchObject({status:"Initializing",label:"typescript"});
  expect(snapshot([server("typescript"),server("eslint")]).lsp).toMatchObject({status:"Active",label:"typescript+1"});
  const revision=editor.frame.lsp?.statusRevision;
  expect(snapshot([server("typescript"),server("eslint")]).lsp?.statusRevision).toBe(revision);
  expect(snapshot([],{file_path:"/project/other.ts"}).lsp?.status).toBe("Active");
  expect(snapshot([],{surface_id:"other-editor"}).lsp?.status).toBe("Active");
  const surface={id:"file",workspaceId:"a",kind:"file" as const,title:"main.ts",path:"main.ts"};
  expect(statusInfo(surface,"/project",{mode:"Insert",line:0,col:0,dirty:false},editor.frame)).toMatchObject({lsp_status:"Active",lsp_label:"LSP"});
  expect(statusInfo({...surface,kind:"terminal"},"/project",{mode:"Normal",line:0,col:0,dirty:false},editor.frame).lsp_status).toBeNull();
  expect(snapshot([server("typescript","error")]).lsp).toMatchObject({status:"Missing",label:"LSP error"});
  expect(snapshot([]).lsp).toMatchObject({status:"Missing",label:null});
  expect(editor.call("lsp.reply",{message:{LspStatus:{surface_id:"status-test",state:"none"}}}).lsp?.status).toBeNull();
  snapshot([server("typescript")]);expect(editor.call("lsp.disable").lsp?.status).toBeNull();
  expect(snapshot([server("late-server")]).lsp?.status).toBeNull();
 }finally{editor.dispose();}
});
interface PopupFrame{visible:boolean;rect:number[];selected:number;rows:Array<{index:number;rect:number[];name:string;state:string}>;commands:Array<{kind:string;text?:string}>;clipboard?:string|null}
test("original LSP details show servers, runtime and live errors with bounded placement",()=>{
 const{editor,snapshot}=fixture();snapshot([server("typescript"),server("eslint","available")]);
 const{id}=coreCall<{id:number}>("lsppopup.new",{editorId:editor.id});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<PopupFrame>(`lsppopup.${op}`,{id,width:800,height:600,anchorX:700,anchorY:580,anchorWidth:90,anchorHeight:22,...args});
 const text=(frame:PopupFrame)=>frame.commands.map(command=>command.text||"").join(" ");
 try{
  let frame=call("open");expect(frame.rows.map(row=>[row.name,row.state])).toEqual([["typescript","Attached"],["eslint","Available"]]);
  expect(text(frame)).toContain("Yarn TypeScript SDK");expect(text(frame)).toContain("5.8.2");
  expect(call("key",{key:"down"}).selected).toBe(1);
  snapshot([server("eslint","error")]);
  editor.call("lsp.reply",{message:{LspMessage:{surface_id:"status-test",server:"eslint",text:"Server crashed while starting",level:"error"}}});
  frame=call("frame");expect(frame.rows[0].state).toBe("Errored");expect(text(frame)).toContain("Server crashed while starting");
  const copy=frame.commands.find(command=>command.kind==="text"&&command.text?.endsWith(" Copy")) as unknown as {x:number;y:number};
  expect(copy).toBeDefined();expect(call("pointer",{x:copy.x+2,y:copy.y+2,activate:true}).clipboard).toBe("Server crashed while starting");
  frame=call("frame",{width:300,anchorX:290});expect(frame.rect[0]).toBeGreaterThanOrEqual(0);expect(frame.rect[0]+frame.rect[2]).toBeLessThanOrEqual(300);
  expect(call("key",{key:"escape"}).visible).toBe(false);
 }finally{coreCall("lsppopup.dispose",{id});editor.dispose();}
});
test("actual App restores the native status hit, live updates, popup and tab scoping",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/lsp-status-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({idleStatusUpdates:true,originalPopup:true,scopedToActiveFile:true,unchangedFrames:240,extraStatusPaints:0,extraPopupSamples:0,extraLspRequests:0});
},15000);
