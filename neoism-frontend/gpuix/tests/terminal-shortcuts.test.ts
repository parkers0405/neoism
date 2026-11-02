import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("terminal respects configured clipboard bindings and retains unbound defaults",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/terminal-shortcuts-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,output:status===0?undefined:stdout}).toEqual({status:0,stderr:"",output:undefined});
 expect(JSON.parse(stdout)).toMatchObject({interruptWithSelection:true,interruptFromVi:true,commandClipboard:true,bracketedPaste:true,globalBindingsRespected:true});
},10000);
