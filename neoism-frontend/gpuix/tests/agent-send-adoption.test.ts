import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
for(const args of [[],["--reduced"],["--reject"],["--queue"],["--background"],["--admission"]])test(`real App first send retains timeline host and arrival through adoption ${args.join(" ")}`,async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/agent-send-adoption-check.tsx",...args],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toMatchObject({noWindow:true,noEmptyChatFrame:true,rejected:args.includes("--reject"),queued:args.includes("--queue")});
},15000);