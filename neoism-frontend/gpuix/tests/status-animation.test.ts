import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("bottom status mode switches keep original Rust scramble/colors and run to completion on native display wakes",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/status-animation-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toEqual({noWindow:true,actualNativeEditorMode:true,originalRainbowPixels:true,nativeDisplayDriven:true,original320msSettle:true,backgroundCrossfade:true,agentPageTransition:true,idleStops:true,stableStatusHost:true});
},10000);
