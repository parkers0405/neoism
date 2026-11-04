import {expect,test} from "bun:test";
test("local widget pointers preserve editor/composer focus while keyboard navigation and editing work",async()=>{
  const child=Bun.spawn([process.execPath,"run","tests/support/json-render-controls-check.tsx"],{cwd:process.cwd(),stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
},30000);