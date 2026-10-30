import {expect,test} from "bun:test";
test("real App tree input retains press activation and shared drag behavior",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tree-drag-check.tsx"],{stdout:"pipe",stderr:"pipe"});
 const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
 expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
 expect(JSON.parse(stdout)).toMatchObject({pressActivation:true,springOpen:true,move:true,outsideCancel:true,escapeCancel:true,nativeWindowsCreated:0});
},15_000);
