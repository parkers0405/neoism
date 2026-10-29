import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
test("actual Settings modal follows delivered natural-scroll direction for precise and wheel events",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/settings-scroll-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toEqual({noWindow:true,actualSettingsView:true,preciseDirection:true,wheel48Pixels:true,visibleRowsMove:true,topClamped:true});
},10000);
