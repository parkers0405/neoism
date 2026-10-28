import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual AltA creates complete fresh Agent input and wordmark before any new layout events",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-new-firstpaint-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualAltA:true,freshNativeComposerReadyInFirstBatch:true,wordmarkPaintAndGeometryReadyBeforeLayout:true,canonicalSplitAndSidebarViewport:true,independentFreshDrafts:true,noPtyOrPrompt:true});
},10000);
