import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual App retains tree wheel viewports across panels, workspaces, loading and generations",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tree-scroll-retention-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,independentPanels:true,independentWorkspaces:true,loadingAndRelistsRetained:true,firstPaintRetained:true,fontGenerationRetained:true,emptyAndKeyboardClamp:true,rootScoped:true});
},15000);
