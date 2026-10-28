import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual App returns to Agent with a complete first paint and retained sidebar glyph images",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-workspace-paint-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),env:{...process.env,NODE_ENV:"test"},stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualProductionApp:true,stableHostsForSameWorkspaceRecord:true,returnFirstFrameAlreadyHasInputAndSidebar:true,nativeComposerRetained:true,sidebarGlyphsPinnedAcrossSwitch:true,noSidebarGlyphReupload:true,independentDraftsPreserved:true,noPromptOrPtySideEffects:true,closedTabComposerRetired:true});
},10000);
