import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("confirmed server and system removal deletes saved entries and refreshes both lists",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/server-removal-check.ts"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr}).toEqual({status:0,stderr:""});
 expect(JSON.parse(stdout)).toEqual({systemRemoved:true,serverRemoved:true,listsRefreshed:true});
},10000);
