import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual tree h toggles cached dotfiles independently per workspace/panel, retaining selection and Left navigation",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tree-hidden-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({actualApp:true,noWindow:true,plainHToggles:true,nestedDotfiles:true,canonicalExclusions:true,selectedPathPreserved:true,independentWorkspaceAndNotes:true,zoomRetained:true,arrowLeftStillCollapses:true});
},15000);
