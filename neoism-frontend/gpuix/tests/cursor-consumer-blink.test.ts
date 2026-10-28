import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual chrome, terminal and Finder carets resume opaque idle blinking without window or new input",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/cursor-consumer-blink-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualChromeFinderTerminalConsumers:true,opaqueLocalCursorColors:true,idleHoldExpires:true,idleBlinkKeepsRunning:true,terminalInputRestoresVisibility:true,finderPaintHostRetained:true});
},12000);
