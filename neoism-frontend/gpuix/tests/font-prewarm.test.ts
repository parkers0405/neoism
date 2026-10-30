import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("background font loading is reused by the foreground chat renderer",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/font-prewarm-check.ts"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,output:status===0?undefined:stdout}).toEqual({status:0,stderr:"",output:undefined});
 expect(JSON.parse(stdout)).toMatchObject({ready:true,painted:true,noWindow:true});
},15000);
