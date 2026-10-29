import{expect,test}from"bun:test";
import{fileURLToPath}from"node:url";
import{coreCall,NativeEditor}from"../src/native-core";
interface Frame{visible:boolean;interactive:boolean;rect:number[];rows:Array<{index:number;rect:number[];line:number;message:string}>;commands:Array<{kind:string;text?:string}>;selected:number;target?:{line:number;column:number}|null}
const diagnostic=(line:number,severity:string,message:string)=>({line,col:2,end_line:line,end_col:3,lnum:line+1,severity,message});
function fixture(pill:"Error"|"Warn",items=[diagnostic(6,"Error","late\nmessage"),diagnostic(2,"Error","early error"),diagnostic(4,"Warn","one warning")]){
 const editor=new NativeEditor("/project/main.ts",Array.from({length:40},(_,n)=>`line${n}`).join("\n"),true);
 editor.call("lsp.enable",{surface:"diagnostics-test",path:"/project/main.ts",openPaths:["/project/main.ts"]});
 editor.call("lsp.reply",{message:{Diagnostics:{file_path:"/project/main.ts",items}}});
 const{id}=coreCall<{id:number}>("diagnosticpopup.new",{editorId:editor.id,pill});
 const call=(operation:string,args:Record<string,unknown>={})=>coreCall<Frame>(`diagnosticpopup.${operation}`,{id,width:800,scale:1,theme:"pastel_dark",anchorX:400,anchorY:570,...args});
 call("open");return{editor,call,dispose:()=>{coreCall("diagnosticpopup.dispose",{id});editor.dispose();}};
}
test("canonical error/warning status popovers use actual active diagnostics and line navigation",async()=>{
 for(const pill of ["Error","Warn"]as const){const f=fixture(pill);try{
  await Bun.sleep(180);const frame=f.call("frame");expect(frame.interactive).toBe(true);
  expect(frame.commands.filter(c=>c.kind==="text").map(c=>c.text).join(" ")).toContain(pill==="Error"?"Errors — 2":"Warnings — 1");
  expect(frame.rows.map(row=>row.line)).toEqual(pill==="Error"?[3,7]:[5]);
  if(pill==="Error"){expect(frame.rows[1].message).toBe("late  message");f.call("key",{key:"down"});expect(f.call("key",{key:"enter"}).target).toEqual({line:6,column:0});}
  else{const row=frame.rows[0].rect;expect(f.call("pointer",{x:row[0]+10,y:row[1]+5,activate:true}).target).toEqual({line:4,column:0});}
 }finally{f.dispose();}}
});
test("canonical popover scrolls bounded rows, refreshes diagnostics and fits narrow windows",async()=>{
 const items=Array.from({length:30},(_,line)=>diagnostic(line,"Error",`error ${line}`)),f=fixture("Error",items);
 try{
  await Bun.sleep(180);const first=f.call("frame",{width:280,anchorX:140});expect(first.rows).toHaveLength(10);expect(first.rect[0]).toBeGreaterThanOrEqual(0);expect(first.rect[0]+first.rect[2]).toBeLessThanOrEqual(280);
  const scrolled=f.call("wheel",{deltaY:-120,precise:true,y:first.rows[0].rect[1]});expect(scrolled.rows[0].line).toBeGreaterThan(1);
  f.editor.call("lsp.reply",{message:{Diagnostics:{file_path:"/project/main.ts",items:[diagnostic(8,"Error","changed")]}}});
  const updated=f.call("frame");expect(updated.rows.map(row=>row.line)).toEqual([9]);expect(updated.rows[0].message).toBe("changed");
  f.call("close");await Bun.sleep(180);expect(f.call("frame").visible).toBe(false);
 }finally{f.dispose();}
});
test("short content bodies at large font scales keep every keyboard-selected diagnostic visible",async()=>{
 for(const [scale,height]of[[1,160],[2,200],[3,240]]){
  const f=fixture("Error",Array.from({length:20},(_,line)=>diagnostic(line,"Error",`short-window diagnostic ${line}`)));
  const bounds={width:280,height,scale,anchorX:140,anchorY:height};
  try{
   await Bun.sleep(180);let frame=f.call("frame",bounds);
   const visible=(frame:Frame)=>{
    expect(frame.rows.length).toBeGreaterThan(0);expect(frame.rows.length).toBeLessThan(10);
    expect(frame.rect[0]).toBeGreaterThanOrEqual(0);expect(frame.rect[0]+frame.rect[2]).toBeLessThanOrEqual(bounds.width);
    expect(frame.rect[1]).toBeGreaterThanOrEqual(0);expect(frame.rect[1]+frame.rect[3]).toBeLessThanOrEqual(height);
    const selected=frame.rows.find(row=>row.index===frame.selected);expect(selected).toBeDefined();
    expect(selected!.rect[1]).toBeGreaterThanOrEqual(0);expect(selected!.rect[1]+selected!.rect[3]).toBeLessThanOrEqual(height);
   };
   visible(frame);
   for(let index=1;index<20;index++){frame=f.call("key",{...bounds,key:"down"});expect(frame.selected).toBe(index);visible(frame);}
   frame=f.call("frame",{...bounds,height:1200,anchorY:1200});expect(frame.rows).toHaveLength(10);
   frame=f.call("frame",bounds);visible(frame);
   const selected=frame.rows.find(row=>row.index===frame.selected)!;
   expect(f.call("pointer",{...bounds,x:selected.rect[0]+5,y:selected.rect[1]+selected.rect[3]/2,activate:true}).target).toEqual({line:19,column:0});
  }finally{f.dispose();}
 }
});
test("actual App status error/warning clicks open original popup and move focused file cursor",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/diagnostics-popup-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,errorAndWarningBadgeClicks:true,originalRowsAndCounts:true,clickAndKeyboardJump:true,editorFocusRestored:true,noUnexpectedFileOrPty:true});
},15000);
