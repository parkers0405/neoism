import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("Home switches between a retained shell and the real Agent UI without opening a tab",async()=>{
  const child=Bun.spawn([process.execPath,"run","tests/support/home-agent-mode-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr}).toEqual({status:0,stderr:""});
  expect(JSON.parse(stdout)).toEqual({actualApp:true,noWindow:true,homeLayoutRetained:true,independentDrafts:true,pendingAdoptionSafe:true,streamingWhileInTerminal:true,slashCommands:true,normalTabNavigation:true});
},15000);