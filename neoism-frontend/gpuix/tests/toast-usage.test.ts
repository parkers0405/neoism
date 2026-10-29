import{expect,test}from"bun:test";
import{coreCall}from"../src/native-core";
import{fileURLToPath}from"node:url";
type Command={kind:string;text?:string;rect?:number[]};
test("original Rust toasts stack, dedupe, wrap, pause on hover and retain messages across theme/font reconfiguration",()=>{
 const{id}=coreCall<{id:number}>("notifications.new");
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<{commands:Command[];rects:number[][];active:boolean;messages:string[]}>(`notifications.${op}`,{id,width:900,height:600,scale:1,theme:"pastel_dark",fontFamily:"Cascadia Code NF",fontWeight:400,...args});
 try{
  const first=call("push",{message:"Original warning",level:"warn"});expect(first.messages).toEqual(["Original warning"]);expect(first.rects[0][0]).toBe(556);expect(first.rects[0][1]).toBe(16);expect(first.commands.some(command=>command.kind==="text"&&command.text==="Original warning")).toBe(true);
  expect(call("push",{message:"Original warning",level:"warn"}).rects).toHaveLength(1);
  const stacked=call("push",{message:"A long error ".repeat(30),level:"error"});expect(stacked.rects).toHaveLength(2);expect(stacked.rects[1][3]).toBeGreaterThan(first.rects[0][3]);
  const hovered=call("hover",{x:first.rects[0][0]+10,y:first.rects[0][1]+10});expect(hovered.messages).toHaveLength(2);
  const changed=call("frame",{scale:1.5,theme:"pastel_light",width:280});expect(changed.messages).toEqual(stacked.messages);for(const rect of changed.rects){expect(rect[0]).toBeGreaterThanOrEqual(0);expect(rect[0]+rect[2]).toBeLessThanOrEqual(280);}
  call("dismiss",{index:1});call("hover",{x:changed.rects[0][0]+1,y:changed.rects[0][1]+1,scale:1.5,width:280});expect(call("frame",{scale:1.5,width:280}).active).toBe(false);expect(call("leave").active).toBe(true);
 }finally{coreCall("notifications.dispose",{id});}
});
test("usage picker paints original account plans, remaining bars, reset details, inline account errors and skeleton",()=>{
 const{id}=coreCall<{id:number}>("picker.new",{kind:"usage",title:"Codex",loading:true});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<any>(`picker.${op}`,{id,anchor:[20,650,900,100],minY:100,scale:1,theme:"pastel_dark",fontFamily:"Cascadia Code NF",fontWeight:400,...args});
 try{
  const loading=call("paint");expect(loading.animating).toBe(true);expect(loading.commands.some((command:Command)=>command.kind==="rect")).toBe(true);
  call("usage",{loading:false,usage:{accounts:[{connection_id:"a",label:"Patriot",is_default:true,auth_type:"oauth",plan_type:"team",error:null,windows:[{label:"5-hour",used_percent:0,reset_at:1900000000,limit_window_seconds:18000},{label:"weekly",used_percent:25,reset_at:1900500000,limit_window_seconds:604800}]},{connection_id:"b",label:"Communities",is_default:false,auth_type:"oauth",plan_type:"pro",error:"OpenAI usage service rejected the request",windows:[]}]}});
  const ready=call("paint"),text=ready.commands.filter((command:Command)=>command.kind==="text").map((command:Command)=>command.text).join(" ");
  for(const token of ["Codex","2 accounts","Patriot","Team","5-hour limit","100% left","Weekly limit","75% left","Communities","OpenAI usage service rejected the request"])expect(text).toContain(token);
  expect(text).toContain("UTC");expect(ready.rect[1]).toBeGreaterThanOrEqual(100);expect(ready.rect[1]+ready.rect[3]).toBeLessThan(650);
 }finally{coreCall("picker.dispose",{id});}
});
test("actual App keeps original toast below tabs and slash skeletons hydrate in place into structured usage",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/toast-usage-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,originalToastBelowChrome:true,toastSurvivesGeneration:true,immediateUsageSkeleton:true,usageHydratesInPlace:true,structuredAccountsAndBars:true,refreshAndEscape:true,noBackendMutation:true});
},15000);
