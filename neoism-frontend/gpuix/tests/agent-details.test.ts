import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";

test("agent details default closed and remain toggleable in narrow windows",async()=>{
  // This recording-renderer test needs JSX compilation, not the window display binding.
  const child=Bun.spawn([process.execPath,"--config=tests/support/recording-renderer.bunfig.toml","tests/support/agent-details-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
  const [status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);
  expect({status,stderr,stdout:status===0?undefined:stdout}).toEqual({status:0,stderr:"",stdout:undefined});
},20000);