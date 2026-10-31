import {expect,test} from "bun:test";
test("actual App keeps connection tabs and local plus routing across host switches",async()=>{
 const child=Bun.spawn([process.execPath,"tests/support/window-sessions-app-check.tsx"],{stdout:"pipe",stderr:"pipe"});
 const [code,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({code,stderr}).toEqual({code:0,stderr:""});expect(stdout).toContain("local plus routing passed");
},20000);
