import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("chat pointer interaction preserves sidebar keyboard focus while Alt navigation still transfers it",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-sidebar-focus-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,sidebarSurvivesTranscriptSelection:true,explicitKeyboardTransfer:true});
},15000);
